#![no_std]
#![no_main]

extern crate alloc;

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use dx7_core::voice::{Voice, VoiceState};
use dx7_core::load_rom1a_voice;
use dx7_core::tables::N;

const MAX_VOICES: usize = 6;

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

// --- Audio ring buffer (core 0 writes, core 1 reads) ---

#[cfg(feature = "pwm")]
mod audio_ring {
    use core::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

    /// Ring buffer capacity (must be power of 2). 256 samples = 4 blocks of headroom.
    const RING_SIZE: usize = 256;
    const RING_MASK: usize = RING_SIZE - 1;

    /// Ring buffer of pre-scaled PWM duty values (0..1023 for 10-bit).
    static RING: [AtomicU16; RING_SIZE] = {
        const INIT: AtomicU16 = AtomicU16::new(512);
        [INIT; RING_SIZE]
    };
    static HEAD: AtomicUsize = AtomicUsize::new(0); // written by core 0
    static TAIL: AtomicUsize = AtomicUsize::new(0); // read by core 1

    /// Number of samples available to read.
    #[inline]
    pub fn available() -> usize {
        let h = HEAD.load(Ordering::Acquire);
        let t = TAIL.load(Ordering::Relaxed);
        h.wrapping_sub(t) & RING_MASK
    }

    /// Number of free slots for writing.
    #[inline]
    pub fn free_slots() -> usize {
        // Keep one slot empty to distinguish full from empty
        (RING_SIZE - 1) - available()
    }

    /// Push a block of pre-scaled duty values. Caller must ensure free_slots() >= count.
    #[inline]
    pub fn push_block(duties: &[u16]) {
        let mut h = HEAD.load(Ordering::Relaxed);
        for &d in duties {
            RING[h & RING_MASK].store(d, Ordering::Relaxed);
            h = h.wrapping_add(1);
        }
        HEAD.store(h, Ordering::Release);
    }

    /// Pop one duty value. Returns 512 (silence) if empty.
    #[inline]
    pub fn pop() -> u16 {
        let t = TAIL.load(Ordering::Relaxed);
        if t == HEAD.load(Ordering::Acquire) {
            return 512; // underrun → silence (10-bit center)
        }
        let val = RING[t & RING_MASK].load(Ordering::Relaxed);
        TAIL.store(t.wrapping_add(1), Ordering::Release);
        val
    }
}

// --- Multi-core rendering: timer ISR for PWM + parallel voice rendering ---

#[cfg(all(feature = "usb-midi", feature = "pwm"))]
mod mc_render {
    use core::sync::atomic::{AtomicBool, Ordering};
    use dx7_core::voice::Voice;
    use dx7_core::tables::N;
    use super::{MAX_VOICES, audio_ring};

    /// Shared voice pool. Core 0 handles all note_on/note_off (when core 1 is idle).
    /// During render: core 0 renders voices 0..2, core 1 renders voices 2..4.
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

    // TIMER0 registers (already enabled by embassy for its time driver).
    // We use ALARM3 which embassy doesn't use.
    const T0_ALARM3: *mut u32 = (0x400B_0000 + 0x1C) as *mut u32;
    const T0_TIMERAWL: *const u32 = (0x400B_0000 + 0x28) as *const u32;
    // RP2350 has PAUSE/LOCKED/SOURCE registers between TIMERAWL and INTR,
    // shifting interrupt registers compared to RP2040
    const T0_INTR: *mut u32 = (0x400B_0000 + 0x3C) as *mut u32;
    const T0_INTE: *mut u32 = (0x400B_0000 + 0x40) as *mut u32;

    // PWM slice 7 channel B compare register
    const PWM_CC7: *mut u32 = (0x400A_8000 + 7 * 0x14 + 0x0C) as *mut u32;

    // ISR state (only accessed from ISR on core 1 — no sync needed)
    static mut ISR_FRAC: u32 = 0;
    static mut ISR_NEXT: u32 = 0;

    // Vector table for core 1 (needs alignment for VTOR)
    #[repr(C, align(256))]
    struct VTable([usize; 48]);
    static mut CORE1_VTABLE: VTable = VTable([0; 48]);

    /// Timer0 Alarm3 ISR: pops one sample from ring buffer, writes PWM duty.
    /// Runs at 48kHz on core 1 via interrupt.
    #[allow(static_mut_refs)]
    unsafe extern "C" fn timer_isr() {
        // Clear alarm 3 interrupt (write-1-to-clear bit 3)
        core::ptr::write_volatile(T0_INTR, 1 << 3);

        // Pop sample and write PWM channel B (upper 16 bits of CC register)
        let duty = audio_ring::pop();
        let cc = core::ptr::read_volatile(PWM_CC7);
        core::ptr::write_volatile(PWM_CC7, (cc & 0xFFFF) | ((duty as u32) << 16));

        // Schedule next alarm (fractional 48kHz: 125/6 = 20.8333 us)
        ISR_FRAC += 125;
        let advance = ISR_FRAC / 6;
        ISR_FRAC %= 6;
        ISR_NEXT = ISR_NEXT.wrapping_add(advance);
        core::ptr::write_volatile(T0_ALARM3, ISR_NEXT);
    }

    extern "C" fn default_handler() {
        loop { cortex_m::asm::nop(); }
    }

    /// Core 1 entry: sets up timer ISR for PWM output, then renders voices 2..4 on demand.
    #[allow(static_mut_refs)]
    pub unsafe fn core1_entry() -> ! {
        // Build vector table for core 1 with our timer ISR
        let default_addr = default_handler as *const () as usize;
        for entry in CORE1_VTABLE.0.iter_mut() {
            *entry = default_addr;
        }
        // IRQ 3 = TIMER0_IRQ_3 → vector table index 16 + 3 = 19
        CORE1_VTABLE.0[16 + 3] = timer_isr as *const () as usize;

        // Set VTOR on core 1 to our table
        core::ptr::write_volatile(0xE000_ED08 as *mut u32, CORE1_VTABLE.0.as_ptr() as u32);

        // Initialize TIMER0 ALARM3
        let now = core::ptr::read_volatile(T0_TIMERAWL);
        ISR_FRAC = 0;
        ISR_NEXT = now.wrapping_add(21);
        core::ptr::write_volatile(T0_ALARM3, ISR_NEXT);

        // Enable alarm 3 interrupt (read-modify-write to preserve embassy's alarm bits)
        let inte = core::ptr::read_volatile(T0_INTE);
        core::ptr::write_volatile(T0_INTE, inte | (1 << 3));

        // Enable IRQ 3 (TIMER0_IRQ_3) in core 1's NVIC
        core::ptr::write_volatile(0xE000_E100 as *mut u32, 1 << 3);

        // Ensure interrupts are globally enabled on core 1
        cortex_m::interrupt::enable();

        let voices = VOICES.assume_init_mut();

        // Render loop: wait for signal from core 0, render voices 2..4, signal done
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

// --- USB interrupt binding ---

#[cfg(feature = "usb-midi")]
embassy_rp::bind_interrupts!(struct Irqs {
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
    let patch = load_rom1a_voice(10).unwrap();
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

    // USB MIDI synth never returns — handle separately to avoid unreachable-code warning
    #[cfg(all(feature = "usb-midi", feature = "pwm"))]
    usb_midi_pwm_synth(p.USB, p.PWM_SLICE7, p.PIN_15, p.CORE1).await;

    #[cfg(not(all(feature = "usb-midi", feature = "pwm")))]
    {
        #[cfg(all(feature = "pwm", not(feature = "usb-midi")))]
        pwm_demo(&patch, p.PWM_SLICE7, p.PIN_15, p.CORE1);

        #[cfg(not(feature = "pwm"))]
        let _ = p;

        info!("Done.");
        loop {
            embassy_time::Timer::after_secs(1).await;
        }
    }
}

// === PWM demo playback (hardcoded note, dual-core) ===

#[cfg(all(feature = "pwm", not(feature = "usb-midi")))]
fn pwm_demo(
    patch: &dx7_core::DxVoice,
    slice: embassy_rp::peripherals::PWM_SLICE7,
    pin: embassy_rp::peripherals::PIN_15,
    core1: embassy_rp::peripherals::CORE1,
) {
    use embassy_rp::pwm::{Config as PwmConfig, Pwm};

    info!("PWM audio on GP15 (dual-core, 10-bit, ~195 kHz carrier)");

    // Setup PWM on core 0
    let mut config = PwmConfig::default();
    config.top = 1023;
    config.compare_b = 512;
    let _pwm = Pwm::new_output_b(slice, pin, config);

    // Start core 1 for PWM output
    start_pwm_core1(core1);

    // Pre-fill ring buffer with 2 blocks of silence
    let silence = [512u16; N];
    audio_ring::push_block(&silence);
    audio_ring::push_block(&silence);

    let mut voice = Voice::new();
    voice.note_on(patch, 60, 100);
    let note_blocks = (SAMPLE_RATE as usize * 2) / N;
    let mut output = [0i32; N];
    let mut duties = [0u16; N];

    info!("Playing {} blocks...", note_blocks);
    for block in 0..note_blocks {
        voice.render(&mut output);
        if block == note_blocks / 2 {
            voice.note_off();
        }
        // Scale output to 10-bit PWM duty (0..1023)
        for i in 0..N {
            let duty = (output[i] >> 17) + 512;
            duties[i] = usat::<10>(duty) as u16;
        }
        // Wait for space in ring buffer
        while audio_ring::free_slots() < N {
            cortex_m::asm::nop();
        }
        audio_ring::push_block(&duties);
    }
    // Wait for ring buffer to drain
    while audio_ring::available() > 0 {
        cortex_m::asm::nop();
    }
    info!("Playback done.");
}

// === Core 1: PWM sample output ===

#[cfg(feature = "pwm")]
static mut CORE1_STACK: embassy_rp::multicore::Stack<4096> = embassy_rp::multicore::Stack::new();

/// Start core 1 to drain the audio ring buffer to PWM at sample-accurate timing.
/// PWM must already be configured on core 0 before calling this.
/// Used by pwm_demo only; USB MIDI synth uses mc_render::core1_entry instead.
#[cfg(all(feature = "pwm", not(feature = "usb-midi")))]
fn start_pwm_core1(core1: embassy_rp::peripherals::CORE1) {
    // Microseconds per sample (integer part). 48kHz = 20.833us per sample.
    // We alternate 21/21/21/21/20 to average 20.8333 (exact 48kHz over 6 samples).
    // Pattern: 5 samples at 21us + 1 sample at 16us? No — simpler: use fractional accumulator.
    const USEC_PER_SAMPLE_X6: u32 = 125; // 125/6 = 20.8333 us exactly (48000 Hz)

    #[allow(static_mut_refs)]
    unsafe {
        embassy_rp::multicore::spawn_core1(core1, &mut CORE1_STACK, move || -> ! {
            // PWM slice 7, channel B compare register (bits 31:16 of CC)
            // RP2350 PWM base: 0x400A8000, slice stride: 0x14, CC offset: 0x0C
            let pwm_cc = (0x400A_8000u32 + 7 * 0x14 + 0x0C) as *mut u32;

            // Use RP2350 TIMER0 timerawl (1 MHz, shared between cores)
            // TIMER0 base: 0x400B0000, timerawl offset: 0x28
            let timer_raw = 0x400B_0028 as *const u32;

            let mut last_us = core::ptr::read_volatile(timer_raw);
            // Fractional accumulator: counts in units of 1/6 microsecond
            // Each sample advances by 125/6 us. We track (last_us * 6 + frac).
            let mut frac: u32 = 0;

            loop {
                let duty = audio_ring::pop();
                // Write channel B compare (upper 16 bits of CC register)
                let cc = core::ptr::read_volatile(pwm_cc);
                core::ptr::write_volatile(pwm_cc, (cc & 0xFFFF) | ((duty as u32) << 16));

                // Advance fractional accumulator by 125 (= 20.8333 us * 6)
                frac += USEC_PER_SAMPLE_X6;
                let advance_us = frac / 6;
                frac %= 6;
                let target = last_us.wrapping_add(advance_us);

                // Busy-wait on shared timer until target time
                while core::ptr::read_volatile(timer_raw)
                    .wrapping_sub(target)
                    > 1_000_000 // guard: if we're within 1 second, we haven't passed yet
                {}
                last_us = target;
            }
        });
    }
}

// === USB MIDI + PWM live synth (dual-core) ===

#[cfg(all(feature = "usb-midi", feature = "pwm"))]
async fn usb_midi_pwm_synth(
    usb_peripheral: embassy_rp::peripherals::USB,
    pwm_slice: embassy_rp::peripherals::PWM_SLICE7,
    pwm_pin: embassy_rp::peripherals::PIN_15,
    core1: embassy_rp::peripherals::CORE1,
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

    // Start core 1: timer ISR for PWM output + parallel voice rendering
    #[allow(static_mut_refs)]
    unsafe {
        embassy_rp::multicore::spawn_core1(core1, &mut CORE1_STACK, || -> ! {
            mc_render::core1_entry()
        });
    }

    // Pre-fill ring buffer with 2 blocks of silence
    let silence = [512u16; N];
    audio_ring::push_block(&silence);
    audio_ring::push_block(&silence);

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
                                        if len == 4104
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
        let mut current_patch = load_rom1a_voice(0).unwrap();
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

        // CPU utilization tracking
        let budget_cycles = (N as u32 * CPU_HZ) / SAMPLE_RATE;
        let blocks_per_sec = SAMPLE_RATE / N as u32;
        let mut block_count: u32 = 0;
        let mut peak_cycles: u32 = 0;
        let mut peak_raw: i32 = 0;      // peak |output[i]| over 1 second
        let mut peak_duty_off: u16 = 0; // peak |duty - 128| over 1 second

        info!("USB MIDI ready — dual-core, {} voices", MAX_VOICES);

        loop {
            // Drain MIDI queue
            while let Some(msg) = MIDI_QUEUE.pop() {
                match msg {
                    dx7_midi::MidiMessage::NoteOn { note, velocity } => {
                        *voice_age += 1;
                        let slot = voices.iter().position(|v| v.is_finished())
                            .or_else(|| {
                                // oldest released voice
                                voices.iter().enumerate()
                                    .filter(|(_, v)| v.state == VoiceState::Released)
                                    .min_by_key(|(i, _)| voice_ages[*i])
                                    .map(|(i, _)| i)
                            })
                            .unwrap_or_else(|| {
                                // steal oldest active
                                (0..MAX_VOICES).min_by_key(|&i| voice_ages[i]).unwrap()
                            });
                        voices[slot].note_on(&current_patch, note, velocity);
                        voice_ages[slot] = *voice_age;
                    }
                    dx7_midi::MidiMessage::NoteOff { note, .. } => {
                        for v in voices.iter_mut() {
                            if v.note == note && !v.is_finished() {
                                v.note_off();
                                break;
                            }
                        }
                    }
                    dx7_midi::MidiMessage::ProgramChange { program } => {
                        if program == 32 {
                            // Pure sine test patch (INIT VOICE)
                            current_patch = dx7_core::DxVoice::init_voice();
                        } else if SYSEX_BANK_LOADED.load(core::sync::atomic::Ordering::Acquire)
                        {
                            // Load from received SysEx bank
                            let idx = program as usize;
                            if idx < 32 {
                                let start = idx * 128;
                                let mut voice_data = [0u8; 128];
                                #[allow(static_mut_refs)]
                                unsafe {
                                    voice_data.copy_from_slice(
                                        &SYSEX_BANK[start..start + 128],
                                    );
                                }
                                current_patch =
                                    dx7_core::DxVoice::from_packed(&voice_data);
                                info!("Loaded sysex patch {}", idx);
                            }
                        } else if let Some(p) = load_rom1a_voice(program as usize) {
                            current_patch = p;
                        }
                    }
                    dx7_midi::MidiMessage::ControlChange { .. } => {}
                    _ => {}
                }
            }

            // Signal core 1 to render voices 2..4
            mc_render::RENDER_DONE.store(false, core::sync::atomic::Ordering::Relaxed);
            mc_render::RENDER_START.store(true, core::sync::atomic::Ordering::Release);

            // Render voices 0..MAX_VOICES/2 on core 0
            output.fill(0);
            let render_start = read_cycles();
            for idx in 0..(MAX_VOICES / 2) {
                if !voices[idx].is_finished() {
                    let mut voice_buf = [0i32; N];
                    voices[idx].render(&mut voice_buf);
                    for i in 0..N {
                        output[i] = qadd(output[i], voice_buf[i]);
                    }
                }
            }
            let render_cycles = read_cycles().wrapping_sub(render_start);

            // Wait for core 1 to finish, yielding to let USB tasks run
            while !mc_render::RENDER_DONE.load(core::sync::atomic::Ordering::Acquire) {
                embassy_futures::yield_now().await;
            }

            // Combine core 1's rendered output
            #[allow(static_mut_refs)]
            for i in 0..N {
                output[i] = qadd(output[i], unsafe { mc_render::CORE1_BUF[i] });
            }

            if render_cycles > peak_cycles {
                peak_cycles = render_cycles;
            }

            // Convert to f32, apply DC blocker + LPF, then scale to 10-bit PWM duty
            for i in 0..N {
                let raw_abs = output[i].abs();
                if raw_abs > peak_raw {
                    peak_raw = raw_abs;
                }
                // Convert i32 to f32. Single voice peaks ±2^26.
                // Divide by 2^26 * 2 — loud enough for typical playing,
                // soft-clip handles occasional peaks from dense chords.
                let sample_f32 = output[i] as f32 / (67108864.0 * 2.0);
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
                let off = if duties[i] >= 512 { duties[i] - 512 } else { 512 - duties[i] };
                if off > peak_duty_off {
                    peak_duty_off = off;
                }
            }

            // Wait for space in ring buffer, yielding to let USB tasks run
            while audio_ring::free_slots() < N {
                embassy_futures::yield_now().await;
            }
            audio_ring::push_block(&duties);

            // Send diagnostics once per second
            block_count += 1;
            if block_count >= blocks_per_sec {
                let cpu_pct = ((peak_cycles as u64 * 127) / budget_cycles as u64) as u8;
                let cpu_val = if cpu_pct > 127 { 127 } else { cpu_pct };
                // CC 119: CPU utilization (0-127)
                let _ = sender.write_packet(&[0x0B, 0xB0, 0x77, cpu_val]).await;
                // CC 118: peak duty offset from 512 (0=silence, 127=max swing)
                let duty_val = ((peak_duty_off as u32 * 127) / 512).min(127) as u8;
                let _ = sender.write_packet(&[0x0B, 0xB0, 0x76, duty_val]).await;
                // CC 117: peak raw output (log scale: bits used, 0=silent, 26=max)
                let raw_bits = if peak_raw == 0 { 0u8 } else { (32 - peak_raw.leading_zeros()) as u8 };
                // Scale 0-26 range to 0-127
                let raw_val = ((raw_bits as u16 * 127) / 26).min(127) as u8;
                let _ = sender.write_packet(&[0x0B, 0xB0, 0x75, raw_val]).await;

                block_count = 0;
                peak_cycles = 0;
                peak_raw = 0;
                peak_duty_off = 0;
            }

            // Yield to let USB tasks process
            embassy_futures::yield_now().await;
        }
    };

    // Run all three concurrently on core 0
    embassy_futures::join::join3(usb_run, midi_read, audio_render).await;
    core::unreachable!()
}
