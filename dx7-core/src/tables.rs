//! Sine, Exp2, and Frequency lookup tables for FM synthesis.
//!
//! Based on MSFA (Apache 2.0, Google Inc.) for envelope, frequency, and LFO tables.
//! Log-domain sine/exp tables implement the standard OPL-family technique described in:
//! <https://github.com/gtaylormb/opl3_fpga/blob/master/docs/opl3math/opl3math.pdf>
//! Tables must be initialized by calling `init_tables(sample_rate)` before use.

use crate::generated_tables;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU8, Ordering};

/// Block size exponent: N = 2^LG_N = 64 samples per block.
pub const LG_N: i32 = 6;
/// Block size for sub-sampled processing.
pub const N: usize = 1 << LG_N as usize;

// --- Sine table ---
const SIN_LG_N_SAMPLES: i32 = 10;
const SIN_N_SAMPLES: usize = 1 << SIN_LG_N_SAMPLES as usize;

// --- Exp2 table ---
const EXP2_LG_N_SAMPLES: i32 = 10;
const EXP2_N_SAMPLES: usize = 1 << EXP2_LG_N_SAMPLES as usize;

// --- Frequency LUT ---
const FREQ_SAMPLE_SHIFT: i32 = 24 - 10; // 14
const FREQ_MAX_LOGFREQ_INT: i32 = 20;

// --- Log-domain sine/exp tables ---
// OPL-family log-domain technique: store sin as log2 attenuation, apply envelope
// by addition in log domain, convert back via exp table. Standard approach used
// in YM2414/OPL3 FM synthesis chips.
pub const ENV_BITDEPTH: u16 = 14;
pub const ENV_MAX: u16 = 1 << ENV_BITDEPTH; // 16384

/// Sample rate multiplier for envelope/LFO rate compensation (Q24).
/// sr_multiplier = (44100 / sample_rate) * (1 << 24)
static SR_MULTIPLIER: AtomicU32 = AtomicU32::new(1 << 24);

/// Frequency LUTs live in flash/rodata; freqlut_lookup runs once per
/// operator per 64-sample block, so XIP latency is irrelevant here.
static FREQLUT_44100: [i32; 1025] = generated_tables::FREQLUT_44100;
static FREQLUT_48000: [i32; 1025] = generated_tables::FREQLUT_48000;

/// Pointer to the active FREQLUT (selected by sample rate at init).
static FREQLUT: AtomicPtr<i32> = AtomicPtr::new(FREQLUT_44100.as_ptr() as *mut i32);

/// RAM-resident copies of the per-sample hot tables (copied from flash at
/// init). On embedded targets, flash XIP cache misses make const table
/// lookups slow, and these are read for every sample of every operator.
struct RamTables {
    sinlog_full: [u16; 4096],
    sinexp: [u16; 1024],
}

/// Interior-mutable wrapper. Soundness contract: the contents are written
/// exactly once, by the thread that wins `RAM_STATE` 0 -> 1, and only read
/// after `RAM_STATE` is 2. `init_tables` enforces this; readers must not
/// run before `init_tables` has returned at least once on some thread.
struct RamCell(UnsafeCell<RamTables>);
unsafe impl Sync for RamCell {}

static RAM: RamCell = RamCell(UnsafeCell::new(RamTables {
    sinlog_full: [0; 4096],
    sinexp: [0; 1024],
}));

const RAM_UNINIT: u8 = 0;
const RAM_FILLING: u8 = 1;
const RAM_READY: u8 = 2;
static RAM_STATE: AtomicU8 = AtomicU8::new(RAM_UNINIT);

/// Initialize all lookup tables. Must be called at least once before any
/// audio rendering (`Synth::new` does this). Safe to call repeatedly and
/// from several threads: the sample-rate-independent tables are filled
/// once, and the sample-rate-dependent selections are atomics where the
/// most recent call wins.
///
/// Only 44100 and 48000 Hz are supported.
pub fn init_tables(sample_rate: u32) {
    match RAM_STATE.compare_exchange(RAM_UNINIT, RAM_FILLING, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            // We own the one-time fill.
            // SAFETY: RAM_STATE is FILLING, so no other thread reads or writes
            // the cell until we publish READY below.
            let ram = unsafe { &mut *RAM.0.get() };
            // Expand quarter-wave SINLOG to full-wave (4096 entries) for branchless lookup
            for i in 0..1024 {
                let s = generated_tables::SINLOG_TABLE[i];
                let m = generated_tables::SINLOG_TABLE[i ^ 0x3FF];
                ram.sinlog_full[i] = s;                 // Q0: sin(0..pi/2)
                ram.sinlog_full[1024 + i] = m;          // Q1: sin(pi/2..pi)
                ram.sinlog_full[2048 + i] = s | 0x8000; // Q2: -sin(0..pi/2)
                ram.sinlog_full[3072 + i] = m | 0x8000; // Q3: -sin(pi/2..pi)
            }
            ram.sinexp.copy_from_slice(&generated_tables::SINEXP_TABLE);
            RAM_STATE.store(RAM_READY, Ordering::Release);
        }
        Err(_) => {
            // Another thread is filling or has filled; wait for READY.
            while RAM_STATE.load(Ordering::Acquire) != RAM_READY {
                core::hint::spin_loop();
            }
        }
    }

    match sample_rate {
        48000 => {
            FREQLUT.store(FREQLUT_48000.as_ptr() as *mut i32, Ordering::Release);
            // (44100 / 48000) * (1 << 24) = 15420469
            SR_MULTIPLIER.store(15420469, Ordering::Release);
        }
        _ => {
            // Default to 44100
            FREQLUT.store(FREQLUT_44100.as_ptr() as *mut i32, Ordering::Release);
            SR_MULTIPLIER.store(1 << 24, Ordering::Release);
        }
    }
}

/// Get the sample rate multiplier (Q24). Used by envelope and LFO for
/// rate compensation relative to 44100 Hz.
#[inline]
pub fn sr_multiplier() -> u32 {
    SR_MULTIPLIER.load(Ordering::Relaxed)
}

/// Shared view of the RAM tables.
/// SAFETY (caller): `init_tables` must have completed at least once.
#[inline(always)]
fn ram() -> &'static RamTables {
    unsafe { &*RAM.0.get() }
}

// --- Lookup functions (hot path, ported from sin.h, exp2.h, freqlut.cc) ---

/// Sine lookup with linear interpolation. Q24 phase in, Q24 amplitude out.
/// Phase wraps naturally at 24 bits (0..2^24 = one full cycle).
#[inline]
pub fn sin_lookup(phase: i32) -> i32 {
    const SHIFT: i32 = 24 - SIN_LG_N_SAMPLES; // 14
    let lowbits = phase & ((1 << SHIFT) - 1);
    let phase_int =
        ((phase >> (SHIFT - 1)) & (((SIN_N_SAMPLES as i32) - 1) << 1)) as usize;

    let dy = generated_tables::SINTAB[phase_int];
    let y0 = generated_tables::SINTAB[phase_int + 1];
    y0 + (((dy as i64) * (lowbits as i64)) >> SHIFT) as i32
}

/// Exp2 lookup: Q24 log input → Q24 linear output.
/// Computes 2^(x / 2^24) scaled to Q24.
#[inline]
pub fn exp2_lookup(x: i32) -> i32 {
    const SHIFT: i32 = 24 - EXP2_LG_N_SAMPLES; // 14
    let lowbits = x & ((1 << SHIFT) - 1);
    let x_int =
        ((x >> (SHIFT - 1)) & (((EXP2_N_SAMPLES as i32) - 1) << 1)) as usize;

    let dy = generated_tables::EXP2TAB[x_int];
    let y0 = generated_tables::EXP2TAB[x_int + 1];
    let y = y0 + (((dy as i64) * (lowbits as i64)) >> SHIFT) as i32;
    let shift = 6 - (x >> 24);
    if shift < 0 {
        // Would shift left — clamp to max (very loud, shouldn't happen)
        y << (-shift).min(31)
    } else if shift >= 32 {
        // Very quiet — effectively zero
        0
    } else {
        y >> shift
    }
}

/// Frequency lookup: Q24 logfreq → phase increment per sample.
/// Logfreq is log2(freq) in Q24 format (1.0 in Q24 = one octave).
#[inline]
pub fn freqlut_lookup(logfreq: i32) -> i32 {
    let ix = ((logfreq & 0xffffff) >> FREQ_SAMPLE_SHIFT) as usize;

    let lut = FREQLUT.load(Ordering::Relaxed);
    // SAFETY: FREQLUT only ever points at one of the two 1025-entry static
    // tables, and ix <= 1023, so ix + 1 is in bounds.
    unsafe {
        let y0 = *lut.add(ix);
        let y1 = *lut.add(ix + 1);
        let lowbits = logfreq & ((1 << FREQ_SAMPLE_SHIFT) - 1);
        let y = y0 + ((((y1 - y0) as i64) * (lowbits as i64)) >> FREQ_SAMPLE_SHIFT) as i32;
        // Integer part of log2(freq). Coarse 31 + fine 99 + pitch EG +4 oct
        // + bend on a high note can exceed FREQ_MAX_LOGFREQ_INT, and a deep
        // negative pitch EG on a low note can go below 0; clamp the shift so
        // neither direction overflows (would panic in debug, wrap in release).
        let hibits = logfreq >> 24;
        let shift = (FREQ_MAX_LOGFREQ_INT - hibits).clamp(0, 31);
        y >> shift
    }
}

// --- Log-domain sine/exp lookup functions ---
//
// These implement the standard OPL-family log-domain sine technique:
//   1. Quarter-wave sine table stores round(-1024 * log2(sin(x))) for x in (0, pi/2)
//   2. Full-wave lookup uses quadrant bits to mirror and negate
//   3. Result is log-attenuation with bit 15 as sign flag
//
// Reference: OPL3 FPGA math documentation (gtaylormb/opl3_fpga)

/// Log-sine lookup with quadrant handling. Input `phi` uses lower 12 bits:
/// bits 9..0 = table index, bits 11..10 = quadrant. Returns log attenuation
/// with bit 15 as sign flag.
#[inline(always)]
pub fn sin_log(phi: u16) -> u16 {
    ram().sinlog_full[(phi & 0xFFF) as usize]
}

/// Exp table lookup: converts log-attenuation back to linear. Returns mantissa (0..4095).
#[inline(always)]
pub fn sin_exp(index: u16) -> u16 {
    ram().sinexp[(index & 0x3FF) as usize]
}

/// Convert MIDI note number to Q24 logfreq (standard 12-TET, A4=440Hz).
/// logfreq = log2(freq) * (1 << 24).
#[inline]
pub fn midinote_to_logfreq(note: i32) -> i32 {
    generated_tables::MIDINOTE_LOGFREQ[(note as usize) & 127]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_init() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            init_tables(44100);
            crate::lfo::init_lfo(44100);
            crate::pitchenv::init_pitchenv(44100);
        });
    }

    #[test]
    fn test_sin_lookup_zero() {
        ensure_init();
        let val = sin_lookup(0);
        assert!(val.abs() <= 1, "sin(0) should be ~0, got {val}");
    }

    #[test]
    fn test_sin_lookup_quarter() {
        ensure_init();
        // Phase at 1/4 cycle = 2^24 / 4 = 2^22
        let quarter = 1 << 22;
        let val = sin_lookup(quarter);
        // Should be near max (~2^24 / (2*pi) scale... actually Q24 output)
        // Peak of MSFA sine is ~(1<<24) = 16777216
        assert!(val > 1 << 23, "sin(pi/2) should be large positive, got {val}");
    }

    #[test]
    fn test_sin_lookup_half() {
        ensure_init();
        let half = 1 << 23;
        let val = sin_lookup(half);
        assert!(val.abs() < 1000, "sin(pi) should be ~0, got {val}");
    }

    #[test]
    fn test_exp2_lookup_zero() {
        ensure_init();
        // exp2(0) should be 2^0 = 1.0 in Q24 = (1<<24) = 16777216
        // But shifted by >> 6 in the lookup, so the base is (1<<30) >> 6 = (1<<24)
        let val = exp2_lookup(0);
        let expected = 1 << 24;
        let diff = (val - expected).abs();
        assert!(diff < 100, "exp2(0) should be ~{expected}, got {val}");
    }

    #[test]
    fn test_freqlut_basic() {
        ensure_init();
        // A4 = 440 Hz, logfreq = log2(440) * (1<<24) ≈ 147M
        let logfreq = midinote_to_logfreq(69);
        let phase_inc = freqlut_lookup(logfreq);
        // phase_inc should be positive and reasonable
        assert!(phase_inc > 0, "Phase increment should be positive, got {phase_inc}");
    }

    /// Issue #6: init_tables must be safe to call concurrently and
    /// repeatedly while other threads render.
    /// (Only 44100 here: the sample-rate selection is process-global by
    /// design, and other tests in this binary assume 44100.)
    #[test]
    fn test_concurrent_init_and_render() {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    let sr = 44100;
                    let mut synth = crate::synth::Synth::new(sr);
                    synth.load_patch(crate::patch::DxVoice::init_voice());
                    synth.note_on(60, 100);
                    let mut out = vec![0.0f32; 4096];
                    for _ in 0..20 {
                        synth.render(&mut out);
                        init_tables(sr);
                    }
                    out.iter().fold(0.0f32, |a, &b| a.max(b.abs()))
                })
            })
            .collect();
        for h in handles {
            let peak = h.join().expect("thread panicked");
            assert!(peak > 0.01, "every synth should render audio, peak={peak}");
        }
        // Sine tables are sample-rate independent and must be intact.
        assert_eq!(sin_log(0), generated_tables::SINLOG_TABLE[0]);
        assert_eq!(sin_log(2048) & 0x8000, 0x8000);
    }

    /// Issue #5: shift amount must stay in range for any logfreq.
    #[test]
    fn test_freqlut_extreme_logfreq_does_not_panic() {
        ensure_init();
        // 24.6 octaves (above FREQ_MAX_LOGFREQ_INT = 20) and well below zero.
        let hi = freqlut_lookup((24 << 24) + (10 << 20));
        let lo = freqlut_lookup(-(12 << 24));
        assert!(hi > 0, "very high logfreq should saturate, not overflow");
        assert_eq!(lo, 0, "very low logfreq should round to zero");
        // Within range, higher logfreq still means a larger increment.
        assert!(freqlut_lookup(19 << 24) > freqlut_lookup(18 << 24));
    }

    #[test]
    fn test_midinote_to_logfreq_octave() {
        // One octave up should add exactly (1<<24) to logfreq
        let a4 = midinote_to_logfreq(69);
        let a5 = midinote_to_logfreq(81); // 69 + 12
        let diff = a5 - a4;
        let octave = 1 << 24;
        assert!(
            (diff - octave).abs() < 2,
            "Octave should be exactly 1<<24={octave}, got diff={diff}"
        );
    }
}
