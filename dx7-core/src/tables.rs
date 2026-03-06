//! Sine, Exp2, and Frequency lookup tables for FM synthesis.
//!
//! Ported from Dexed/MSFA (Apache 2.0, Google Inc. / Pascal Gauthier).
//! Tables must be initialized by calling `init_tables(sample_rate)` before use.

use crate::generated_tables;

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

// --- MkI (Mark I) engine tables ---
// OPL-style log-domain sine/exp lookup for DX7-accurate FM synthesis.
pub const ENV_BITDEPTH: u16 = 14;
pub const ENV_MAX: u16 = 1 << ENV_BITDEPTH; // 16384

/// Sample rate multiplier for envelope/LFO rate compensation (Q24).
/// sr_multiplier = (44100 / sample_rate) * (1 << 24)
static mut SR_MULTIPLIER: u32 = 1 << 24;

/// Pointer to the active FREQLUT (selected by sample rate at init).
static mut FREQLUT: *const i32 = core::ptr::null();

/// Initialize all lookup tables. Must be called once at startup before any
/// audio rendering. Not thread-safe — call from a single thread.
///
/// Only 44100 and 48000 Hz are supported.
pub fn init_tables(sample_rate: u32) {
    unsafe {
        match sample_rate {
            48000 => {
                FREQLUT = generated_tables::FREQLUT_48000.as_ptr();
                // (44100 / 48000) * (1 << 24) = 15420469
                SR_MULTIPLIER = 15420469;
            }
            _ => {
                // Default to 44100
                FREQLUT = generated_tables::FREQLUT_44100.as_ptr();
                SR_MULTIPLIER = 1 << 24;
            }
        }
    }
}

/// Get the sample rate multiplier (Q24). Used by envelope and LFO for
/// rate compensation relative to 44100 Hz.
#[inline]
pub fn sr_multiplier() -> u32 {
    unsafe { SR_MULTIPLIER }
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

    unsafe {
        let y0 = *FREQLUT.add(ix);
        let y1 = *FREQLUT.add(ix + 1);
        let lowbits = logfreq & ((1 << FREQ_SAMPLE_SHIFT) - 1);
        let y = y0 + ((((y1 - y0) as i64) * (lowbits as i64)) >> FREQ_SAMPLE_SHIFT) as i32;
        let hibits = logfreq >> 24;
        y >> (FREQ_MAX_LOGFREQ_INT - hibits)
    }
}

// --- MkI lookup functions (ported from EngineMkI.cpp) ---

/// Log-sine lookup with quadrant handling. Input `phi` uses lower 12 bits:
/// bits 9..0 = table index, bits 11..10 = quadrant. Returns log attenuation
/// with bit 15 as sign flag.
#[inline]
pub fn sin_log(phi: u16) -> u16 {
    let index = (phi & 0x3FF) as usize;
    match (phi >> 10) & 3 {
        0 => generated_tables::SINLOG_TABLE[index],
        1 => generated_tables::SINLOG_TABLE[index ^ 0x3FF],
        2 => generated_tables::SINLOG_TABLE[index] | 0x8000,
        _ => generated_tables::SINLOG_TABLE[index ^ 0x3FF] | 0x8000,
    }
}

/// Exp table lookup for MkI. Returns mantissa value (0..4095).
#[inline]
pub fn sin_exp(index: u16) -> u16 {
    generated_tables::SINEXP_TABLE[(index & 0x3FF) as usize]
}

/// Convert MIDI note number to Q24 logfreq (standard 12-TET, A4=440Hz).
/// logfreq = log2(freq) * (1 << 24).
#[inline]
pub fn midinote_to_logfreq(note: i32) -> i32 {
    generated_tables::MIDINOTE_LOGFREQ[(note as usize) & 127]
}

// --- Legacy compat (kept during migration) ---

/// 14-bit amplitude range: +/- 8191 (used by old sine table).
pub const AMP_14BIT: i32 = 8191;

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
