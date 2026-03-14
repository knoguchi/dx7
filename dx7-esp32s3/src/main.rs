#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

use esp_println::println;
use esp_backtrace as _;
use dx7_core::voice::{Voice, VoiceState};
use dx7_core::load_rom1a_voice;
use dx7_core::tables::N;


#[cfg(feature = "es8311")]
mod es8311;
#[cfg(feature = "lcd")]
mod lcd;

const SAMPLE_RATE: u32 = 48000;
const CPU_CLOCK: esp_hal::clock::CpuClock = esp_hal::clock::CpuClock::_160MHz;
#[allow(dead_code)]
const CPU_HZ: u32 = CPU_CLOCK as u32 * 1_000_000;
const MAX_VOICES: usize = 8;

#[cfg(not(any(feature = "ble-midi", feature = "usb-midi")))]
const BLOCK_DEADLINE_US: u64 = (N as u64 * 1_000_000) / SAMPLE_RATE as u64;
#[cfg(feature = "pwm")]
const CYCLES_PER_SAMPLE: u32 = CPU_HZ / SAMPLE_RATE;

#[allow(dead_code)]
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

// --- Channel state for MIDI dispatch ---

struct ChannelState {
    patch: dx7_core::DxVoice,
    pitch_bend: i32,
    mod_wheel: i32,
    sustain: bool,
}

impl ChannelState {
    fn new(patch: dx7_core::DxVoice) -> Self {
        Self { patch, pitch_bend: 0, mod_wheel: 0, sustain: false }
    }
}

// --- Output filter (DC blocker + 4th-order Butterworth LPF at 10.5kHz) ---

struct BiquadF32 {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    x1: f32, x2: f32,
    y1: f32, y2: f32,
}

impl BiquadF32 {
    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
              - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

struct DcBlockerF32 {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlockerF32 {
    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

struct OutputFilterF32 {
    dc1: DcBlockerF32,
    dc2: DcBlockerF32,
    lpf1: BiquadF32,
    lpf2: BiquadF32,
}

impl OutputFilterF32 {
    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let x = self.dc1.process(x);
        let x = self.dc2.process(x);
        let x = self.lpf1.process(x);
        self.lpf2.process(x)
    }
}

// --- Dual-core voice rendering ---

#[cfg(any(feature = "ble-midi", feature = "usb-midi"))]
mod mc_render {
    use core::sync::atomic::{AtomicBool, Ordering};
    use dx7_core::voice::Voice;
    use dx7_core::tables::N;
    use super::MAX_VOICES;

    pub static mut VOICES: core::mem::MaybeUninit<[Voice; MAX_VOICES]> =
        core::mem::MaybeUninit::uninit();
    pub static mut VOICE_AGES: [u32; MAX_VOICES] = [0; MAX_VOICES];
    pub static mut VOICE_AGE: u32 = 0;

    /// Core 1 render output buffer.
    pub static mut CORE1_BUF: [i32; N] = [0i32; N];

    /// Core 0 sets RENDER_START=true to signal core 1.
    pub static RENDER_START: AtomicBool = AtomicBool::new(false);
    /// Core 1 sets RENDER_DONE=true when finished.
    pub static RENDER_DONE: AtomicBool = AtomicBool::new(true);

    /// Core 1 entry: render voices MAX_VOICES/2..MAX_VOICES on demand.
    #[allow(static_mut_refs)]
    pub fn core1_entry() {
        let voices = unsafe { VOICES.assume_init_mut() };

        loop {
            while !RENDER_START.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
            RENDER_START.store(false, Ordering::Relaxed);

            unsafe { CORE1_BUF.fill(0); }
            for idx in (MAX_VOICES / 2)..MAX_VOICES {
                if !voices[idx].is_finished() {
                    let mut buf = [0i32; N];
                    voices[idx].render(&mut buf);
                    for j in 0..N {
                        unsafe { CORE1_BUF[j] = CORE1_BUF[j].saturating_add(buf[j]); }
                    }
                }
            }

            RENDER_DONE.store(true, Ordering::Release);
        }
    }
}

// --- Voice dispatch (ported from RP2350) ---

#[cfg(any(feature = "ble-midi", feature = "usb-midi"))]
fn dispatch_midi(
    msg: dx7_midi::MidiMessage,
    voices: &mut [Voice; MAX_VOICES],
    voice_ages: &mut [u32; MAX_VOICES],
    voice_age: &mut u32,
    channels: &mut [ChannelState; 16],
) {
    match msg {
        dx7_midi::MidiMessage::NoteOn { note, velocity, .. } => {
            // Kill any existing voice on same note
            for v in voices.iter_mut() {
                if v.note == note && !v.is_finished() {
                    v.state = VoiceState::Inactive;
                }
            }
            *voice_age += 1;
            // Steal priority: inactive > sustain-held > released > oldest active
            let slot = voices.iter().position(|v| v.state == VoiceState::Inactive)
                .or_else(|| {
                    voices.iter().enumerate()
                        .filter(|(_, v)| v.sustain_held)
                        .min_by_key(|(i, _)| voice_ages[*i])
                        .map(|(i, _)| i)
                })
                .or_else(|| {
                    voices.iter().enumerate()
                        .filter(|(_, v)| v.state == VoiceState::Released)
                        .min_by_key(|(i, _)| voice_ages[*i])
                        .map(|(i, _)| i)
                })
                .unwrap_or_else(|| {
                    (0..MAX_VOICES).min_by_key(|&i| voice_ages[i]).unwrap()
                });
            voices[slot].state = VoiceState::Inactive;
            voices[slot].note_on(&channels[0].patch, note, velocity);
            voices[slot].sustain_held = false;
            voices[slot].pitch_bend = channels[0].pitch_bend;
            voices[slot].mod_wheel = channels[0].mod_wheel;
            voice_ages[slot] = *voice_age;
        }
        dx7_midi::MidiMessage::NoteOff { note, .. } => {
            for v in voices.iter_mut() {
                if v.note == note && !v.is_finished() {
                    if channels[0].sustain {
                        v.sustain_held = true;
                    } else {
                        v.note_off();
                    }
                }
            }
        }
        dx7_midi::MidiMessage::PitchBend { value, .. } => {
            let bend_signed = value as i32 - 8192;
            let bend_range_semitones = 12;
            let bend = (bend_signed * bend_range_semitones * 256) / 8192;
            channels[0].pitch_bend = bend;
            for v in voices.iter_mut() {
                if !v.is_finished() {
                    v.pitch_bend = bend;
                }
            }
        }
        dx7_midi::MidiMessage::ControlChange { controller, value, .. } => {
            match controller {
                1 => {
                    let mw = value as i32;
                    channels[0].mod_wheel = mw;
                    for v in voices.iter_mut() {
                        if !v.is_finished() {
                            v.mod_wheel = mw;
                        }
                    }
                }
                64 => {
                    let on = value >= 64;
                    channels[0].sustain = on;
                    if !on {
                        for v in voices.iter_mut() {
                            if v.sustain_held {
                                v.sustain_held = false;
                                v.note_off();
                            }
                        }
                    }
                }
                120 | 123 => {
                    for v in voices.iter_mut() {
                        if !v.is_finished() {
                            v.note_off();
                        }
                    }
                }
                _ => {}
            }
        }
        dx7_midi::MidiMessage::ProgramChange { program, .. } => {
            if let Some(p) = load_rom1a_voice(program as usize) {
                #[cfg(feature = "lcd")]
                lcd::draw_patch(program, p.name_str());
                channels[0].patch = p;
            }
        }
        _ => {}
    }
}

// === BLE MIDI mode: async entry point via esp-rtos ===
#[cfg(feature = "ble-midi")]
#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let config = esp_hal::Config::default()
        .with_cpu_clock(CPU_CLOCK);
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 200 * 1024);

    println!("DX7 ESP32-S3 — BLE MIDI, {} voices, CPU {}MHz",
        MAX_VOICES, esp_hal::clock::CpuClock::max() as u32);

    dx7_core::tables::init_tables(SAMPLE_RATE);
    dx7_core::lfo::init_lfo(SAMPLE_RATE);
    dx7_core::pitchenv::init_pitchenv(SAMPLE_RATE);

    // Init scheduler (BLE radio needs it)
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // Initialize LCD display
    #[cfg(feature = "lcd")]
    {
        lcd::init();
        lcd::draw_patch(0, load_rom1a_voice(0).unwrap().name_str());
        println!("LCD initialized");
    }

    // Initialize shared voice pool
    #[allow(static_mut_refs)]
    unsafe {
        mc_render::VOICES.write(core::array::from_fn(|_| Voice::new()));
    }

    // Start core 1 for parallel voice rendering
    use esp_hal::interrupt::software::SoftwareInterruptControl;
    let sw_ints = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    static mut CORE1_STACK: esp_hal::system::Stack<8192> = esp_hal::system::Stack::new();
    #[allow(static_mut_refs)]
    esp_rtos::start_second_core(
        peripherals.CPU_CTRL,
        sw_ints.software_interrupt0,
        sw_ints.software_interrupt1,
        unsafe { &mut CORE1_STACK },
        mc_render::core1_entry,
    );
    println!("Core 1 started");

    ble_midi_synth(peripherals.BT).await;
}

// === BLE MIDI live synth (4 voices, I2S ES8311 output) ===
#[cfg(feature = "ble-midi")]
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

    // Setup I2S + ES8311 audio
    #[cfg(feature = "es8311")]
    let (mut i2s_tx, _pa) = setup_i2s_es8311();

    // Start circular DMA — gapless audio output
    // DMA buffer: 4 blocks worth of stereo i16 samples
    #[cfg(feature = "es8311")]
    static mut DMA_BUF: [u8; N * 2 * 2 * 4] = [0u8; N * 2 * 2 * 4];
    #[cfg(feature = "es8311")]
    #[allow(static_mut_refs)]
    let mut dma_transfer = i2s_tx.write_dma_circular(unsafe { &mut DMA_BUF }).unwrap();

    // Voice pool (shared with core 1 via statics)
    #[allow(static_mut_refs)]
    let voices = unsafe { mc_render::VOICES.assume_init_mut() };
    #[allow(static_mut_refs)]
    let voice_ages = unsafe { &mut mc_render::VOICE_AGES };
    #[allow(static_mut_refs)]
    let voice_age = unsafe { &mut mc_render::VOICE_AGE };
    // Start with ROM1A patch 0 (BRASS 1); switch via Program Change
    let init_patch = load_rom1a_voice(0).unwrap();
    let mut channels: [ChannelState; 16] = core::array::from_fn(|_| ChannelState::new(init_patch.clone()));

    #[cfg(feature = "es8311")]
    let mut i2s_buf = [0i16; N * 2];

    // Output filter (static to avoid async stack pressure)
    static mut FILTER: OutputFilterF32 = OutputFilterF32 {
        dc1: DcBlockerF32 { r: 0.9993455, x1: 0.0, y1: 0.0 },
        dc2: DcBlockerF32 { r: 0.9993455, x1: 0.0, y1: 0.0 },
        lpf1: BiquadF32 {
            b0: 0.21113742, b1: 0.42227485, b2: 0.21113742,
            a1: -0.20469809, a2: 0.04924778,
            x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0,
        },
        lpf2: BiquadF32 {
            b0: 0.29262414, b1: 0.58524828, b2: 0.29262414,
            a1: -0.28369960, a2: 0.45419615,
            x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0,
        },
    };
    #[allow(static_mut_refs)]
    let filter = unsafe { &mut FILTER };

    println!("BLE MIDI ready — {} voices. Advertising as 'DX7'...", MAX_VOICES);

    let ble_task = async {
        loop {
            let mut adv_buf = [0u8; 31];
            // BLE MIDI service UUID (little-endian)
            const MIDI_SVC_UUID: [u8; 16] = [
                0x00, 0xC7, 0xC4, 0x4E, 0xE3, 0x6C, 0x51, 0xA7,
                0x33, 0x4B, 0xE8, 0xED, 0x5A, 0x0E, 0xB8, 0x03,
            ];
            let adv_len = AdStructure::encode_slice(
                &[
                    AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                    AdStructure::ServiceUuids128(&[MIDI_SVC_UUID]),
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
        let mut output = [0i32; N];
        loop {
            // Drain MIDI queue
            while let Some(msg) = MIDI_QUEUE.pop() {
                dispatch_midi(msg, voices, voice_ages, voice_age, &mut channels);
            }

            // Signal core 1 to render voices MAX_VOICES/2..MAX_VOICES
            mc_render::RENDER_DONE.store(false, core::sync::atomic::Ordering::Relaxed);
            mc_render::RENDER_START.store(true, core::sync::atomic::Ordering::Release);

            // Render voices 0..MAX_VOICES/2 on core 0
            output.fill(0);
            for idx in 0..(MAX_VOICES / 2) {
                if !voices[idx].is_finished() {
                    let mut voice_buf = [0i32; N];
                    voices[idx].render(&mut voice_buf);
                    for i in 0..N {
                        output[i] = output[i].saturating_add(voice_buf[i]);
                    }
                }
            }

            // Wait for core 1, yielding to let BLE tasks run
            while !mc_render::RENDER_DONE.load(core::sync::atomic::Ordering::Acquire) {
                embassy_futures::yield_now().await;
            }

            // Combine core 1's output
            #[allow(static_mut_refs)]
            for i in 0..N {
                output[i] = output[i].saturating_add(unsafe { mc_render::CORE1_BUF[i] });
            }

            // Normalize, filter, soft-clip, convert to i16 stereo
            #[cfg(feature = "es8311")]
            {
                // Single voice peaks ~2^26; normalize for MAX_VOICES mix
                const NORM: f32 = 1.0 / (67108864.0 * MAX_VOICES as f32);
                for i in 0..N {
                    let sample_f32 = output[i] as f32 * NORM;
                    let filtered = filter.process(sample_f32);
                    // Soft clip (cubic)
                    let x = filtered;
                    let soft = if x > 1.0 {
                        1.0
                    } else if x < -1.0 {
                        -1.0
                    } else {
                        x * (1.5 - 0.5 * x * x)
                    };
                    let sample = (soft * 32767.0) as i16;
                    i2s_buf[i * 2] = sample;     // L
                    i2s_buf[i * 2 + 1] = sample; // R
                }
                // Push as bytes into circular DMA buffer, waiting for space
                let bytes: &[u8] = unsafe {
                    core::slice::from_raw_parts(
                        i2s_buf.as_ptr() as *const u8,
                        i2s_buf.len() * 2,
                    )
                };
                let mut sent = 0;
                while sent < bytes.len() {
                    let avail = dma_transfer.available().unwrap();
                    if avail > 0 {
                        let chunk = usize::min(avail, bytes.len() - sent);
                        sent += dma_transfer.push(&bytes[sent..sent + chunk]).unwrap();
                    } else {
                        embassy_futures::yield_now().await;
                    }
                }
            }

            embassy_futures::yield_now().await;
        }
    };

    let _ = embassy_futures::join::join3(runner.run(), ble_task, audio_task).await;
    unreachable!()
}

// === USB MIDI mode ===

#[cfg(all(feature = "usb-midi", not(feature = "ble-midi")))]
#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let config = esp_hal::Config::default()
        .with_cpu_clock(CPU_CLOCK);
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 200 * 1024);

    println!("DX7 ESP32-S3 — USB MIDI, {} voices", MAX_VOICES);

    dx7_core::tables::init_tables(SAMPLE_RATE);
    dx7_core::lfo::init_lfo(SAMPLE_RATE);
    dx7_core::pitchenv::init_pitchenv(SAMPLE_RATE);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // Initialize shared voice pool
    #[allow(static_mut_refs)]
    unsafe {
        mc_render::VOICES.write(core::array::from_fn(|_| Voice::new()));
    }

    // Start core 1
    use esp_hal::interrupt::software::SoftwareInterruptControl;
    let sw_ints = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    static mut CORE1_STACK: esp_hal::system::Stack<8192> = esp_hal::system::Stack::new();
    #[allow(static_mut_refs)]
    esp_rtos::start_second_core(
        peripherals.CPU_CTRL,
        sw_ints.software_interrupt0,
        sw_ints.software_interrupt1,
        unsafe { &mut CORE1_STACK },
        mc_render::core1_entry,
    );
    println!("Core 1 started");

    usb_midi_synth().await;
}

#[cfg(all(feature = "usb-midi", not(feature = "ble-midi")))]
async fn usb_midi_synth() -> ! {
    use esp_hal::otg_fs::{Usb, asynch};

    static MIDI_QUEUE: dx7_midi::MidiQueue = dx7_midi::MidiQueue::new();

    // Setup USB MIDI
    let usb_peri = Usb::new(
        unsafe { esp_hal::peripherals::USB0::steal() },
        unsafe { esp_hal::peripherals::GPIO20::steal() },
        unsafe { esp_hal::peripherals::GPIO19::steal() },
    );

    static mut EP_OUT_BUF: [u8; 256] = [0u8; 256];
    #[allow(static_mut_refs)]
    let driver = asynch::Driver::new(usb_peri, unsafe { &mut EP_OUT_BUF }, asynch::Config::default());

    let mut usb_config = embassy_usb::Config::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("DX7");
    usb_config.product = Some("DX7 MIDI Synth");
    usb_config.serial_number = Some("DX7-ESP32S3-001");

    static mut CONFIG_DESC: [u8; 256] = [0u8; 256];
    static mut BOS_DESC: [u8; 256] = [0u8; 256];
    static mut MSOS_DESC: [u8; 256] = [0u8; 256];
    static mut CONTROL_BUF: [u8; 64] = [0u8; 64];

    #[allow(static_mut_refs)]
    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        unsafe { &mut CONFIG_DESC },
        unsafe { &mut BOS_DESC },
        unsafe { &mut MSOS_DESC },
        unsafe { &mut CONTROL_BUF },
    );

    let midi = embassy_usb::class::midi::MidiClass::new(&mut builder, 1, 1, 64);
    let mut usb = builder.build();

    let (_sender, mut receiver) = midi.split();

    println!("USB MIDI ready");

    // Setup I2S + ES8311 audio
    #[cfg(feature = "es8311")]
    let (mut i2s_tx, _pa) = setup_i2s_es8311();

    #[cfg(feature = "es8311")]
    static mut DMA_BUF: [u8; N * 2 * 2 * 4] = [0u8; N * 2 * 2 * 4];
    #[cfg(feature = "es8311")]
    #[allow(static_mut_refs)]
    let mut dma_transfer = i2s_tx.write_dma_circular(unsafe { &mut DMA_BUF }).unwrap();

    // Voice pool
    #[allow(static_mut_refs)]
    let voices = unsafe { mc_render::VOICES.assume_init_mut() };
    #[allow(static_mut_refs)]
    let voice_ages = unsafe { &mut mc_render::VOICE_AGES };
    #[allow(static_mut_refs)]
    let voice_age = unsafe { &mut mc_render::VOICE_AGE };
    let init_patch = load_rom1a_voice(0).unwrap();
    let mut channels: [ChannelState; 16] = core::array::from_fn(|_| ChannelState::new(init_patch.clone()));

    #[cfg(feature = "es8311")]
    let mut i2s_buf = [0i16; N * 2];

    static mut FILTER: OutputFilterF32 = OutputFilterF32 {
        dc1: DcBlockerF32 { r: 0.9993455, x1: 0.0, y1: 0.0 },
        dc2: DcBlockerF32 { r: 0.9993455, x1: 0.0, y1: 0.0 },
        lpf1: BiquadF32 {
            b0: 0.21113742, b1: 0.42227485, b2: 0.21113742,
            a1: -0.20469809, a2: 0.04924778,
            x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0,
        },
        lpf2: BiquadF32 {
            b0: 0.29262414, b1: 0.58524828, b2: 0.29262414,
            a1: -0.28369960, a2: 0.45419615,
            x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0,
        },
    };
    #[allow(static_mut_refs)]
    let filter = unsafe { &mut FILTER };

    // USB device driver task
    let usb_run = usb.run();

    // USB MIDI reader task
    let midi_read = async {
        loop {
            receiver.wait_connection().await;
            let mut buf = [0u8; 64];
            match receiver.read_packet(&mut buf).await {
                Ok(n) => {
                    for chunk in buf[..n].chunks_exact(4) {
                        dx7_midi::usb::parse_usb_midi_event(chunk, &MIDI_QUEUE);
                    }
                }
                Err(_) => continue,
            }
        }
    };

    // Audio render task (same as BLE MIDI version)
    let audio_task = async {
        let mut output = [0i32; N];
        loop {
            while let Some(msg) = MIDI_QUEUE.pop() {
                dispatch_midi(msg, voices, voice_ages, voice_age, &mut channels);
            }

            mc_render::RENDER_DONE.store(false, core::sync::atomic::Ordering::Relaxed);
            mc_render::RENDER_START.store(true, core::sync::atomic::Ordering::Release);

            output.fill(0);
            for idx in 0..(MAX_VOICES / 2) {
                if !voices[idx].is_finished() {
                    let mut voice_buf = [0i32; N];
                    voices[idx].render(&mut voice_buf);
                    for i in 0..N {
                        output[i] = output[i].saturating_add(voice_buf[i]);
                    }
                }
            }

            while !mc_render::RENDER_DONE.load(core::sync::atomic::Ordering::Acquire) {
                embassy_futures::yield_now().await;
            }

            #[allow(static_mut_refs)]
            for i in 0..N {
                output[i] = output[i].saturating_add(unsafe { mc_render::CORE1_BUF[i] });
            }

            #[cfg(feature = "es8311")]
            {
                const NORM: f32 = 1.0 / (67108864.0 * MAX_VOICES as f32);
                for i in 0..N {
                    let sample_f32 = output[i] as f32 * NORM;
                    let filtered = filter.process(sample_f32);
                    let x = filtered;
                    let soft = if x > 1.0 { 1.0 } else if x < -1.0 { -1.0 } else { x * (1.5 - 0.5 * x * x) };
                    let sample = (soft * 32767.0) as i16;
                    i2s_buf[i * 2] = sample;
                    i2s_buf[i * 2 + 1] = sample;
                }
                let bytes: &[u8] = unsafe {
                    core::slice::from_raw_parts(i2s_buf.as_ptr() as *const u8, i2s_buf.len() * 2)
                };
                let mut sent = 0;
                while sent < bytes.len() {
                    let avail = dma_transfer.available().unwrap();
                    if avail > 0 {
                        let chunk = usize::min(avail, bytes.len() - sent);
                        sent += dma_transfer.push(&bytes[sent..sent + chunk]).unwrap();
                    } else {
                        embassy_futures::yield_now().await;
                    }
                }
            }

            embassy_futures::yield_now().await;
        }
    };

    let _ = embassy_futures::join::join3(usb_run, midi_read, audio_task).await;
    unreachable!()
}

// === I2S + ES8311 setup ===

#[cfg(all(any(feature = "ble-midi", feature = "usb-midi"), feature = "es8311"))]
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

// === Non-BLE/USB modes: sync entry point ===
#[cfg(not(any(feature = "ble-midi", feature = "usb-midi")))]
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

/// Play audio through I2S + ES8311 codec (demo mode).
#[cfg(all(feature = "es8311", not(any(feature = "ble-midi", feature = "usb-midi"))))]
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
            .with_channels(Channels::MONO),
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
