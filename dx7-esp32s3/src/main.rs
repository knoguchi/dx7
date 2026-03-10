#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

extern crate alloc;

use esp_println::println;
use esp_backtrace as _;
use dx7_core::voice::Voice;
use dx7_core::load_rom1a_voice;
use dx7_core::tables::N;

#[cfg(feature = "es8311")]
mod es8311;

const SAMPLE_RATE: u32 = 48000;
const CPU_HZ: u32 = 240_000_000;
#[cfg(not(feature = "ble-midi"))]
const BLOCK_DEADLINE_US: u64 = (N as u64 * 1_000_000) / SAMPLE_RATE as u64;
#[cfg(feature = "pwm")]
const CYCLES_PER_SAMPLE: u32 = CPU_HZ / SAMPLE_RATE;

#[inline(always)]
fn read_ccount() -> u32 {
    let val: u32;
    unsafe { core::arch::asm!("rsr.ccount {}", out(reg) val) };
    val
}

#[cfg(feature = "qemu")]
#[allow(static_mut_refs)]
fn init_heap() {
    const HEAP_SIZE: usize = 64 * 1024;
    static mut HEAP: [u8; HEAP_SIZE] = [0u8; HEAP_SIZE];
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            HEAP.as_mut_ptr(),
            HEAP_SIZE,
            esp_alloc::MemoryCapability::Internal.into(),
        ));
    }
}

// === BLE MIDI mode: async entry point via esp-rtos ===
#[cfg(feature = "ble-midi")]
#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 200 * 1024);

    println!("DX7 ESP32-S3 — BLE MIDI");

    dx7_core::tables::init_tables(SAMPLE_RATE);
    dx7_core::lfo::init_lfo(SAMPLE_RATE);
    dx7_core::pitchenv::init_pitchenv(SAMPLE_RATE);

    // Init scheduler (BLE radio needs it)
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // Run benchmark
    let patch = load_rom1a_voice(10).unwrap();
    let mut voice = Voice::new();
    voice.note_on(&patch, 36, 100);
    let mut output = [0i32; N];
    let start = read_ccount();
    voice.render(&mut output);
    let end = read_ccount();
    let total_cycles = end.wrapping_sub(start);
    let us_per_block = total_cycles / (CPU_HZ / 1_000_000);
    println!("1v: {} cyc/blk  {} us/blk", total_cycles, us_per_block);

    ble_midi_synth(peripherals.BT).await;
}

// === Non-BLE modes: sync entry point ===
#[cfg(not(feature = "ble-midi"))]
#[esp_hal::main]
fn main() -> ! {
    #[cfg(feature = "qemu")]
    {
        init_heap();
    }

    #[cfg(not(feature = "qemu"))]
    let peripherals = {
        let p = esp_hal::init(esp_hal::Config::default());
        esp_alloc::heap_allocator!(size: 200 * 1024);
        p
    };

    println!("DX7 ESP32-S3 Benchmark");

    dx7_core::tables::init_tables(SAMPLE_RATE);
    dx7_core::lfo::init_lfo(SAMPLE_RATE);
    dx7_core::pitchenv::init_pitchenv(SAMPLE_RATE);

    let patch = load_rom1a_voice(10).unwrap();
    let mut voice = Voice::new();
    voice.note_on(&patch, 36, 100);

    let mut output = [0i32; N];
    let start = read_ccount();
    voice.render(&mut output);
    let end = read_ccount();

    let total_cycles = end.wrapping_sub(start);
    let us_per_block = total_cycles / (CPU_HZ / 1_000_000);
    let status = if (us_per_block as u64) < BLOCK_DEADLINE_US { "OK" } else { "OVER" };
    println!("1v: {} cyc/blk  {} us/blk  {}", total_cycles, us_per_block, status);

    #[cfg(not(feature = "qemu"))]
    let _ = &peripherals;

    #[cfg(feature = "pwm")]
    {
        pwm_playback(&patch);
    }

    #[cfg(feature = "es8311")]
    {
        i2s_playback(&patch);
    }

    println!("\nDone.");
    loop {}
}

// === BLE MIDI live synth ===
#[cfg(feature = "ble-midi")]
#[allow(dead_code)]
async fn ble_midi_synth(bluetooth: esp_hal::peripherals::BT<'static>) -> ! {
    use trouble_host::prelude::*;
    use esp_radio::ble::controller::BleConnector;

    static MIDI_QUEUE: dx7_midi::MidiQueue = dx7_midi::MidiQueue::new();

    println!("Initializing BLE...");

    let radio_ctrl = esp_radio::init().unwrap();
    let connector = BleConnector::new(&radio_ctrl, bluetooth, Default::default()).unwrap();
    let controller: ExternalController<_, 1> = ExternalController::new(connector);

    let address: Address = Address::random([0xD7, 0x07, 0x42, 0x01, 0x02, 0x03]);

    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
    let Host { mut peripheral, mut runner, .. } = stack.build();

    // GATT server with BLE MIDI service
    #[gatt_server]
    struct MidiServer {
        midi_svc: MidiSvc,
    }

    #[gatt_service(uuid = "03B80E5A-EDE8-4B33-A751-6CE34EC4C700")]
    struct MidiSvc {
        #[characteristic(uuid = "7772E5DB-3868-4112-A1A9-F2669D106BF3", write_without_response, read, notify)]
        midi_io: [u8; 20],
    }

    let server = MidiServer::new_with_config(GapConfig::default("DX7")).unwrap();

    // Setup I2S audio
    #[cfg(feature = "es8311")]
    let (mut i2s_tx, _pa) = setup_i2s_es8311();

    let mut voice = Voice::new();
    let mut current_patch = load_rom1a_voice(0).unwrap();
    let mut output = [0i32; N];
    #[cfg(feature = "es8311")]
    let mut i2s_buf = [0i16; N * 2];

    println!("BLE MIDI ready. Advertising as 'DX7'...");

    let ble_task = async {
        loop {
            let mut adv_buf = [0u8; 31];
            let adv_len = AdStructure::encode_slice(
                &[
                    AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                    AdStructure::CompleteLocalName(b"DX7"),
                ],
                &mut adv_buf,
            ).unwrap();

            let adv_data = Advertisement::ConnectableScannableUndirected {
                adv_data: &adv_buf[..adv_len],
                scan_data: &[],
            };

            let advertiser = peripheral.advertise(&Default::default(), adv_data).await.unwrap();
            let conn = advertiser.accept().await.unwrap();
            println!("BLE connected!");

            let gatt_conn = conn.with_attribute_server(&server).unwrap();

            loop {
                match gatt_conn.next().await {
                    GattConnectionEvent::Disconnected { .. } => {
                        println!("BLE disconnected");
                        break;
                    }
                    GattConnectionEvent::Gatt { event } => {
                        match event {
                            GattEvent::Write(write_evt) => {
                                let data = write_evt.data();
                                dx7_midi::ble::parse_ble_midi_packet(data, &MIDI_QUEUE);
                                let _ = write_evt.accept();
                            }
                            other => { let _ = other.accept(); }
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    let audio_task = async {
        loop {
            // Process MIDI
            while let Some(msg) = MIDI_QUEUE.pop() {
                match msg {
                    dx7_midi::MidiMessage::NoteOn { note, velocity, .. } => {
                        voice.note_on(&current_patch, note, velocity);
                    }
                    dx7_midi::MidiMessage::NoteOff { .. } => {
                        voice.note_off();
                    }
                    dx7_midi::MidiMessage::ProgramChange { program, .. } => {
                        if let Some(p) = load_rom1a_voice(program as usize) {
                            current_patch = p;
                        }
                    }
                    dx7_midi::MidiMessage::ControlChange { .. } => {}
                    _ => {}
                }
            }

            // Render audio
            voice.render(&mut output);

            #[cfg(feature = "es8311")]
            {
                for i in 0..N {
                    let sample = (output[i] >> 9) as i16;
                    i2s_buf[i * 2] = sample;
                    i2s_buf[i * 2 + 1] = sample;
                }
                i2s_tx.write_words(&i2s_buf).unwrap();
            }

            embassy_futures::yield_now().await;
        }
    };

    // Run BLE stack + BLE task + audio concurrently
    let _ = embassy_futures::join::join3(runner.run(), ble_task, audio_task).await;
    unreachable!()
}

#[cfg(all(feature = "ble-midi", feature = "es8311"))]
fn setup_i2s_es8311() -> (
    esp_hal::i2s::master::I2sTx<'static, esp_hal::Blocking>,
    esp_hal::gpio::Output<'static>,
) {
    use esp_hal::i2s::master::{I2s, Config, Channels, DataFormat};
    use esp_hal::i2c::master::{I2c, Config as I2cConfig};
    use esp_hal::time::Rate;
    use esp_hal::gpio::{Level, Output, OutputConfig};

    let pa_pin = unsafe { esp_hal::peripherals::GPIO46::steal() };
    let pa = Output::new(pa_pin, Level::High, OutputConfig::default());

    let i2c_scl = unsafe { esp_hal::peripherals::GPIO14::steal() };
    let i2c_sda = unsafe { esp_hal::peripherals::GPIO15::steal() };
    let mut i2c = I2c::new(
        unsafe { esp_hal::peripherals::I2C0::steal() },
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    ).unwrap()
    .with_scl(i2c_scl)
    .with_sda(i2c_sda);

    const MCLK_HZ: u32 = SAMPLE_RATE * 256;
    es8311::init(&mut i2c, MCLK_HZ, SAMPLE_RATE);
    println!("ES8311 initialized");

    let dma_channel = unsafe { esp_hal::peripherals::DMA_CH0::steal() };
    let i2s_periph = unsafe { esp_hal::peripherals::I2S0::steal() };
    let i2s = I2s::new(
        i2s_periph, dma_channel,
        Config::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(SAMPLE_RATE))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO),
    ).unwrap();
    let i2s = i2s.with_mclk(unsafe { esp_hal::peripherals::GPIO16::steal() });

    static mut TX_DESC: [esp_hal::dma::DmaDescriptor; 8] = [esp_hal::dma::DmaDescriptor::EMPTY; 8];
    #[allow(static_mut_refs)]
    let tx_descriptors = unsafe { &mut TX_DESC };

    let i2s_tx = i2s.i2s_tx
        .with_bclk(unsafe { esp_hal::peripherals::GPIO9::steal() })
        .with_ws(unsafe { esp_hal::peripherals::GPIO45::steal() })
        .with_dout(unsafe { esp_hal::peripherals::GPIO8::steal() })
        .build(tx_descriptors);

    (i2s_tx, pa)
}

/// Play audio through I2S + ES8311 codec (demo mode).
#[cfg(all(feature = "es8311", not(feature = "ble-midi")))]
fn i2s_playback(patch: &dx7_core::DxVoice) {
    use esp_hal::i2s::master::{I2s, Config, Channels, DataFormat};
    use esp_hal::i2c::master::{I2c, Config as I2cConfig};
    use esp_hal::time::Rate;
    use esp_hal::gpio::{Level, Output, OutputConfig};

    println!("I2S audio via ES8311 (16-bit, {} Hz)", SAMPLE_RATE);

    let pa_pin = unsafe { esp_hal::peripherals::GPIO46::steal() };
    let _pa = Output::new(pa_pin, Level::High, OutputConfig::default());

    let i2c_scl = unsafe { esp_hal::peripherals::GPIO14::steal() };
    let i2c_sda = unsafe { esp_hal::peripherals::GPIO15::steal() };
    let mut i2c = I2c::new(
        unsafe { esp_hal::peripherals::I2C0::steal() },
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    ).unwrap()
    .with_scl(i2c_scl)
    .with_sda(i2c_sda);

    const MCLK_HZ: u32 = SAMPLE_RATE * 256;
    es8311::init(&mut i2c, MCLK_HZ, SAMPLE_RATE);
    println!("ES8311 initialized (MCLK={}Hz)", MCLK_HZ);

    let dma_channel = unsafe { esp_hal::peripherals::DMA_CH0::steal() };
    let i2s_periph = unsafe { esp_hal::peripherals::I2S0::steal() };
    let i2s = I2s::new(
        i2s_periph, dma_channel,
        Config::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(SAMPLE_RATE))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO),
    ).unwrap();
    let i2s = i2s.with_mclk(unsafe { esp_hal::peripherals::GPIO16::steal() });

    static mut TX_DESC: [esp_hal::dma::DmaDescriptor; 8] = [esp_hal::dma::DmaDescriptor::EMPTY; 8];
    #[allow(static_mut_refs)]
    let tx_descriptors = unsafe { &mut TX_DESC };

    let mut i2s_tx = i2s.i2s_tx
        .with_bclk(unsafe { esp_hal::peripherals::GPIO9::steal() })
        .with_ws(unsafe { esp_hal::peripherals::GPIO45::steal() })
        .with_dout(unsafe { esp_hal::peripherals::GPIO8::steal() })
        .build(tx_descriptors);

    let mut voice = Voice::new();
    voice.note_on(patch, 60, 100);
    let note_blocks = (SAMPLE_RATE as usize * 2) / N;
    let mut output = [0i32; N];
    let mut i2s_buf = [0i16; N * 2];

    println!("Playing {} blocks...", note_blocks);
    for block in 0..note_blocks {
        voice.render(&mut output);
        if block == note_blocks / 2 { voice.note_off(); }
        for i in 0..N {
            let sample = (output[i] >> 9) as i16;
            i2s_buf[i * 2] = sample;
            i2s_buf[i * 2 + 1] = sample;
        }
        i2s_tx.write_words(&i2s_buf).unwrap();
    }
    i2s_buf.fill(0);
    i2s_tx.write_words(&i2s_buf).unwrap();
    println!("Playback done.");
}

/// Play audio through LEDC PWM on GPIO4.
#[cfg(feature = "pwm")]
fn pwm_playback(patch: &dx7_core::DxVoice) {
    use esp_hal::ledc::{Ledc, LSGlobalClkSource, LowSpeed};
    use esp_hal::ledc::timer::{self, TimerIFace};
    use esp_hal::ledc::channel::{self, ChannelIFace, ChannelHW};
    use esp_hal::gpio::DriveMode;

    println!("PWM audio on GPIO4 (8-bit, 312 kHz)");

    let mut ledc = Ledc::new(unsafe { esp_hal::peripherals::LEDC::steal() });
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);

    let mut timer0 = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    timer0.configure(timer::config::Config {
        duty: timer::config::Duty::Duty8Bit,
        clock_source: timer::LSClockSource::APBClk,
        frequency: esp_hal::time::Rate::from_khz(312),
    }).unwrap();

    let gpio4 = unsafe { esp_hal::peripherals::GPIO4::steal() };
    let mut channel0 = ledc.channel(channel::Number::Channel0, gpio4);
    channel0.configure(channel::config::Config {
        timer: &timer0,
        duty_pct: 50,
        drive_mode: DriveMode::PushPull,
    }).unwrap();

    let mut voice = Voice::new();
    voice.note_on(patch, 60, 100);
    let note_blocks = (SAMPLE_RATE as usize * 2) / N;
    let mut output = [0i32; N];

    println!("Playing {} blocks...", note_blocks);
    for block in 0..note_blocks {
        voice.render(&mut output);
        if block == note_blocks / 2 { voice.note_off(); }
        let block_start = read_ccount();
        for i in 0..N {
            let signed = output[i] >> 17;
            let duty = (signed + 128).clamp(0, 255) as u32;
            channel0.set_duty_hw(duty);
            let target = block_start.wrapping_add(CYCLES_PER_SAMPLE * (i as u32 + 1));
            while read_ccount().wrapping_sub(target) > CYCLES_PER_SAMPLE {}
        }
    }
    channel0.set_duty_hw(128);
    println!("Playback done.");
}
