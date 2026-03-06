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
const CYCLES_PER_SAMPLE: u32 = CPU_HZ / SAMPLE_RATE; // 5442
const BLOCK_DEADLINE_US: u64 = (N as u64 * 1_000_000) / SAMPLE_RATE as u64;

#[inline(always)]
fn read_ccount() -> u32 {
    let val: u32;
    unsafe { core::arch::asm!("rsr.ccount {}", out(reg) val) };
    val
}

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

#[esp_hal::main]
fn main() -> ! {
    #[cfg(feature = "qemu")]
    let _peripherals = {
        init_heap();
        ()
    };

    #[cfg(not(feature = "qemu"))]
    let _peripherals = {
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

    // Benchmark one block
    let mut output = [0i32; N];
    let start = read_ccount();
    voice.render(&mut output);
    let end = read_ccount();

    let total_cycles = end.wrapping_sub(start);
    let us_per_block = total_cycles / (CPU_HZ / 1_000_000);
    let status = if (us_per_block as u64) < BLOCK_DEADLINE_US { "OK" } else { "OVER" };
    println!("1v: {} cyc/blk  {} us/blk  {}", total_cycles, us_per_block, status);

    // PWM audio output (bare ESP32-S3, no codec)
    #[cfg(feature = "pwm")]
    {
        pwm_playback(&patch);
    }

    // I2S audio via ES8311 codec (1.28" box board)
    #[cfg(feature = "es8311")]
    {
        i2s_playback(&patch);
    }

    println!("\nDone.");
    loop {}
}

/// Play audio through I2S + ES8311 codec.
/// Board: ESP32S3-1.28inch-BOX
///   I2S_BCLK=GPIO9, I2S_LRCK=GPIO45, I2S_DOUT=GPIO8, MCLK=GPIO16
///   I2C_SCL=GPIO14, I2C_SDA=GPIO15
///   PA_CTRL=GPIO46 (NS4150B speaker amp enable)
#[cfg(feature = "es8311")]
fn i2s_playback(patch: &dx7_core::DxVoice) {
    use esp_hal::i2s::master::{I2s, Config, Channels, DataFormat};
    use esp_hal::i2c::master::{I2c, Config as I2cConfig};
    use esp_hal::time::Rate;
    use esp_hal::gpio::{Level, Output, OutputConfig};
    use esp_hal::dma_buffers;

    println!("I2S audio via ES8311 (16-bit, {} Hz)", SAMPLE_RATE);

    // Enable speaker amplifier (GPIO46 = PA_CTRL)
    let pa_pin = unsafe { esp_hal::peripherals::GPIO46::steal() };
    let _pa = Output::new(pa_pin, Level::High, OutputConfig::default());

    // Configure ES8311 codec over I2C
    let i2c_scl = unsafe { esp_hal::peripherals::GPIO14::steal() };
    let i2c_sda = unsafe { esp_hal::peripherals::GPIO15::steal() };
    let mut i2c = I2c::new(
        unsafe { esp_hal::peripherals::I2C0::steal() },
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    ).unwrap()
    .with_scl(i2c_scl)
    .with_sda(i2c_sda);

    // MCLK = 256 * sample_rate. For 48kHz: 12.288MHz
    const MCLK_HZ: u32 = SAMPLE_RATE * 256;
    es8311::init(&mut i2c, MCLK_HZ, SAMPLE_RATE);
    println!("ES8311 initialized (MCLK={}Hz)", MCLK_HZ);

    // Setup I2S
    let dma_channel = unsafe { esp_hal::peripherals::DMA_CH0::steal() };
    let i2s_periph = unsafe { esp_hal::peripherals::I2S0::steal() };

    let i2s = I2s::new(
        i2s_periph,
        dma_channel,
        Config::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(SAMPLE_RATE))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO),
    ).unwrap();

    let mclk_pin = unsafe { esp_hal::peripherals::GPIO16::steal() };
    let i2s = i2s.with_mclk(mclk_pin);

    let bclk_pin = unsafe { esp_hal::peripherals::GPIO9::steal() };
    let ws_pin = unsafe { esp_hal::peripherals::GPIO45::steal() };
    let dout_pin = unsafe { esp_hal::peripherals::GPIO8::steal() };

    // DMA descriptors (static lifetime)
    static mut TX_DESC: [esp_hal::dma::DmaDescriptor; 8] = [esp_hal::dma::DmaDescriptor::EMPTY; 8];
    let tx_descriptors = unsafe { &mut TX_DESC };

    let mut i2s_tx = i2s
        .i2s_tx
        .with_bclk(bclk_pin)
        .with_ws(ws_pin)
        .with_dout(dout_pin)
        .build(tx_descriptors);

    // Play a note: render blocks and stream via I2S
    let mut voice = Voice::new();
    voice.note_on(patch, 60, 100); // Middle C

    let note_blocks = (SAMPLE_RATE as usize * 2) / N; // 2 seconds
    let mut output = [0i32; N];

    // I2S write buffer: 16-bit stereo = 4 bytes per sample, N samples per block
    let mut i2s_buf = [0i16; N * 2]; // L, R interleaved

    println!("Playing {} blocks...", note_blocks);
    for block in 0..note_blocks {
        voice.render(&mut output);

        // Release after 1 second
        if block == note_blocks / 2 {
            voice.note_off();
        }

        // Convert i32 (24-bit range) to i16 stereo
        for i in 0..N {
            let sample = (output[i] >> 9) as i16; // 24-bit -> 16-bit
            i2s_buf[i * 2] = sample;     // Left
            i2s_buf[i * 2 + 1] = sample; // Right (mono)
        }

        i2s_tx.write_words(&i2s_buf).unwrap();
    }

    // Silence
    i2s_buf.fill(0);
    i2s_tx.write_words(&i2s_buf).unwrap();

    println!("Playback done.");
}

/// Play audio through LEDC PWM on GPIO4.
/// Connect GPIO4 → 1kΩ → capacitor 100nF → GND, tap between R and C for audio.
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

    // Play a note: render blocks and output samples via PWM
    let mut voice = Voice::new();
    voice.note_on(patch, 60, 100); // Middle C

    let note_blocks = (SAMPLE_RATE as usize * 2) / N; // 2 seconds
    let mut output = [0i32; N];

    println!("Playing {} blocks...", note_blocks);
    for block in 0..note_blocks {
        voice.render(&mut output);

        // Release after 1 second
        if block == note_blocks / 2 {
            voice.note_off();
        }

        // Output each sample with cycle-accurate timing
        let block_start = read_ccount();
        for i in 0..N {
            // Convert voice output (signed ~24-bit) to unsigned 8-bit
            // Voice output is roughly ±(1<<24). Shift down and bias to 0..255.
            let signed = output[i] >> 17; // ±128 range
            let duty = (signed + 128).clamp(0, 255) as u32;
            channel0.set_duty_hw(duty);

            // Wait until this sample's time slot
            let target = block_start.wrapping_add(CYCLES_PER_SAMPLE * (i as u32 + 1));
            while read_ccount().wrapping_sub(target) > CYCLES_PER_SAMPLE {
                // spin
            }
        }
    }

    // Silence
    channel0.set_duty_hw(128);
    println!("Playback done.");
}
