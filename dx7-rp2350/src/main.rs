#![no_std]
#![no_main]

extern crate alloc;

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_hal_internal::Peri;
use dx7_core::voice::Voice;
use dx7_core::load_rom1a_voice;
use dx7_core::tables::N;
use core::sync::atomic::{AtomicU8, Ordering};

const MAX_VOICES: usize = 10;
const NUM_CHANNELS: usize = 16;

/// 0 = DX7 monotimbral (power-up default), 1 = GM multitimbral
static SYNTH_MODE: AtomicU8 = AtomicU8::new(0);
/// MIDI receive channel for DX7 mode: 0x00-0x0F = ch 1-16, 0x7F = OMNI
static MIDI_CHANNEL: AtomicU8 = AtomicU8::new(0x7F);

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

const SAMPLE_RATE: u32 = 48000;
const CPU_HZ: u32 = 200_000_000;
#[cfg(feature = "pwm")]
const CYCLES_PER_SAMPLE: u32 = CPU_HZ / SAMPLE_RATE;

// --- Heap allocator ---
use embedded_alloc::LlffHeap as Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

fn init_heap() {
    const HEAP_SIZE: usize = 16 * 1024;
    static mut HEAP_MEM: [u8; HEAP_SIZE] = [0u8; HEAP_SIZE];
    #[allow(static_mut_refs)]
    unsafe {
        HEAP.init(HEAP_MEM.as_mut_ptr() as usize, HEAP_SIZE);
    }
}

// --- f32 output filters (DC blocker + 4th-order Butterworth LPF) ---

/// Biquad (2nd-order IIR) filter using f32 single-precision FPU.
/// Direct Form I: y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]
struct BiquadF32 {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    x1: f32, x2: f32, // input delay
    y1: f32, y2: f32, // output delay
}

impl BiquadF32 {
    const fn new(b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> Self {
        Self { b0, b1, b2, a1, a2, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

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

/// DC-blocking high-pass filter (1st-order, ~5Hz cutoff at 48kHz).
/// y[n] = x[n] - x[n-1] + r * y[n-1], where r ≈ 0.9993455
struct DcBlockerF32 {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlockerF32 {
    const fn new(r: f32) -> Self {
        Self { r, x1: 0.0, y1: 0.0 }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Output filter chain: DC blocker → 4th-order Butterworth LPF at 10.5kHz.
/// Removes DC offset from asymmetric FM and aliases above Nyquist/2.
struct OutputFilterF32 {
    dc1: DcBlockerF32,
    dc2: DcBlockerF32,
    lpf1: BiquadF32, // Stage 1 (Q=0.5412)
    lpf2: BiquadF32, // Stage 2 (Q=1.3066)
}

impl OutputFilterF32 {
    fn new() -> Self {
        // Coefficients for 4th-order Butterworth LPF at 10500 Hz / 48000 Hz
        Self {
            dc1: DcBlockerF32::new(0.9993455),
            dc2: DcBlockerF32::new(0.9993455),
            // Stage 1 (Q=0.5412)
            lpf1: BiquadF32::new(
                0.21113742, 0.42227485, 0.21113742,
                -0.20469809, 0.04924778,
            ),
            // Stage 2 (Q=1.3066)
            lpf2: BiquadF32::new(
                0.29262414, 0.58524828, 0.29262414,
                -0.28369960, 0.45419615,
            ),
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let x = self.dc1.process(x);
        let x = self.dc2.process(x);
        let x = self.lpf1.process(x);
        self.lpf2.process(x)
    }
}

// --- Cortex-M33 DSP intrinsics ---

/// Saturating 32-bit signed add (QADD instruction, 1 cycle).
/// Clamps result to [i32::MIN, i32::MAX] instead of wrapping.
#[inline(always)]
fn qadd(a: i32, b: i32) -> i32 {
    let result: i32;
    unsafe {
        core::arch::asm!(
            "qadd {out}, {a}, {b}",
            a = in(reg) a,
            b = in(reg) b,
            out = lateout(reg) result,
        );
    }
    result
}

/// Unsigned saturate to N bits (USAT instruction, 1 cycle).
/// Clamps signed i32 to [0, 2^N - 1].
#[inline(always)]
fn usat<const N: u32>(val: i32) -> u32 {
    let result: u32;
    unsafe {
        core::arch::asm!(
            "usat {out}, #{n}, {val}",
            val = in(reg) val,
            n = const N,
            out = lateout(reg) result,
        );
    }
    result
}

// --- DWT cycle counter (Cortex-M33) ---

fn enable_cycle_counter() {
    unsafe {
        // DEMCR: enable trace (bit 24 = TRCENA)
        let demcr = 0xE000_EDFC as *mut u32;
        core::ptr::write_volatile(demcr, core::ptr::read_volatile(demcr) | (1 << 24));
        // DWT CTRL: enable cycle counter (bit 0 = CYCCNTENA)
        let dwt_ctrl = 0xE000_1000 as *mut u32;
        core::ptr::write_volatile(dwt_ctrl, core::ptr::read_volatile(dwt_ctrl) | 1);
    }
}

#[inline(always)]
fn read_cycles() -> u32 {
    unsafe { core::ptr::read_volatile(0xE000_1004 as *const u32) }
}

// --- DMA-driven PWM audio output (ping-pong, zero CPU) ---

#[cfg(feature = "pwm")]
mod dma_audio {
    use core::sync::atomic::{AtomicU8, Ordering};
    use dx7_core::tables::N;

    /// Ping-pong DMA buffers. Each entry = (duty << 16) for PWM CC channel B.
    static mut BUF_A: [u32; N] = [512 << 16; N];
    static mut BUF_B: [u32; N] = [512 << 16; N];

    // DMA register addresses (RP2350)
    const DMA_BASE: u32 = 0x5000_0000;
    // CH0 registers
    const CH0_READ_ADDR:   *mut u32 = (DMA_BASE + 0x000) as *mut u32;
    const CH0_WRITE_ADDR:  *mut u32 = (DMA_BASE + 0x004) as *mut u32;
    const CH0_TRANS_COUNT: *mut u32 = (DMA_BASE + 0x008) as *mut u32;
    const CH0_CTRL_TRIG:   *mut u32 = (DMA_BASE + 0x00C) as *mut u32;
    // CH1 registers (stride = 0x40)
    const CH1_READ_ADDR:   *mut u32 = (DMA_BASE + 0x040) as *mut u32;
    const CH1_WRITE_ADDR:  *mut u32 = (DMA_BASE + 0x044) as *mut u32;
    const CH1_TRANS_COUNT: *mut u32 = (DMA_BASE + 0x048) as *mut u32;
    const CH1_CTRL_TRIG:   *mut u32 = (DMA_BASE + 0x04C) as *mut u32;
    const CH1_AL1_CTRL:    *mut u32 = (DMA_BASE + 0x050) as *mut u32;
    // Interrupt, abort, and timer registers (RP2350: 4 IRQ sets shift timer to 0x440)
    const DMA_INTR:      *mut u32 = (DMA_BASE + 0x400) as *mut u32;
    const DMA_INTE0:     *mut u32 = (DMA_BASE + 0x404) as *mut u32;
    const DMA_TIMER0:    *mut u32 = (DMA_BASE + 0x440) as *mut u32;
    const DMA_CHAN_ABORT: *mut u32 = (DMA_BASE + 0x474) as *mut u32;

    /// PWM slice 7 CC register (channel B in upper 16 bits).
    const PWM_CC7: u32 = 0x400A_8000 + 7 * 0x14 + 0x0C;

    // CTRL register (RP2350): EN(0), DATA_SIZE(3:2), INCR_READ(4),
    //   CHAIN_TO(16:13), TREQ_SEL(22:17)
    const CTRL_BASE: u32 = (1 << 0)       // EN
                         | (2 << 2)        // DATA_SIZE = word (32-bit)
                         | (1 << 4);       // INCR_READ
    const TREQ_DMA_TIMER0: u32 = 0x3B << 17;
    const CH0_CTRL_VAL: u32 = CTRL_BASE | (1 << 13) | TREQ_DMA_TIMER0; // CHAIN_TO = CH1
    const CH1_CTRL_VAL: u32 = CTRL_BASE | (0 << 13) | TREQ_DMA_TIMER0; // CHAIN_TO = CH0

    /// Which buffer is available for filling (0=A, 1=B, 2=none yet).
    static FILL_WHICH: AtomicU8 = AtomicU8::new(2);

    /// Initialize DMA ping-pong for PWM audio output at 48kHz.
    /// Both buffers start with silence (duty=512). CH0 starts immediately.
    #[allow(static_mut_refs)]
    pub unsafe fn init() {
        // First: disable INTE0 bits 0-1 so embassy's DMA IRQ handler ignores CH0/CH1.
        // Embassy init sets INTE0=0xFFFF — we must clear our bits before any DMA activity.
        let inte = core::ptr::read_volatile(DMA_INTE0);
        core::ptr::write_volatile(DMA_INTE0, inte & !0x03);

        // Abort any pre-existing activity on CH0/CH1
        core::ptr::write_volatile(DMA_CHAN_ABORT, 0x03);
        // Wait for abort to complete (BUSY bits clear)
        while core::ptr::read_volatile(CH0_CTRL_TRIG) & (1 << 26) != 0 {}
        while core::ptr::read_volatile(CH1_CTRL_TRIG) & (1 << 26) != 0 {}

        // Clear any pending raw interrupts for CH0/CH1
        core::ptr::write_volatile(DMA_INTR, 0x03);

        // DMA pace timer 0: 200MHz × 3/12500 = 48000 Hz
        core::ptr::write_volatile(DMA_TIMER0, (3 << 16) | 12500);

        // CH0: reads BUF_A → writes PWM_CC7, N transfers, chains to CH1
        core::ptr::write_volatile(CH0_READ_ADDR, BUF_A.as_ptr() as u32);
        core::ptr::write_volatile(CH0_WRITE_ADDR, PWM_CC7);
        core::ptr::write_volatile(CH0_TRANS_COUNT, N as u32);

        // CH1: reads BUF_B → writes PWM_CC7, N transfers, chains to CH0
        core::ptr::write_volatile(CH1_READ_ADDR, BUF_B.as_ptr() as u32);
        core::ptr::write_volatile(CH1_WRITE_ADDR, PWM_CC7);
        core::ptr::write_volatile(CH1_TRANS_COUNT, N as u32);

        // Arm CH1 (non-triggering write — waits for chain from CH0)
        core::ptr::write_volatile(CH1_AL1_CTRL, CH1_CTRL_VAL);

        FILL_WHICH.store(2, Ordering::Release);

        // Start CH0 (writing CTRL_TRIG triggers the first transfer)
        core::ptr::write_volatile(CH0_CTRL_TRIG, CH0_CTRL_VAL);
    }

    /// Poll DMA completion. Returns true when a buffer is ready for refilling.
    /// IMPORTANT: Immediately reloads the completed channel's READ_ADDR and
    /// TRANS_COUNT to prevent runaway chain (TRANS_COUNT=0 → instant completion).
    /// If the fill is too slow, the channel replays old data instead of crashing.
    #[inline]
    #[allow(static_mut_refs)]
    pub fn poll_and_check() -> bool {
        let intr = unsafe { core::ptr::read_volatile(DMA_INTR) };
        if intr & 0x01 != 0 {
            // CH0 finished playing BUF_A → immediately reload CH0 so chain is safe
            unsafe {
                core::ptr::write_volatile(DMA_INTR, 0x01);
                core::ptr::write_volatile(CH0_READ_ADDR, BUF_A.as_ptr() as u32);
                core::ptr::write_volatile(CH0_TRANS_COUNT, N as u32);
            }
            FILL_WHICH.store(0, Ordering::Release);
            true
        } else if intr & 0x02 != 0 {
            // CH1 finished playing BUF_B → immediately reload CH1 so chain is safe
            unsafe {
                core::ptr::write_volatile(DMA_INTR, 0x02);
                core::ptr::write_volatile(CH1_READ_ADDR, BUF_B.as_ptr() as u32);
                core::ptr::write_volatile(CH1_TRANS_COUNT, N as u32);
            }
            FILL_WHICH.store(1, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Get the buffer that needs filling. Only valid after poll_and_check() returns true.
    #[allow(static_mut_refs)]
    pub unsafe fn get_fill_buffer() -> &'static mut [u32; N] {
        if FILL_WHICH.load(Ordering::Acquire) == 0 { &mut BUF_A } else { &mut BUF_B }
    }

    /// Signal that the fill buffer has been written.
    pub fn buffer_filled() {
        FILL_WHICH.store(2, Ordering::Release);
    }
}

// --- Multi-core voice rendering (DMA handles PWM output) ---

#[cfg(all(any(feature = "usb-midi", feature = "ble-midi"), feature = "pwm"))]
mod mc_render {
    use core::sync::atomic::{AtomicBool, Ordering};
    use dx7_core::voice::Voice;
    use dx7_core::tables::N;
    use super::MAX_VOICES;

    /// Shared voice pool. Core 0 handles all note_on/note_off (when core 1 is idle).
    /// During render: core 0 renders voices 0..2, core 1 renders voices 3..5.
    pub static mut VOICES: core::mem::MaybeUninit<[Voice; MAX_VOICES]> =
        core::mem::MaybeUninit::uninit();
    pub static mut VOICE_AGES: [u32; MAX_VOICES] = [0; MAX_VOICES];
    pub static mut VOICE_AGE: u32 = 0;

    /// Core 1 render output buffer (written by core 1, read by core 0 after RENDER_DONE).
    pub static mut CORE1_BUF: [i32; N] = [0i32; N];

    /// Core 0 sets RENDER_START=true to signal core 1 to begin rendering.
    pub static RENDER_START: AtomicBool = AtomicBool::new(false);
    /// Core 1 sets RENDER_DONE=true when finished. Core 0 checks before combining.
    pub static RENDER_DONE: AtomicBool = AtomicBool::new(true);

    /// Peak render cycles since last CPU meter read (reset-on-read).
    pub static RENDER_CYCLES_PEAK: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);

    /// Core 1 entry: render voices MAX_VOICES/2..MAX_VOICES on demand.
    /// DMA handles PWM output — core 1 only renders voices.
    #[allow(static_mut_refs)]
    pub unsafe fn core1_entry() -> ! {
        let voices = VOICES.assume_init_mut();

        loop {
            while !RENDER_START.load(Ordering::Acquire) {
                cortex_m::asm::nop();
            }
            RENDER_START.store(false, Ordering::Relaxed);

            // Render voices MAX_VOICES/2..MAX_VOICES into CORE1_BUF
            CORE1_BUF.fill(0);
            for idx in (MAX_VOICES / 2)..MAX_VOICES {
                if !voices[idx].is_finished() {
                    let mut buf = [0i32; N];
                    voices[idx].render(&mut buf);
                    for j in 0..N {
                        CORE1_BUF[j] = super::qadd(CORE1_BUF[j], buf[j]);
                    }
                }
            }

            RENDER_DONE.store(true, Ordering::Release);
        }
    }
}

// --- CYW43 firmware embedding (BLE MIDI) ---

#[cfg(feature = "ble-midi")]
mod cyw43_fw {
    #[repr(C, align(4))]
    pub struct Aligned<const N: usize>(pub [u8; N]);

    pub static FW: &[u8] = include_bytes!("../cyw43-firmware/43439A0.bin");
    pub static BTFW: &[u8] = include_bytes!("../cyw43-firmware/43439A0_btfw.bin");
    pub static CLM: &[u8] = include_bytes!("../cyw43-firmware/43439A0_clm.bin");
}

// --- Voice dispatch (shared by all entry points) ---

#[cfg(all(any(feature = "usb-midi", feature = "ble-midi"), feature = "pwm"))]
fn dispatch_midi(
    msg: dx7_midi::MidiMessage,
    voices: &mut [dx7_core::voice::Voice],
    voice_ages: &mut [u32],
    voice_age: &mut u32,
    channels: &mut [ChannelState; NUM_CHANNELS],
    sysex_bank: Option<&[u8; 4096]>,
    synth_mode: u8,
) {
    use dx7_core::voice::VoiceState;

    // DX7 mode: channel filter
    if synth_mode == 0 {
        let midi_ch = MIDI_CHANNEL.load(Ordering::Acquire);
        if midi_ch != 0x7F {
            // Not OMNI — check channel
            let msg_ch = match &msg {
                dx7_midi::MidiMessage::NoteOn { channel, .. } => *channel,
                dx7_midi::MidiMessage::NoteOff { channel, .. } => *channel,
                dx7_midi::MidiMessage::PitchBend { channel, .. } => *channel,
                dx7_midi::MidiMessage::ControlChange { channel, .. } => *channel,
                dx7_midi::MidiMessage::ProgramChange { channel, .. } => *channel,
                _ => return,
            };
            if msg_ch != midi_ch {
                return;
            }
        }
    }

    match msg {
        dx7_midi::MidiMessage::NoteOn { channel, note, velocity } => {
            if synth_mode == 0 {
                // DX7 mode: match by note only (ignore channel for dedup)
                for v in voices.iter_mut() {
                    if v.note == note && !v.is_finished() {
                        v.state = VoiceState::Inactive;
                    }
                }
            } else {
                // GM mode: match by (note, channel)
                for v in voices.iter_mut() {
                    if v.note == note && v.channel == channel && !v.is_finished() {
                        v.state = VoiceState::Inactive;
                    }
                }
            }
            *voice_age += 1;
            let n = voices.len();
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
                    (0..n).min_by_key(|&i| voice_ages[i]).unwrap()
                });
            voices[slot].state = VoiceState::Inactive;
            let patch;
            if synth_mode == 0 {
                // DX7 mode: always use channels[0].patch, no drum mapping
                patch = channels[0].patch.clone();
            } else if channel == 9 {
                // GM mode: channel 9 = drums
                if let Some(dp) = dx7_midi::drum::drum_voice(note) {
                    patch = dp;
                } else {
                    return;
                }
            } else {
                patch = channels[channel as usize].patch.clone();
            }
            voices[slot].note_on(&patch, note, velocity);
            voices[slot].channel = channel;
            voices[slot].sustain_held = false;
            if synth_mode == 0 {
                // DX7 mode: use global (ch 0) bend/mod
                voices[slot].pitch_bend = channels[0].pitch_bend;
                voices[slot].mod_wheel = channels[0].mod_wheel;
            } else {
                voices[slot].pitch_bend = channels[channel as usize].pitch_bend;
                voices[slot].mod_wheel = channels[channel as usize].mod_wheel;
            }
            voice_ages[slot] = *voice_age;
        }
        dx7_midi::MidiMessage::NoteOff { channel, note, .. } => {
            if synth_mode == 0 {
                // DX7 mode: match by note only
                for v in voices.iter_mut() {
                    if v.note == note && !v.is_finished() {
                        if channels[0].sustain {
                            v.sustain_held = true;
                        } else {
                            v.note_off();
                        }
                    }
                }
            } else {
                let ch = channel as usize;
                for v in voices.iter_mut() {
                    if v.note == note && v.channel == channel && !v.is_finished() {
                        if channels[ch].sustain {
                            v.sustain_held = true;
                        } else {
                            v.note_off();
                        }
                    }
                }
            }
        }
        dx7_midi::MidiMessage::PitchBend { channel, value } => {
            let bend_signed = value as i32 - 8192;
            let bend_range_semitones = 12;
            let bend = (bend_signed * bend_range_semitones * 256) / 8192;
            if synth_mode == 0 {
                // DX7 mode: apply globally to ALL active voices
                channels[0].pitch_bend = bend;
                for v in voices.iter_mut() {
                    if !v.is_finished() {
                        v.pitch_bend = bend;
                    }
                }
            } else {
                let ch = channel as usize;
                channels[ch].pitch_bend = bend;
                for v in voices.iter_mut() {
                    if v.channel == channel && !v.is_finished() {
                        v.pitch_bend = bend;
                    }
                }
            }
        }
        dx7_midi::MidiMessage::ControlChange { channel, controller, value } => {
            match controller {
                1 => {
                    let mw = value as i32;
                    if synth_mode == 0 {
                        // DX7 mode: apply globally
                        channels[0].mod_wheel = mw;
                        for v in voices.iter_mut() {
                            if !v.is_finished() {
                                v.mod_wheel = mw;
                            }
                        }
                    } else {
                        let ch = channel as usize;
                        channels[ch].mod_wheel = mw;
                        for v in voices.iter_mut() {
                            if v.channel == channel && !v.is_finished() {
                                v.mod_wheel = mw;
                            }
                        }
                    }
                }
                64 => {
                    let on = value >= 64;
                    if synth_mode == 0 {
                        // DX7 mode: global sustain
                        channels[0].sustain = on;
                        if !on {
                            for v in voices.iter_mut() {
                                if v.sustain_held {
                                    v.sustain_held = false;
                                    v.note_off();
                                }
                            }
                        }
                    } else {
                        let ch = channel as usize;
                        channels[ch].sustain = on;
                        if !on {
                            for v in voices.iter_mut() {
                                if v.channel == channel && v.sustain_held {
                                    v.sustain_held = false;
                                    v.note_off();
                                }
                            }
                        }
                    }
                }
                120 | 123 => {
                    if synth_mode == 0 {
                        // DX7 mode: kill ALL voices
                        for v in voices.iter_mut() {
                            if !v.is_finished() {
                                v.note_off();
                            }
                        }
                    } else {
                        for v in voices.iter_mut() {
                            if v.channel == channel && !v.is_finished() {
                                v.note_off();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        dx7_midi::MidiMessage::ProgramChange { channel, program } => {
            if synth_mode == 0 {
                // DX7 mode: update channels[0].patch (global)
                if let Some(bank) = sysex_bank {
                    let idx = program as usize;
                    if idx < 32 {
                        let start = idx * 128;
                        let mut voice_data = [0u8; 128];
                        voice_data.copy_from_slice(&bank[start..start + 128]);
                        channels[0].patch = dx7_core::DxVoice::from_packed(&voice_data);
                        defmt::info!("DX7 sysex patch {}", idx);
                    }
                } else if let Some(p) = dx7_core::load_rom1a_voice(program as usize) {
                    channels[0].patch = p;
                    defmt::info!("DX7 ROM1A patch {}", program);
                } else {
                    defmt::info!("DX7 patch {} out of range", program);
                }
            } else {
                // GM mode: per-channel patch
                let ch = channel as usize;
                if let Some(bank) = sysex_bank {
                    let idx = program as usize;
                    if idx < 32 {
                        let start = idx * 128;
                        let mut voice_data = [0u8; 128];
                        voice_data.copy_from_slice(&bank[start..start + 128]);
                        channels[ch].patch = dx7_core::DxVoice::from_packed(&voice_data);
                        defmt::info!("Ch{} loaded sysex patch {}", ch, idx);
                    }
                } else {
                    channels[ch].patch = dx7_midi::gm::gm_voice(program);
                    defmt::info!("Ch{} GM program {}", ch, program);
                }
            }
        }
        _ => {}
    }
}

// --- Interrupt bindings ---

#[cfg(all(feature = "usb-midi", not(feature = "ble-midi")))]
embassy_rp::bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<embassy_rp::peripherals::USB>;
});

#[cfg(all(feature = "ble-midi", not(feature = "usb-midi")))]
embassy_rp::bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<embassy_rp::peripherals::PIO0>;
});

#[cfg(all(feature = "ble-midi", feature = "usb-midi"))]
embassy_rp::bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<embassy_rp::peripherals::PIO0>;
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<embassy_rp::peripherals::USB>;
});

// --- Entry point ---

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Overclock to 200 MHz for audio headroom
    // PLL: 12MHz * 100 / 6 / 1 = 200MHz (VCO=1200MHz, within 750-1600 range)
    let mut config = embassy_rp::config::Config::default();
    {
        let pll = config.clocks.xosc.as_mut().unwrap().sys_pll.as_mut().unwrap();
        pll.fbdiv = 100;
        pll.post_div1 = 6;
        pll.post_div2 = 1;
    }
    let p = embassy_rp::init(config);
    init_heap();
    enable_cycle_counter();

    info!("DX7 Raspberry Pi Pico 2 W");

    dx7_core::tables::init_tables(SAMPLE_RATE);
    dx7_core::lfo::init_lfo(SAMPLE_RATE);
    dx7_core::pitchenv::init_pitchenv(SAMPLE_RATE);

    // === Benchmark ===
    let patch = load_rom1a_voice(0).unwrap();
    let mut voice = Voice::new();
    voice.note_on(&patch, 36, 100);
    let mut output = [0i32; N];

    let start = read_cycles();
    voice.render(&mut output);
    let end = read_cycles();

    let total_cycles = end.wrapping_sub(start);
    let us_per_block = total_cycles / (CPU_HZ / 1_000_000);
    let deadline_us = (N as u32 * 1_000_000) / SAMPLE_RATE;
    let status = if us_per_block < deadline_us { "OK" } else { "OVER" };
    info!("1v: {} cyc/blk  {} us/blk  {}", total_cycles, us_per_block, status);

    // === Feature-gated playback ===

    // USB + BLE MIDI synth (both active simultaneously)
    #[cfg(all(feature = "usb-midi", feature = "ble-midi", feature = "pwm"))]
    usb_ble_midi_pwm_synth(
        p.USB,
        p.PIO0, p.PIN_23, p.PIN_24, p.PIN_25, p.PIN_29,
        p.DMA_CH2, p.PWM_SLICE7, p.PIN_15, p.CORE1,
    ).await;

    // BLE MIDI only
    #[cfg(all(feature = "ble-midi", not(feature = "usb-midi"), feature = "pwm"))]
    ble_midi_pwm_synth(
        p.PIO0, p.PIN_23, p.PIN_24, p.PIN_25, p.PIN_29,
        p.DMA_CH2, p.PWM_SLICE7, p.PIN_15, p.CORE1,
    ).await;

    // USB MIDI only
    #[cfg(all(feature = "usb-midi", not(feature = "ble-midi"), feature = "pwm"))]
    usb_midi_pwm_synth(p.USB, p.PWM_SLICE7, p.PIN_15, p.CORE1).await;

    #[cfg(not(any(
        all(feature = "usb-midi", feature = "ble-midi", feature = "pwm"),
        all(feature = "ble-midi", not(feature = "usb-midi"), feature = "pwm"),
        all(feature = "usb-midi", not(feature = "ble-midi"), feature = "pwm"),
    )))]
    {
        #[cfg(all(feature = "pwm", not(feature = "usb-midi"), not(feature = "ble-midi")))]
        pwm_demo(&patch, p.PWM_SLICE7, p.PIN_15, p.CORE1);

        #[cfg(not(feature = "pwm"))]
        let _ = p;

        info!("Done.");
        loop {
            embassy_time::Timer::after_secs(1).await;
        }
    }
}

// === PWM demo playback (hardcoded note, single-core with DMA) ===

#[cfg(all(feature = "pwm", not(feature = "usb-midi"), not(feature = "ble-midi")))]
fn pwm_demo(
    patch: &dx7_core::DxVoice,
    slice: Peri<'static, embassy_rp::peripherals::PWM_SLICE7>,
    pin: Peri<'static, embassy_rp::peripherals::PIN_15>,
    _core1: Peri<'static, embassy_rp::peripherals::CORE1>,
) {
    use embassy_rp::pwm::{Config as PwmConfig, Pwm};

    info!("PWM audio on GP15 (DMA output, 10-bit, ~195 kHz carrier)");

    let mut config = PwmConfig::default();
    config.top = 1023;
    config.compare_b = 512;
    let _pwm = Pwm::new_output_b(slice, pin, config);

    // Start DMA ping-pong (both buffers initialized to silence)
    unsafe { dma_audio::init(); }

    let mut voice = Voice::new();
    voice.note_on(patch, 60, 100);
    let note_blocks = (SAMPLE_RATE as usize * 2) / N;
    let mut output = [0i32; N];

    info!("Playing {} blocks...", note_blocks);
    for block in 0..note_blocks {
        voice.render(&mut output);
        if block == note_blocks / 2 {
            voice.note_off();
        }
        // Wait for DMA buffer available, then fill it
        while !dma_audio::poll_and_check() {
            cortex_m::asm::nop();
        }
        unsafe {
            let buf = dma_audio::get_fill_buffer();
            for i in 0..N {
                let duty = (output[i] >> 17) + 512;
                buf[i] = (usat::<10>(duty) as u32) << 16;
            }
            dma_audio::buffer_filled();
        }
    }
    // Flush: fill remaining buffers with silence
    for _ in 0..2 {
        while !dma_audio::poll_and_check() {
            cortex_m::asm::nop();
        }
        unsafe {
            let buf = dma_audio::get_fill_buffer();
            buf.fill(512 << 16);
            dma_audio::buffer_filled();
        }
    }
    info!("Playback done.");
}

// === Core 1 stack (used by USB/BLE MIDI synth for parallel voice rendering) ===

#[cfg(all(any(feature = "usb-midi", feature = "ble-midi"), feature = "pwm"))]
static mut CORE1_STACK: embassy_rp::multicore::Stack<4096> = embassy_rp::multicore::Stack::new();

// === USB MIDI + PWM live synth (dual-core) ===

#[cfg(all(feature = "usb-midi", not(feature = "ble-midi"), feature = "pwm"))]
async fn usb_midi_pwm_synth(
    usb_peripheral: Peri<'static, embassy_rp::peripherals::USB>,
    pwm_slice: Peri<'static, embassy_rp::peripherals::PWM_SLICE7>,
    pwm_pin: Peri<'static, embassy_rp::peripherals::PIN_15>,
    core1: Peri<'static, embassy_rp::peripherals::CORE1>,
) -> ! {
    use embassy_rp::pwm::{Config as PwmConfig, Pwm};
    use embassy_rp::usb::Driver;

    info!("USB MIDI synth with PWM output on GP15 (dual-core render)");

    // Setup PWM on core 0 (10-bit: 200MHz/1024 ≈ 195kHz carrier)
    let mut pwm_config = PwmConfig::default();
    pwm_config.top = 1023;
    pwm_config.compare_b = 512;
    let _pwm = Pwm::new_output_b(pwm_slice, pwm_pin, pwm_config);

    // Initialize shared voice pool
    #[allow(static_mut_refs)]
    unsafe {
        mc_render::VOICES.write(core::array::from_fn(|_| Voice::new()));
    }

    // Start core 1 for parallel voice rendering (DMA handles PWM output)
    #[allow(static_mut_refs)]
    unsafe {
        embassy_rp::multicore::spawn_core1(core1, &mut CORE1_STACK, || -> ! {
            mc_render::core1_entry()
        });
    }

    // Setup USB
    let driver = Driver::new(usb_peripheral, Irqs);
    let mut usb_config = embassy_usb::Config::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("DX7");
    usb_config.product = Some("DX7 MIDI Synth");
    usb_config.serial_number = Some("DX7-RPI-001");

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut msos_descriptor = [0u8; 256];
    let mut control_buf = [0u8; 64];

    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );

    let midi = embassy_usb::class::midi::MidiClass::new(&mut builder, 1, 1, 64);
    let mut usb = builder.build();

    let (mut sender, mut receiver) = midi.split();

    static MIDI_QUEUE: dx7_midi::MidiQueue = dx7_midi::MidiQueue::new();

    // SysEx reception buffer (max 4104 bytes for DX7 32-voice bulk dump)
    static mut SYSEX_RX_BUF: [u8; 4200] = [0u8; 4200];
    static mut SYSEX_RX_POS: usize = 0;
    static mut SYSEX_RX_ACTIVE: bool = false;

    // SysEx voice bank storage (32 packed voices × 128 bytes)
    static mut SYSEX_BANK: [u8; 4096] = [0u8; 4096];
    static SYSEX_BANK_LOADED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);

    // Task 1: USB device driver
    let usb_run = usb.run();

    // Task: CPU meter — sends CC 119 via USB MIDI every 500ms (async, never blocks render)
    let cpu_meter = async {
        loop {
            embassy_time::Timer::after_millis(500).await;
            let cycles = mc_render::RENDER_CYCLES_PEAK.swap(0, core::sync::atomic::Ordering::Relaxed);
            let pct = ((cycles as u64 * 127) / 266667).min(127) as u8;
            let packet = [0x0B, 0xB0, 119, pct];
            let _ = sender.write_packet(&packet).await;
        }
    };

    // Task 2: Read USB MIDI packets (with SysEx accumulation)
    let midi_read = async {
        loop {
            receiver.wait_connection().await;
            let mut buf = [0u8; 64];
            match receiver.read_packet(&mut buf).await {
                Ok(n) => {
                    for chunk in buf[..n].chunks_exact(4) {
                        let cin = chunk[0] & 0x0F;
                        match cin {
                            0x04 => {
                                // SysEx start or continue — 3 data bytes
                                #[allow(static_mut_refs)]
                                unsafe {
                                    if chunk[1] == 0xF0 {
                                        SYSEX_RX_POS = 0;
                                        SYSEX_RX_ACTIVE = true;
                                    }
                                    if SYSEX_RX_ACTIVE {
                                        for &b in &chunk[1..4] {
                                            if SYSEX_RX_POS < SYSEX_RX_BUF.len() {
                                                SYSEX_RX_BUF[SYSEX_RX_POS] = b;
                                                SYSEX_RX_POS += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            0x05 | 0x06 | 0x07 => {
                                // SysEx end: 1, 2, or 3 final bytes
                                #[allow(static_mut_refs)]
                                unsafe {
                                    if SYSEX_RX_ACTIVE {
                                        let count = (cin - 0x04) as usize; // 1, 2, or 3
                                        for &b in &chunk[1..1 + count] {
                                            if SYSEX_RX_POS < SYSEX_RX_BUF.len() {
                                                SYSEX_RX_BUF[SYSEX_RX_POS] = b;
                                                SYSEX_RX_POS += 1;
                                            }
                                        }
                                        // Process complete SysEx
                                        let len = SYSEX_RX_POS;
                                        if len == 5
                                            && SYSEX_RX_BUF[0] == 0xF0
                                            && SYSEX_RX_BUF[1] == 0x7D
                                            && SYSEX_RX_BUF[4] == 0xF7
                                        {
                                            match SYSEX_RX_BUF[2] {
                                                0x01 => {
                                                    // Mode switch: 0=DX7, 1=GM
                                                    let mode = SYSEX_RX_BUF[3];
                                                    SYNTH_MODE.store(mode, Ordering::Release);
                                                    info!("Mode: {}", if mode == 0 { "DX7" } else { "GM" });
                                                    // Kill all voices on mode switch
                                                    for msg_ch in 0..16u8 {
                                                        MIDI_QUEUE.push(dx7_midi::MidiMessage::ControlChange {
                                                            channel: msg_ch, controller: 123, value: 0,
                                                        });
                                                    }
                                                }
                                                0x02 => {
                                                    // Set MIDI channel (DX7 mode)
                                                    let ch = SYSEX_RX_BUF[3];
                                                    MIDI_CHANNEL.store(ch, Ordering::Release);
                                                    if ch == 0x7F {
                                                        info!("MIDI channel: OMNI");
                                                    } else {
                                                        info!("MIDI channel: {}", ch + 1);
                                                    }
                                                }
                                                _ => {
                                                    info!("SysEx: unknown cmd 0x{:02X}", SYSEX_RX_BUF[2]);
                                                }
                                            }
                                        } else if len == 4104
                                            && SYSEX_RX_BUF[0] == 0xF0
                                            && SYSEX_RX_BUF[1] == 0x43
                                            && (SYSEX_RX_BUF[2] & 0xF0) == 0x00
                                            && SYSEX_RX_BUF[3] == 0x09
                                            && SYSEX_RX_BUF[4] == 0x20
                                            && SYSEX_RX_BUF[5] == 0x00
                                            && SYSEX_RX_BUF[4103] == 0xF7
                                        {
                                            // DX7 32-voice bulk dump — verify checksum
                                            let sum: u8 = SYSEX_RX_BUF[6..4102]
                                                .iter()
                                                .fold(0u8, |acc, &b| acc.wrapping_add(b));
                                            let expected = (!sum).wrapping_add(1) & 0x7F;
                                            if expected == SYSEX_RX_BUF[4102] {
                                                SYSEX_BANK.copy_from_slice(
                                                    &SYSEX_RX_BUF[6..4102],
                                                );
                                                SYSEX_BANK_LOADED.store(
                                                    true,
                                                    core::sync::atomic::Ordering::Release,
                                                );
                                                info!("SysEx: loaded 32-voice bank");
                                            } else {
                                                info!(
                                                    "SysEx: checksum mismatch ({} vs {})",
                                                    expected, SYSEX_RX_BUF[4102]
                                                );
                                            }
                                        } else if len > 0 {
                                            info!("SysEx: ignored ({} bytes)", len);
                                        }
                                        SYSEX_RX_ACTIVE = false;
                                    }
                                }
                            }
                            _ => {
                                // Regular MIDI message
                                dx7_midi::usb::parse_usb_midi_event(chunk, &MIDI_QUEUE);
                            }
                        }
                    }
                }
                Err(_) => continue,
            }
        }
    };

    // Task 3: Audio render loop (core 0 renders voices 0..2, core 1 renders 2..4)
    let audio_render = async {
        #[allow(static_mut_refs)]
        let voices = unsafe { mc_render::VOICES.assume_init_mut() };
        #[allow(static_mut_refs)]
        let voice_ages = unsafe { &mut mc_render::VOICE_AGES };
        #[allow(static_mut_refs)]
        let voice_age = unsafe { &mut mc_render::VOICE_AGE };
        let init_patch = load_rom1a_voice(0).unwrap();
        let mut channels: [ChannelState; NUM_CHANNELS] = core::array::from_fn(|_| ChannelState::new(init_patch.clone()));
        let mut output = [0i32; N];
        let mut duties = [0u16; N];
        // Static to avoid async stack pressure (~96 bytes)
        static mut FILTER: OutputFilterF32 = OutputFilterF32 {
            dc1: DcBlockerF32 { r: 0.9993455, x1: 0.0, y1: 0.0 },
            dc2: DcBlockerF32 { r: 0.9993455, x1: 0.0, y1: 0.0 },
            // 4th-order Butterworth LPF at 10500 Hz / 48000 Hz
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

        unsafe { dma_audio::init(); }

        info!("USB MIDI ready — dual-core, {} voices, DMA audio", MAX_VOICES);

        loop {
            // Drain MIDI queue
            while let Some(msg) = MIDI_QUEUE.pop() {
                let sysex_bank = if SYSEX_BANK_LOADED.load(core::sync::atomic::Ordering::Acquire) {
                    #[allow(static_mut_refs)]
                    Some(unsafe { &SYSEX_BANK })
                } else {
                    None
                };
                dispatch_midi(msg, voices, voice_ages, voice_age, &mut channels, sysex_bank,
                    SYNTH_MODE.load(Ordering::Acquire));
            }

            let t0 = read_cycles();

            // Signal core 1 to render voices 2..4
            mc_render::RENDER_DONE.store(false, core::sync::atomic::Ordering::Relaxed);
            mc_render::RENDER_START.store(true, core::sync::atomic::Ordering::Release);

            // Render voices 0..MAX_VOICES/2 on core 0
            output.fill(0);
            for idx in 0..(MAX_VOICES / 2) {
                if !voices[idx].is_finished() {
                    let mut voice_buf = [0i32; N];
                    voices[idx].render(&mut voice_buf);
                    for i in 0..N {
                        output[i] = qadd(output[i], voice_buf[i]);
                    }
                }
            }

            // Wait for core 1 to finish, yielding to let USB tasks run
            while !mc_render::RENDER_DONE.load(core::sync::atomic::Ordering::Acquire) {
                embassy_futures::yield_now().await;
            }

            // Combine core 1's rendered output
            #[allow(static_mut_refs)]
            for i in 0..N {
                output[i] = qadd(output[i], unsafe { mc_render::CORE1_BUF[i] });
            }

            // Convert to f32, apply DC blocker + LPF, then scale to 10-bit PWM duty
            for i in 0..N {
                // Convert i32 to f32. Single voice peaks ±2^26.
                // Divide by 2^26 * 3 for 8-voice mix — soft-clip handles peaks.
                let sample_f32 = output[i] as f32 / (67108864.0 * MAX_VOICES as f32);
                let filtered = filter.process(sample_f32);
                // Soft clip: tanh approximation (smooth limiting, no harsh distortion)
                let x = filtered;
                let soft = if x > 1.0 {
                    1.0
                } else if x < -1.0 {
                    -1.0
                } else {
                    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
                };
                // Scale to 10-bit PWM: ±1.0 → ±512, center at 512
                let duty = (soft * 512.0 + 512.5) as i32;
                duties[i] = usat::<10>(duty) as u16;
            }

            // Store render cycles for async CPU meter (excludes DMA wait)
            mc_render::RENDER_CYCLES_PEAK.fetch_max(
                read_cycles().wrapping_sub(t0),
                core::sync::atomic::Ordering::Relaxed,
            );

            // Wait for DMA buffer available, yielding to let USB tasks run
            while !dma_audio::poll_and_check() {
                embassy_futures::yield_now().await;
            }
            // Fill DMA buffer with rendered duties
            unsafe {
                let buf = dma_audio::get_fill_buffer();
                for i in 0..N {
                    buf[i] = (duties[i] as u32) << 16;
                }
                dma_audio::buffer_filled();
            }

            embassy_futures::yield_now().await;
        }
    };

    // Run all four concurrently on core 0
    embassy_futures::join::join4(usb_run, midi_read, cpu_meter, audio_render).await;
    core::unreachable!()
}

// === USB + BLE MIDI combined synth (dual-core, CYW43) ===

#[cfg(all(feature = "usb-midi", feature = "ble-midi", feature = "pwm"))]
async fn usb_ble_midi_pwm_synth(
    usb_peripheral: Peri<'static, embassy_rp::peripherals::USB>,
    pio0: Peri<'static, embassy_rp::peripherals::PIO0>,
    pin_23: Peri<'static, embassy_rp::peripherals::PIN_23>,
    pin_24: Peri<'static, embassy_rp::peripherals::PIN_24>,
    pin_25: Peri<'static, embassy_rp::peripherals::PIN_25>,
    pin_29: Peri<'static, embassy_rp::peripherals::PIN_29>,
    dma_ch2: Peri<'static, embassy_rp::peripherals::DMA_CH2>,
    pwm_slice: Peri<'static, embassy_rp::peripherals::PWM_SLICE7>,
    pwm_pin: Peri<'static, embassy_rp::peripherals::PIN_15>,
    core1: Peri<'static, embassy_rp::peripherals::CORE1>,
) -> ! {
    use embassy_rp::pwm::{Config as PwmConfig, Pwm};
    use embassy_rp::gpio::{Level, Output};
    use embassy_rp::pio::Pio;
    use embassy_rp::usb::Driver;
    use trouble_host::prelude::*;

    info!("USB+BLE MIDI synth with PWM output on GP15 (dual-core render)");

    // Setup PWM (10-bit: 200MHz/1024 ≈ 195kHz carrier)
    let mut pwm_config = PwmConfig::default();
    pwm_config.top = 1023;
    pwm_config.compare_b = 512;
    let _pwm = Pwm::new_output_b(pwm_slice, pwm_pin, pwm_config);

    // Initialize shared voice pool
    #[allow(static_mut_refs)]
    unsafe {
        mc_render::VOICES.write(core::array::from_fn(|_| Voice::new()));
    }

    // Start core 1 for parallel voice rendering
    #[allow(static_mut_refs)]
    unsafe {
        embassy_rp::multicore::spawn_core1(core1, &mut CORE1_STACK, || -> ! {
            mc_render::core1_entry()
        });
    }

    // --- CYW43 initialization ---
    let pwr = Output::new(pin_23, Level::Low);
    let cs = Output::new(pin_25, Level::High);
    let mut pio = Pio::new(pio0, Irqs);
    let clock_div = fixed::FixedU32::<fixed::types::extra::U8>::from_num(4);
    let spi = cyw43_pio::PioSpi::new(
        &mut pio.common,
        pio.sm0,
        clock_div,
        pio.irq0,
        cs,
        pin_24,
        pin_29,
        dma_ch2,
    );

    static CYW43_STATE: static_cell::StaticCell<cyw43::State> = static_cell::StaticCell::new();
    let state = CYW43_STATE.init(cyw43::State::new());

    let (_net_device, bt_device, mut control, runner) =
        cyw43::new_with_bluetooth(state, pwr, spi, cyw43_fw::FW, cyw43_fw::BTFW).await;

    // GATT server
    #[gatt_server]
    struct MidiServer {
        midi_svc: MidiSvc,
    }

    #[gatt_service(uuid = "03B80E5A-EDE8-4B33-A751-6CE34EC4C700")]
    struct MidiSvc {
        #[characteristic(uuid = "7772E5DB-3868-4112-A1A9-F2669D106BF3", write_without_response, read, notify)]
        midi_io: [u8; 20],
    }

    static MIDI_QUEUE: dx7_midi::MidiQueue = dx7_midi::MidiQueue::new();

    // SysEx reception buffer (max 4104 bytes for DX7 32-voice bulk dump)
    static mut SYSEX_RX_BUF: [u8; 4200] = [0u8; 4200];
    static mut SYSEX_RX_POS: usize = 0;
    static mut SYSEX_RX_ACTIVE: bool = false;
    static mut SYSEX_BANK: [u8; 4096] = [0u8; 4096];
    static SYSEX_BANK_LOADED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);

    // --- Setup USB MIDI ---
    let driver = Driver::new(usb_peripheral, Irqs);
    let mut usb_config = embassy_usb::Config::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("DX7");
    usb_config.product = Some("DX7 MIDI Synth");
    usb_config.serial_number = Some("DX7-RPI-001");

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut msos_descriptor = [0u8; 256];
    let mut control_buf = [0u8; 64];

    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );

    let midi = embassy_usb::class::midi::MidiClass::new(&mut builder, 1, 1, 64);
    let mut usb = builder.build();
    let (mut sender, mut receiver) = midi.split();

    // Task: CYW43 background driver
    let cyw43_task = runner.run();

    // Task: USB device driver
    let usb_run = usb.run();

    // Task: CPU meter — sends CC 119 via USB MIDI every 500ms (async, never blocks render)
    let cpu_meter = async {
        loop {
            embassy_time::Timer::after_millis(500).await;
            let cycles = mc_render::RENDER_CYCLES_PEAK.swap(0, core::sync::atomic::Ordering::Relaxed);
            // Budget: 64 samples at 48kHz = 266667 cycles at 200MHz
            let pct = ((cycles as u64 * 127) / 266667).min(127) as u8;
            // USB MIDI packet: CIN=0x0B (CC), channel 1, CC#119, value
            let packet = [0x0B, 0xB0, 119, pct];
            let _ = sender.write_packet(&packet).await;
        }
    };

    // Task: USB MIDI reader (with SysEx accumulation)
    let midi_read = async {
        loop {
            receiver.wait_connection().await;
            let mut buf = [0u8; 64];
            match receiver.read_packet(&mut buf).await {
                Ok(n) => {
                    for chunk in buf[..n].chunks_exact(4) {
                        let cin = chunk[0] & 0x0F;
                        match cin {
                            0x04 => {
                                #[allow(static_mut_refs)]
                                unsafe {
                                    if chunk[1] == 0xF0 {
                                        SYSEX_RX_POS = 0;
                                        SYSEX_RX_ACTIVE = true;
                                    }
                                    if SYSEX_RX_ACTIVE {
                                        for &b in &chunk[1..4] {
                                            if SYSEX_RX_POS < SYSEX_RX_BUF.len() {
                                                SYSEX_RX_BUF[SYSEX_RX_POS] = b;
                                                SYSEX_RX_POS += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            0x05 | 0x06 | 0x07 => {
                                #[allow(static_mut_refs)]
                                unsafe {
                                    if SYSEX_RX_ACTIVE {
                                        let count = (cin - 0x04) as usize;
                                        for &b in &chunk[1..1 + count] {
                                            if SYSEX_RX_POS < SYSEX_RX_BUF.len() {
                                                SYSEX_RX_BUF[SYSEX_RX_POS] = b;
                                                SYSEX_RX_POS += 1;
                                            }
                                        }
                                        let len = SYSEX_RX_POS;
                                        if len == 5
                                            && SYSEX_RX_BUF[0] == 0xF0
                                            && SYSEX_RX_BUF[1] == 0x7D
                                            && SYSEX_RX_BUF[4] == 0xF7
                                        {
                                            match SYSEX_RX_BUF[2] {
                                                0x01 => {
                                                    let mode = SYSEX_RX_BUF[3];
                                                    SYNTH_MODE.store(mode, Ordering::Release);
                                                    info!("Mode: {}", if mode == 0 { "DX7" } else { "GM" });
                                                    for msg_ch in 0..16u8 {
                                                        MIDI_QUEUE.push(dx7_midi::MidiMessage::ControlChange {
                                                            channel: msg_ch, controller: 123, value: 0,
                                                        });
                                                    }
                                                }
                                                0x02 => {
                                                    let ch = SYSEX_RX_BUF[3];
                                                    MIDI_CHANNEL.store(ch, Ordering::Release);
                                                    if ch == 0x7F {
                                                        info!("MIDI channel: OMNI");
                                                    } else {
                                                        info!("MIDI channel: {}", ch + 1);
                                                    }
                                                }
                                                _ => {
                                                    info!("SysEx: unknown cmd 0x{:02X}", SYSEX_RX_BUF[2]);
                                                }
                                            }
                                        } else if len == 4104
                                            && SYSEX_RX_BUF[0] == 0xF0
                                            && SYSEX_RX_BUF[1] == 0x43
                                            && (SYSEX_RX_BUF[2] & 0xF0) == 0x00
                                            && SYSEX_RX_BUF[3] == 0x09
                                            && SYSEX_RX_BUF[4] == 0x20
                                            && SYSEX_RX_BUF[5] == 0x00
                                            && SYSEX_RX_BUF[4103] == 0xF7
                                        {
                                            let sum: u8 = SYSEX_RX_BUF[6..4102]
                                                .iter()
                                                .fold(0u8, |acc, &b| acc.wrapping_add(b));
                                            let expected = (!sum).wrapping_add(1) & 0x7F;
                                            if expected == SYSEX_RX_BUF[4102] {
                                                SYSEX_BANK.copy_from_slice(
                                                    &SYSEX_RX_BUF[6..4102],
                                                );
                                                SYSEX_BANK_LOADED.store(
                                                    true,
                                                    core::sync::atomic::Ordering::Release,
                                                );
                                                info!("SysEx: loaded 32-voice bank");
                                            } else {
                                                info!(
                                                    "SysEx: checksum mismatch ({} vs {})",
                                                    expected, SYSEX_RX_BUF[4102]
                                                );
                                            }
                                        } else if len > 0 {
                                            info!("SysEx: ignored ({} bytes)", len);
                                        }
                                        SYSEX_RX_ACTIVE = false;
                                    }
                                }
                            }
                            _ => {
                                dx7_midi::usb::parse_usb_midi_event(chunk, &MIDI_QUEUE);
                            }
                        }
                    }
                }
                Err(_) => continue,
            }
        }
    };

    // Everything else runs after CYW43 runner is started (via join)
    let ble_usb_and_audio = async {
        // CYW43 control init
        control.init(cyw43_fw::CLM).await;
        control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;
        info!("CYW43 initialized with Bluetooth");

        static BLE_CONNECTED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);

        // LED blink task: blink while advertising, solid when connected
        let led_task = async {
            loop {
                if BLE_CONNECTED.load(core::sync::atomic::Ordering::Relaxed) {
                    control.gpio_set(0, true).await;
                    embassy_time::Timer::after_millis(200).await;
                } else {
                    control.gpio_set(0, true).await;
                    embassy_time::Timer::after_millis(300).await;
                    control.gpio_set(0, false).await;
                    embassy_time::Timer::after_millis(300).await;
                }
            }
        };

        // trouble-host BLE stack
        let controller: ExternalController<_, 1> = ExternalController::new(bt_device);
        let address: Address = Address::random([0xD7, 0x07, 0x42, 0x01, 0x02, 0x03]);

        let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
        let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
        let Host { mut peripheral, runner: mut ble_runner, .. } = stack.build();

        let server = MidiServer::new_with_config(GapConfig::default("DX7")).unwrap();

        let ble_host_task = ble_runner.run();

        // BLE MIDI advertise + connection handler
        let ble_task = async {
            const MIDI_SVC_UUID: [u8; 16] = [
                0x00, 0xC7, 0xC4, 0x4E, 0xE3, 0x6C, 0x51, 0xA7,
                0x33, 0x4B, 0xE8, 0xED, 0x5A, 0x0E, 0xB8, 0x03,
            ];
            loop {
                let mut adv_buf = [0u8; 31];
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

                info!("BLE: advertising as 'DX7'...");
                BLE_CONNECTED.store(false, core::sync::atomic::Ordering::Relaxed);
                let advertiser = peripheral.advertise(&Default::default(), adv_data).await.unwrap();
                let conn = advertiser.accept().await.unwrap();
                BLE_CONNECTED.store(true, core::sync::atomic::Ordering::Relaxed);
                info!("BLE: connected!");

                let gatt_conn = conn.with_attribute_server(&server).unwrap();

                loop {
                    match gatt_conn.next().await {
                        GattConnectionEvent::Disconnected { .. } => {
                            BLE_CONNECTED.store(false, core::sync::atomic::Ordering::Relaxed);
                            info!("BLE: disconnected");
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

        // Audio render loop (shared MIDI_QUEUE fed by both USB and BLE)
        let audio_render = async {
            #[allow(static_mut_refs)]
            let voices = unsafe { mc_render::VOICES.assume_init_mut() };
            #[allow(static_mut_refs)]
            let voice_ages = unsafe { &mut mc_render::VOICE_AGES };
            #[allow(static_mut_refs)]
            let voice_age = unsafe { &mut mc_render::VOICE_AGE };
            let init_patch = load_rom1a_voice(0).unwrap();
            let mut channels: [ChannelState; NUM_CHANNELS] = core::array::from_fn(|_| ChannelState::new(init_patch.clone()));
            let mut output = [0i32; N];
            let mut duties = [0u16; N];
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

            unsafe { dma_audio::init(); }

            info!("USB+BLE MIDI ready — dual-core, {} voices, DMA audio", MAX_VOICES);

            loop {
                // Drain MIDI queue (fed by both USB and BLE)
                while let Some(msg) = MIDI_QUEUE.pop() {
                    let sysex_bank = if SYSEX_BANK_LOADED.load(core::sync::atomic::Ordering::Acquire) {
                        #[allow(static_mut_refs)]
                        Some(unsafe { &SYSEX_BANK })
                    } else {
                        None
                    };
                    dispatch_midi(msg, voices, voice_ages, voice_age, &mut channels, sysex_bank,
                        SYNTH_MODE.load(Ordering::Acquire));
                }

                let t0 = read_cycles();

                // Signal core 1
                mc_render::RENDER_DONE.store(false, core::sync::atomic::Ordering::Relaxed);
                mc_render::RENDER_START.store(true, core::sync::atomic::Ordering::Release);

                // Render voices 0..MAX_VOICES/2 on core 0
                output.fill(0);
                for idx in 0..(MAX_VOICES / 2) {
                    if !voices[idx].is_finished() {
                        let mut voice_buf = [0i32; N];
                        voices[idx].render(&mut voice_buf);
                        for i in 0..N {
                            output[i] = qadd(output[i], voice_buf[i]);
                        }
                    }
                }

                while !mc_render::RENDER_DONE.load(core::sync::atomic::Ordering::Acquire) {
                    embassy_futures::yield_now().await;
                }

                #[allow(static_mut_refs)]
                for i in 0..N {
                    output[i] = qadd(output[i], unsafe { mc_render::CORE1_BUF[i] });
                }

                for i in 0..N {
                    let sample_f32 = output[i] as f32 / (67108864.0 * MAX_VOICES as f32);
                    let filtered = filter.process(sample_f32);
                    let x = filtered;
                    let soft = if x > 1.0 {
                        1.0
                    } else if x < -1.0 {
                        -1.0
                    } else {
                        x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
                    };
                    let duty = (soft * 512.0 + 512.5) as i32;
                    duties[i] = usat::<10>(duty) as u16;
                }

                // Store render cycles for async CPU meter (excludes DMA wait)
                mc_render::RENDER_CYCLES_PEAK.fetch_max(
                    read_cycles().wrapping_sub(t0),
                    core::sync::atomic::Ordering::Relaxed,
                );

                while !dma_audio::poll_and_check() {
                    embassy_futures::yield_now().await;
                }
                unsafe {
                    let buf = dma_audio::get_fill_buffer();
                    for i in 0..N {
                        buf[i] = (duties[i] as u32) << 16;
                    }
                    dma_audio::buffer_filled();
                }

                embassy_futures::yield_now().await;
            }
        };

        // Run LED, BLE host, BLE task, and audio concurrently
        embassy_futures::join::join4(
            led_task,
            ble_host_task,
            ble_task,
            audio_render,
        ).await
    };

    // CYW43 runner + USB driver must run alongside everything else
    embassy_futures::join::join3(cyw43_task, usb_run,
        embassy_futures::join::join3(midi_read, cpu_meter, ble_usb_and_audio),
    ).await;
    core::unreachable!()
}

// === BLE MIDI + PWM live synth (dual-core, CYW43) ===

#[cfg(all(feature = "ble-midi", not(feature = "usb-midi"), feature = "pwm"))]
async fn ble_midi_pwm_synth(
    pio0: Peri<'static, embassy_rp::peripherals::PIO0>,
    pin_23: Peri<'static, embassy_rp::peripherals::PIN_23>,
    pin_24: Peri<'static, embassy_rp::peripherals::PIN_24>,
    pin_25: Peri<'static, embassy_rp::peripherals::PIN_25>,
    pin_29: Peri<'static, embassy_rp::peripherals::PIN_29>,
    dma_ch2: Peri<'static, embassy_rp::peripherals::DMA_CH2>,
    pwm_slice: Peri<'static, embassy_rp::peripherals::PWM_SLICE7>,
    pwm_pin: Peri<'static, embassy_rp::peripherals::PIN_15>,
    core1: Peri<'static, embassy_rp::peripherals::CORE1>,
) -> ! {
    use embassy_rp::pwm::{Config as PwmConfig, Pwm};
    use embassy_rp::gpio::{Level, Output};
    use embassy_rp::pio::Pio;
    use trouble_host::prelude::*;

    info!("BLE MIDI synth with PWM output on GP15 (dual-core render)");

    // Setup PWM (10-bit: 200MHz/1024 ≈ 195kHz carrier)
    let mut pwm_config = PwmConfig::default();
    pwm_config.top = 1023;
    pwm_config.compare_b = 512;
    let _pwm = Pwm::new_output_b(pwm_slice, pwm_pin, pwm_config);

    // Initialize shared voice pool
    #[allow(static_mut_refs)]
    unsafe {
        mc_render::VOICES.write(core::array::from_fn(|_| Voice::new()));
    }

    // Start core 1 for parallel voice rendering
    #[allow(static_mut_refs)]
    unsafe {
        embassy_rp::multicore::spawn_core1(core1, &mut CORE1_STACK, || -> ! {
            mc_render::core1_entry()
        });
    }

    // --- CYW43 initialization ---
    let pwr = Output::new(pin_23, Level::Low);
    let cs = Output::new(pin_25, Level::High);
    let mut pio = Pio::new(pio0, Irqs);
    // PIO SPI clock divider: sysclk / divider must be ≤ 50MHz for CYW43.
    // At 200MHz overclock, divider=4 gives 50MHz SPI.
    let clock_div = fixed::FixedU32::<fixed::types::extra::U8>::from_num(4);
    let spi = cyw43_pio::PioSpi::new(
        &mut pio.common,
        pio.sm0,
        clock_div,
        pio.irq0,
        cs,
        pin_24,  // DIO
        pin_29,  // CLK
        dma_ch2,
    );

    static CYW43_STATE: static_cell::StaticCell<cyw43::State> = static_cell::StaticCell::new();
    let state = CYW43_STATE.init(cyw43::State::new());

    let (_net_device, bt_device, mut control, runner) =
        cyw43::new_with_bluetooth(state, pwr, spi, cyw43_fw::FW, cyw43_fw::BTFW).await;

    // GATT server with BLE MIDI service (must be defined before async blocks capture references)
    #[gatt_server]
    struct MidiServer {
        midi_svc: MidiSvc,
    }

    #[gatt_service(uuid = "03B80E5A-EDE8-4B33-A751-6CE34EC4C700")]
    struct MidiSvc {
        #[characteristic(uuid = "7772E5DB-3868-4112-A1A9-F2669D106BF3", write_without_response, read, notify)]
        midi_io: [u8; 20],
    }

    static MIDI_QUEUE: dx7_midi::MidiQueue = dx7_midi::MidiQueue::new();

    // Task 1: CYW43 background driver (must run continuously)
    let cyw43_task = runner.run();

    // Task 2+3: Init CYW43, setup BLE, then run BLE + audio concurrently.
    // control.init() needs the CYW43 runner to be active, so this runs alongside cyw43_task.
    let ble_and_audio = async {
        // CYW43 control init (sends ioctls to the runner — runner must be running)
        control.init(cyw43_fw::CLM).await;
        control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;
        info!("CYW43 initialized with Bluetooth");

        static BLE_CONNECTED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);

        // LED blink task: blink while advertising, solid when connected
        let led_task = async {
            loop {
                if BLE_CONNECTED.load(core::sync::atomic::Ordering::Relaxed) {
                    control.gpio_set(0, true).await;
                    embassy_time::Timer::after_millis(200).await;
                } else {
                    control.gpio_set(0, true).await;
                    embassy_time::Timer::after_millis(300).await;
                    control.gpio_set(0, false).await;
                    embassy_time::Timer::after_millis(300).await;
                }
            }
        };

        // trouble-host BLE stack
        let controller: ExternalController<_, 1> = ExternalController::new(bt_device);
        let address: Address = Address::random([0xD7, 0x07, 0x42, 0x01, 0x02, 0x03]);

        let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
        let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
        let Host { mut peripheral, runner: mut ble_runner, .. } = stack.build();

        let server = MidiServer::new_with_config(GapConfig::default("DX7")).unwrap();

        // BLE host stack runner
        let ble_host_task = ble_runner.run();

        // BLE MIDI advertise + connection handler
        let ble_task = async {
            // BLE MIDI service UUID (03B80E5A-EDE8-4B33-A751-6CE34EC4C700) in little-endian
            const MIDI_SVC_UUID: [u8; 16] = [
                0x00, 0xC7, 0xC4, 0x4E, 0xE3, 0x6C, 0x51, 0xA7,
                0x33, 0x4B, 0xE8, 0xED, 0x5A, 0x0E, 0xB8, 0x03,
            ];
            loop {
                let mut adv_buf = [0u8; 31];
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

                info!("BLE: advertising as 'DX7'...");
                BLE_CONNECTED.store(false, core::sync::atomic::Ordering::Relaxed);
                let advertiser = peripheral.advertise(&Default::default(), adv_data).await.unwrap();
                let conn = advertiser.accept().await.unwrap();
                BLE_CONNECTED.store(true, core::sync::atomic::Ordering::Relaxed);
                info!("BLE: connected!");

                let gatt_conn = conn.with_attribute_server(&server).unwrap();

                loop {
                    match gatt_conn.next().await {
                        GattConnectionEvent::Disconnected { .. } => {
                            BLE_CONNECTED.store(false, core::sync::atomic::Ordering::Relaxed);
                            info!("BLE: disconnected");
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

        // Audio render loop
        let audio_render = async {
        #[allow(static_mut_refs)]
        let voices = unsafe { mc_render::VOICES.assume_init_mut() };
        #[allow(static_mut_refs)]
        let voice_ages = unsafe { &mut mc_render::VOICE_AGES };
        #[allow(static_mut_refs)]
        let voice_age = unsafe { &mut mc_render::VOICE_AGE };
        let init_patch = load_rom1a_voice(0).unwrap();
        let mut channels: [ChannelState; NUM_CHANNELS] = core::array::from_fn(|_| ChannelState::new(init_patch.clone()));
        let mut output = [0i32; N];
        let mut duties = [0u16; N];
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

        unsafe { dma_audio::init(); }

        info!("BLE MIDI ready — dual-core, {} voices, DMA audio", MAX_VOICES);

        loop {
            // Drain MIDI queue
            while let Some(msg) = MIDI_QUEUE.pop() {
                dispatch_midi(msg, voices, voice_ages, voice_age, &mut channels, None,
                    SYNTH_MODE.load(Ordering::Acquire));
            }

            let t0 = read_cycles();

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
                        output[i] = qadd(output[i], voice_buf[i]);
                    }
                }
            }

            // Wait for core 1
            while !mc_render::RENDER_DONE.load(core::sync::atomic::Ordering::Acquire) {
                embassy_futures::yield_now().await;
            }

            // Combine core 1 output
            #[allow(static_mut_refs)]
            for i in 0..N {
                output[i] = qadd(output[i], unsafe { mc_render::CORE1_BUF[i] });
            }

            // Convert to f32, filter, soft-clip, scale to 10-bit PWM duty
            for i in 0..N {
                let sample_f32 = output[i] as f32 / (67108864.0 * MAX_VOICES as f32);
                let filtered = filter.process(sample_f32);
                let x = filtered;
                let soft = if x > 1.0 {
                    1.0
                } else if x < -1.0 {
                    -1.0
                } else {
                    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
                };
                let duty = (soft * 512.0 + 512.5) as i32;
                duties[i] = usat::<10>(duty) as u16;
            }

            // Store render cycles for CPU monitoring
            mc_render::RENDER_CYCLES_PEAK.fetch_max(
                read_cycles().wrapping_sub(t0),
                core::sync::atomic::Ordering::Relaxed,
            );

            // Wait for DMA buffer, yield to let BLE tasks run
            while !dma_audio::poll_and_check() {
                embassy_futures::yield_now().await;
            }
            unsafe {
                let buf = dma_audio::get_fill_buffer();
                for i in 0..N {
                    buf[i] = (duties[i] as u32) << 16;
                }
                dma_audio::buffer_filled();
            }

            embassy_futures::yield_now().await;
        }
    };

        // Run LED, BLE host, BLE task, and audio render concurrently
        embassy_futures::join::join4(led_task, ble_host_task, ble_task, audio_render).await
    };

    // Run CYW43 driver alongside everything else.
    // cyw43_task must be running before control.init() inside ble_and_audio.
    embassy_futures::join::join(cyw43_task, ble_and_audio).await;
    core::unreachable!()
}
