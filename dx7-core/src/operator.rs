//! FM operator kernel and DX7 scaling functions.
//!
//! Operator kernel uses log-domain FM synthesis (OPL-family technique).
//! Scaling functions based on MSFA (Apache 2.0, Google Inc.).

use crate::generated_tables;
use crate::tables::{self, N, LG_N};

// --- Operator parameter types ---

/// Keyboard level scaling curve types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScalingCurve {
    NegLin,
    NegExp,
    PosExp,
    PosLin,
}

impl ScalingCurve {
    pub fn from_u8(v: u8) -> Self {
        match v & 0x03 {
            0 => ScalingCurve::NegLin,
            1 => ScalingCurve::NegExp,
            2 => ScalingCurve::PosExp,
            3 => ScalingCurve::PosLin,
            _ => unreachable!(),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            ScalingCurve::NegLin => 0,
            ScalingCurve::NegExp => 1,
            ScalingCurve::PosExp => 2,
            ScalingCurve::PosLin => 3,
        }
    }
}

/// All per-operator DX7 parameters (21 params per operator in SysEx).
#[derive(Clone, Copy, Debug)]
pub struct OperatorParams {
    pub eg: crate::envelope::EnvParams,
    pub kbd_level_scaling_break_point: u8,
    pub kbd_level_scaling_left_depth: u8,
    pub kbd_level_scaling_right_depth: u8,
    pub kbd_level_scaling_left_curve: ScalingCurve,
    pub kbd_level_scaling_right_curve: ScalingCurve,
    pub kbd_rate_scaling: u8,
    pub amp_mod_sensitivity: u8,
    pub key_velocity_sensitivity: u8,
    pub output_level: u8,
    pub osc_mode: u8,
    pub osc_freq_coarse: u8,
    pub osc_freq_fine: u8,
    pub osc_detune: u8,
}

impl Default for OperatorParams {
    fn default() -> Self {
        Self {
            eg: crate::envelope::EnvParams {
                rates: [99, 99, 99, 99],
                levels: [99, 99, 99, 0],
            },
            kbd_level_scaling_break_point: 39,
            kbd_level_scaling_left_depth: 0,
            kbd_level_scaling_right_depth: 0,
            kbd_level_scaling_left_curve: ScalingCurve::NegLin,
            kbd_level_scaling_right_curve: ScalingCurve::NegLin,
            kbd_rate_scaling: 0,
            amp_mod_sensitivity: 0,
            key_velocity_sensitivity: 0,
            output_level: 99,
            osc_mode: 0,
            osc_freq_coarse: 1,
            osc_freq_fine: 0,
            osc_detune: 7,
        }
    }
}

/// Per-voice runtime state for one operator.
pub struct FmOpParams {
    /// Envelope level (log domain input for gain computation).
    pub level_in: i32,
    /// Previous block's gain: log attenuation (0 = loud, ENV_MAX = silent).
    /// Stored as i32 — can go negative when EG levels exceed the 0-99 spec range.
    pub gain_out: i32,
    /// Phase increment per sample.
    pub freq: i32,
    /// Current phase accumulator.
    pub phase: i32,
}

impl FmOpParams {
    pub fn new() -> Self {
        Self {
            level_in: 0,
            gain_out: 0,
            freq: 0,
            phase: 0,
        }
    }
}

// --- Log-domain FM operator kernel ---
//
// Standard OPL-family log-domain sine technique:
//   sin(phase) * amplitude = exp(log_sin(phase) + log_envelope)
//
// In log domain, multiplication becomes addition, so applying an amplitude
// envelope is just adding the envelope attenuation to the log-sine value.
// The result is converted back to linear via an exponential lookup table.
//
// Reference: OPL3 FPGA math documentation (gtaylormb/opl3_fpga)

/// Log-domain sine with envelope: computes sin(phase) * 2^(-env/1024).
///
/// 1. Convert 24-bit phase to 12-bit, look up log2(|sin|) from quarter-wave table
/// 2. Add envelope attenuation in log domain (addition = multiplication in linear)
/// 3. Split combined log value: lower 10 bits → exp table mantissa, upper bits → exponent
/// 4. Convert mantissa back to linear, right-shift by exponent
/// 5. Apply sign from phase quadrant
///
/// Output range: approximately [-2^26, +2^26].
#[inline(always)]
pub fn log_sin(phase: i32, env: u16) -> i32 {
    // Step 1-2: log-sine lookup + envelope attenuation (both in log domain)
    let combined = tables::sin_log((phase >> 12) as u16).wrapping_add(env);
    // Step 5 prep: extract sign from bit 15 (set by sin_log for negative quadrants)
    let is_negative = (combined & 0x8000) != 0;
    let attenuation = combined & 0x7FFF;
    // Step 3-4: convert log-attenuation back to linear amplitude
    // Lower 10 bits index the exp table (XOR inverts because table is decreasing)
    // Add implicit 1.0 mantissa (4096) to the fractional part from the table
    let mantissa = 4096u32 + tables::sin_exp((attenuation & 0x3FF) ^ 0x3FF) as u32;
    // Upper bits are the exponent (right-shift amount)
    let linear = mantissa >> (attenuation >> 10);
    // Step 5: apply sign and scale to output range (shift left 13 for ~26-bit output)
    if is_negative {
        (-(linear as i32) - 1) << 13
    } else {
        (linear as i32) << 13
    }
}

/// FM operator with modulation input (no feedback). Log-domain version.
pub fn compute(
    output: &mut [i32; N],
    input: &[i32; N],
    phase0: i32,
    freq: i32,
    gain1: i32,
    gain2: i32,
    add: bool,
) {
    let dgain = (gain2 - gain1 + (N as i32 >> 1)) >> LG_N;
    let mut gain = gain1;
    let mut phase = phase0;
    if add {
        for i in 0..N {
            gain += dgain;
            let y = log_sin(phase.wrapping_add(input[i]), gain as u16);
            output[i] += y;
            phase = phase.wrapping_add(freq);
        }
    } else {
        for i in 0..N {
            gain += dgain;
            let y = log_sin(phase.wrapping_add(input[i]), gain as u16);
            output[i] = y;
            phase = phase.wrapping_add(freq);
        }
    }
}

/// Pure sine generator, no modulation input. Log-domain version.
pub fn compute_pure(
    output: &mut [i32; N],
    phase0: i32,
    freq: i32,
    gain1: i32,
    gain2: i32,
    add: bool,
) {
    let dgain = (gain2 - gain1 + (N as i32 >> 1)) >> LG_N;
    let mut gain = gain1;
    let mut phase = phase0;
    if add {
        for i in 0..N {
            gain += dgain;
            let y = log_sin(phase, gain as u16);
            output[i] += y;
            phase = phase.wrapping_add(freq);
        }
    } else {
        for i in 0..N {
            gain += dgain;
            let y = log_sin(phase, gain as u16);
            output[i] = y;
            phase = phase.wrapping_add(freq);
        }
    }
}

/// Self-feedback operator. Log-domain version.
pub fn compute_fb(
    output: &mut [i32; N],
    phase0: i32,
    freq: i32,
    gain1: i32,
    gain2: i32,
    fb_buf: &mut [i32; 2],
    fb_shift: i32,
    add: bool,
) {
    let dgain = (gain2 - gain1 + (N as i32 >> 1)) >> LG_N;
    let mut gain = gain1;
    let mut phase = phase0;
    let mut y0 = fb_buf[0];
    let mut y = fb_buf[1];
    if add {
        for i in 0..N {
            gain += dgain;
            let scaled_fb = (y0 + y) >> (fb_shift + 1);
            y0 = y;
            y = log_sin(phase.wrapping_add(scaled_fb), gain as u16);
            output[i] += y;
            phase = phase.wrapping_add(freq);
        }
    } else {
        for i in 0..N {
            gain += dgain;
            let scaled_fb = (y0 + y) >> (fb_shift + 1);
            y0 = y;
            y = log_sin(phase.wrapping_add(scaled_fb), gain as u16);
            output[i] = y;
            phase = phase.wrapping_add(freq);
        }
    }
    fb_buf[0] = y0;
    fb_buf[1] = y;
}

/// Fused 2-operator feedback chain for algorithm 6.
/// Op 0 feeds back to itself, its output modulates op 1, op 1 is the carrier.
pub fn compute_fb2(
    output: &mut [i32; N],
    phase0_0: i32, freq0: i32, gain1_0: i32, gain2_0: i32,
    phase0_1: i32, freq1: i32, gain1_1: i32, gain2_1: i32,
    fb_buf: &mut [i32; 2],
    fb_shift: i32,
) {
    let dgain0 = (gain2_0 - gain1_0 + (N as i32 >> 1)) >> LG_N;
    let dgain1 = (gain2_1 - gain1_1 + (N as i32 >> 1)) >> LG_N;
    let mut gain0 = gain1_0;
    let mut gain1 = gain1_1;
    let mut phase0 = phase0_0;
    let mut phase1 = phase0_1;
    let mut y0 = fb_buf[0];
    let mut y = fb_buf[1];

    for i in 0..N {
        let scaled_fb = (y0 + y) >> (fb_shift + 1);
        // op 0: feedback operator
        gain0 += dgain0;
        y0 = y;
        y = log_sin(phase0.wrapping_add(scaled_fb), gain0 as u16);
        phase0 = phase0.wrapping_add(freq0);
        // op 1: modulated by op 0
        gain1 += dgain1;
        y = log_sin(phase1.wrapping_add(y), gain1 as u16);
        phase1 = phase1.wrapping_add(freq1);

        output[i] = y;
    }
    fb_buf[0] = y0;
    fb_buf[1] = y;
}

/// Fused 3-operator feedback chain for algorithm 4.
/// Op 0 feeds back, its output → op 1 → op 2 (carrier).
pub fn compute_fb3(
    output: &mut [i32; N],
    phase0_0: i32, freq0: i32, gain1_0: i32, gain2_0: i32,
    phase0_1: i32, freq1: i32, gain1_1: i32, gain2_1: i32,
    phase0_2: i32, freq2: i32, gain1_2: i32, gain2_2: i32,
    fb_buf: &mut [i32; 2],
    fb_shift: i32,
) {
    let dgain0 = (gain2_0 - gain1_0 + (N as i32 >> 1)) >> LG_N;
    let dgain1 = (gain2_1 - gain1_1 + (N as i32 >> 1)) >> LG_N;
    let dgain2 = (gain2_2 - gain1_2 + (N as i32 >> 1)) >> LG_N;
    let mut g0 = gain1_0;
    let mut g1 = gain1_1;
    let mut g2 = gain1_2;
    let mut phase0 = phase0_0;
    let mut phase1 = phase0_1;
    let mut phase2 = phase0_2;
    let mut y0 = fb_buf[0];
    let mut y = fb_buf[1];

    for i in 0..N {
        let scaled_fb = (y0 + y) >> (fb_shift + 1);
        // op 0: feedback operator
        g0 += dgain0;
        y0 = y;
        y = log_sin(phase0.wrapping_add(scaled_fb), g0 as u16);
        phase0 = phase0.wrapping_add(freq0);
        // op 1: modulated by op 0
        g1 += dgain1;
        y = log_sin(phase1.wrapping_add(y), g1 as u16);
        phase1 = phase1.wrapping_add(freq1);
        // op 2: modulated by op 1
        g2 += dgain2;
        y = log_sin(phase2.wrapping_add(y), g2 as u16);
        phase2 = phase2.wrapping_add(freq2);

        output[i] = y;
    }
    fb_buf[0] = y0;
    fb_buf[1] = y;
}

// --- DX7 scaling functions (based on MSFA dx7note.cc, Apache 2.0) ---

/// Coarse frequency multiplier table (log2 domain, Q24).
/// Index 0 = 0.5x (-1 octave), index 1 = 1x, index 2 = 2x, etc.
pub const COARSEMUL: [i32; 32] = [
    -16777216, 0, 16777216, 26591258, 33554432, 38955489, 43368474, 47099600,
    50331648, 53182516, 55732705, 58039632, 60145690, 62083076, 63876816,
    65546747, 67108864, 68576247, 69959732, 71268397, 72509921, 73690858,
    74816848, 75892776, 76922906, 77910978, 78860292, 79773775, 80654032,
    81503396, 82323963, 83117622,
];

/// Velocity sensitivity table (64 entries).
pub const VELOCITY_DATA: [u8; 64] = [
    0, 70, 86, 97, 106, 114, 121, 126, 132, 138, 142, 148, 152, 156, 160, 163,
    166, 170, 173, 174, 178, 181, 184, 186, 189, 190, 194, 196, 198, 200, 202,
    205, 206, 209, 211, 214, 216, 218, 220, 222, 224, 225, 227, 229, 230, 232,
    233, 235, 237, 238, 240, 241, 242, 243, 244, 246, 246, 248, 249, 250, 251,
    252, 253, 254,
];

/// Exponential curve scale data (33 entries).
pub const EXP_SCALE_DATA: [u8; 33] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 14, 16, 19, 23, 27, 33, 39, 47, 56, 66,
    80, 94, 110, 126, 142, 158, 174, 190, 206, 222, 238, 250,
];

/// Pitch modulation sensitivity table (8 entries).
pub const PITCHMODSENSTAB: [u8; 8] = [0, 10, 20, 33, 55, 92, 153, 255];

/// Amplitude modulation sensitivity table (4 entries, Q24 scaled).
pub const AMPMODSENSTAB: [u32; 4] = [0, 4342338, 7171437, 16777216];

/// Scale velocity to microstep delta.
pub fn scale_velocity(velocity: i32, sensitivity: i32) -> i32 {
    let clamped_vel = velocity.clamp(0, 127);
    let vel_value = VELOCITY_DATA[(clamped_vel >> 1) as usize] as i32 - 239;
    let scaled_vel = ((sensitivity * vel_value + 7) >> 3) << 4;
    scaled_vel
}

/// Scale envelope rate by keyboard position.
pub fn scale_rate(midinote: i32, sensitivity: i32) -> i32 {
    let x = (midinote / 3 - 7).clamp(0, 31);
    (sensitivity * x) >> 3
}

/// Scale level using curve type.
pub fn scale_curve(group: i32, depth: i32, curve: i32) -> i32 {
    let scale = if curve == 0 || curve == 3 {
        // Linear
        (group * depth * 329) >> 12
    } else {
        // Exponential
        let raw_exp = EXP_SCALE_DATA[group.min(32) as usize] as i32;
        (raw_exp * depth * 329) >> 15
    };
    if curve < 2 { -scale } else { scale }
}

/// Compute keyboard level scaling for an operator.
pub fn scale_level(
    midinote: i32,
    break_pt: i32,
    left_depth: i32,
    right_depth: i32,
    left_curve: i32,
    right_curve: i32,
) -> i32 {
    let offset = midinote - break_pt - 17;
    if offset >= 0 {
        scale_curve((offset + 1) / 3, right_depth, right_curve)
    } else {
        scale_curve(-(offset - 1) / 3, left_depth, left_curve)
    }
}

/// Compute operator log-frequency from DX7 patch parameters.
/// Returns Q24 logfreq (log2(freq) * (1<<24)).
pub fn osc_freq(midinote: i32, mode: i32, coarse: i32, fine: i32, detune: i32) -> i32 {
    if mode == 0 {
        // Ratio mode
        let mut logfreq = tables::midinote_to_logfreq(midinote);

        // Detune (empirically measured from DX7 hardware)
        // DETUNE_TAB[n] = round(0.0209 * exp(-0.396 * logfreq_frac) / 7.0 * logfreq)
        logfreq += generated_tables::DETUNE_TAB[midinote as usize] * (detune - 7);

        logfreq += COARSEMUL[(coarse & 31) as usize];
        if fine != 0 {
            logfreq += generated_tables::FINE_TAB[fine as usize];
        }
        logfreq
    } else {
        // Fixed frequency mode
        // ((1 << 24) * log(10) / log(2) * 0.01) << 3 = 4458616
        let mut logfreq = (4458616i64 * ((coarse & 3) * 100 + fine) as i64 >> 3) as i32;
        if detune > 7 {
            logfreq += 13457 * (detune - 7);
        }
        logfreq
    }
}

/// Feedback bit depth constant.
pub const FEEDBACK_BITDEPTH: i32 = 8;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables;

    fn ensure_init() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            tables::init_tables(44100);
            crate::lfo::init_lfo(44100);
            crate::pitchenv::init_pitchenv(44100);
        });
    }

    // ========================================================================
    // 1. log_sin — core FM function
    // ========================================================================

    #[test]
    fn test_log_sin_zero_phase_is_near_zero() {
        ensure_init();
        // sin(0) = 0. Phase=0 should produce ~0 output.
        let val = log_sin(0, 0);
        // Allow small error from table quantization
        let max_amp = 1 << 26;
        let tolerance = max_amp / 100; // 1% of peak
        assert!(
            val.abs() < tolerance,
            "log_sin(phase=0, env=0) should be ~0, got {val} (tolerance {tolerance})"
        );
    }

    #[test]
    fn test_log_sin_quarter_phase_is_peak() {
        ensure_init();
        // sin(pi/2) = 1.0. Quarter cycle in 24-bit phase = 1<<22 but
        // log_sin uses phase>>12, so 12-bit effective: quarter = 1<<10 = 1024.
        // Phase needs to be 1024 << 12 = 4194304 for quarter cycle.
        let quarter_phase = 1 << 22; // pi/2 in 24-bit phase
        let val = log_sin(quarter_phase, 0);
        // Peak should be ~(4096+4095)<<13 = ~67M ≈ 2^26
        let expected_peak = (4096 + 4095) << 13; // 66584576
        let tolerance = expected_peak / 20; // 5%
        assert!(
            val > expected_peak - tolerance,
            "log_sin at pi/2 should be near peak {expected_peak}, got {val}"
        );
    }

    #[test]
    fn test_log_sin_half_phase_is_near_zero() {
        ensure_init();
        // sin(pi) = 0
        let half_phase = 1 << 23;
        let val = log_sin(half_phase, 0);
        let max_amp = 1 << 26;
        let tolerance = max_amp / 100;
        assert!(
            val.abs() < tolerance,
            "log_sin at pi should be ~0, got {val}"
        );
    }

    #[test]
    fn test_log_sin_three_quarter_is_negative_peak() {
        ensure_init();
        // sin(3*pi/2) = -1.0
        let three_q_phase = 3 * (1 << 22); // 3/4 of 2^24
        let val = log_sin(three_q_phase, 0);
        let expected_peak = (4096 + 4095) << 13;
        let tolerance = expected_peak / 20;
        assert!(
            val < -(expected_peak - tolerance),
            "log_sin at 3pi/2 should be near -{expected_peak}, got {val}"
        );
    }

    #[test]
    fn test_log_sin_symmetry() {
        ensure_init();
        // sin(x) = -sin(x + pi)
        let phase_a: i32 = 1234567;
        let phase_b = phase_a.wrapping_add(1 << 23);
        let val_a = log_sin(phase_a, 0);
        let val_b = log_sin(phase_b, 0);
        let tolerance = (1 << 26) / 50; // 2%
        assert!(
            (val_a + val_b).abs() < tolerance,
            "sin(x) + sin(x+pi) should be ~0, got {} + {} = {}",
            val_a, val_b, val_a + val_b
        );
    }

    #[test]
    fn test_log_sin_max_amplitude() {
        ensure_init();
        // Sweep all phases at env=0, find the peak amplitude
        let mut max_pos = 0i32;
        let mut max_neg = 0i32;
        for i in 0..4096 {
            let phase = i << 12; // step through all 12-bit phase values
            let val = log_sin(phase, 0);
            if val > max_pos { max_pos = val; }
            if val < max_neg { max_neg = val; }
        }
        // Expected peak: (4096 + 4095) << 13 = 66584576
        // Expected peak: ~(4096+4095)<<13 = 67100672, but table quantization
        // means actual peak may be slightly less
        let expected = (4096 + 4095) << 13;
        let tolerance = expected / 100; // 1%
        assert!(
            max_pos >= expected - tolerance,
            "Max positive should be ~{expected}, got {max_pos}"
        );
        assert!(
            max_neg <= -(expected - tolerance),
            "Max negative should be ~-{expected}, got {max_neg}"
        );
        // Verify approximate symmetry
        let sym_tolerance = expected / 10; // 10%
        assert!(
            (max_pos + max_neg).abs() < sym_tolerance,
            "Positive/negative peaks should be roughly symmetric: +{max_pos} vs {max_neg}"
        );
    }

    // ========================================================================
    // 2. Envelope attenuation
    // ========================================================================

    #[test]
    fn test_log_sin_env_attenuates() {
        ensure_init();
        let phase = 1 << 22; // pi/2 (peak)
        let val_loud = log_sin(phase, 0);
        let val_quiet = log_sin(phase, 1024);
        let val_silent = log_sin(phase, tables::ENV_MAX - 1);
        assert!(
            val_loud > val_quiet,
            "Higher env should attenuate: env=0 gave {val_loud}, env=1024 gave {val_quiet}"
        );
        assert!(
            val_quiet > val_silent,
            "Even higher env should attenuate more: env=1024 gave {val_quiet}, env=max gave {val_silent}"
        );
        assert!(
            val_silent.abs() < 100,
            "Near-max env should be nearly silent, got {val_silent}"
        );
    }

    #[test]
    fn test_log_sin_env_6db_per_doubling() {
        ensure_init();
        // In log domain, adding 1024 to env should halve the amplitude (-6dB)
        // because 2^10 = 1024 and the exp table divides by powers of 2
        let phase = 1 << 22;
        let val_0 = log_sin(phase, 0) as f64;
        let val_1024 = log_sin(phase, 1024) as f64;
        let ratio = val_0 / val_1024;
        // Each 1024 in env = one bit shift = halving = 6.02 dB
        // ratio should be ~2.0
        assert!(
            (ratio - 2.0).abs() < 0.3,
            "1024 env units should halve amplitude (ratio ~2.0), got {ratio:.3}"
        );
    }

    // ========================================================================
    // 3. compute_pure — single operator sine wave
    // ========================================================================

    #[test]
    fn test_compute_pure_frequency() {
        ensure_init();
        // Generate multiple blocks at A4 (440 Hz) and verify frequency
        let logfreq = tables::midinote_to_logfreq(69); // A4
        let freq = tables::freqlut_lookup(logfreq);

        // Render enough samples for several cycles
        let num_blocks = 16;
        let total_samples = N * num_blocks;
        let mut samples = vec![0i32; total_samples];
        let mut phase = 0i32;
        let gain: i32 = 0; // max amplitude

        for block in 0..num_blocks {
            let mut output = [0i32; N];
            compute_pure(&mut output, phase, freq, gain, gain, false);
            for i in 0..N {
                samples[block * N + i] = output[i];
            }
            phase = phase.wrapping_add(freq << LG_N);
        }

        // Count zero crossings (positive-to-negative) to estimate frequency
        let mut crossings = 0;
        for i in 1..total_samples {
            if samples[i - 1] >= 0 && samples[i] < 0 {
                crossings += 1;
            }
        }
        let duration_secs = total_samples as f64 / 44100.0;
        let estimated_freq = crossings as f64 / duration_secs;
        assert!(
            (estimated_freq - 440.0).abs() < 20.0,
            "Expected ~440 Hz, estimated {estimated_freq:.1} Hz ({crossings} crossings in {duration_secs:.4}s)"
        );
    }

    #[test]
    fn test_compute_pure_amplitude_range() {
        ensure_init();
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);

        let mut max_val = 0i32;
        let mut phase = 0i32;
        let gain: i32 = 0;

        for _ in 0..100 {
            let mut output = [0i32; N];
            compute_pure(&mut output, phase, freq, gain, gain, false);
            for &s in output.iter() {
                if s.abs() > max_val { max_val = s.abs(); }
            }
            phase = phase.wrapping_add(freq << LG_N);
        }

        let expected_peak = (4096 + 4095) << 13;
        let tolerance = expected_peak / 10;
        assert!(
            max_val > expected_peak - tolerance,
            "Peak amplitude should be near {expected_peak}, got {max_val}"
        );
    }

    #[test]
    fn test_compute_pure_is_clean_sine() {
        ensure_init();
        // Render at 440 Hz for ~1024 samples and check that it matches
        // a reference sine within tolerance
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);

        let num_blocks = 16;
        let total_samples = N * num_blocks;
        let mut samples = vec![0f64; total_samples];
        let mut phase = 0i32;
        let gain: i32 = 0;

        for block in 0..num_blocks {
            let mut output = [0i32; N];
            compute_pure(&mut output, phase, freq, gain, gain, false);
            for i in 0..N {
                samples[block * N + i] = output[i] as f64;
            }
            phase = phase.wrapping_add(freq << LG_N);
        }

        // Find the peak to normalize
        let peak = samples.iter().map(|s| s.abs()).fold(0.0f64, f64::max);
        let normalized: Vec<f64> = samples.iter().map(|s| s / peak).collect();

        // Compute THD: measure power of fundamental vs harmonics via DFT
        // Use a simple single-bin DFT at the fundamental frequency
        let cycles = 440.0 * (total_samples as f64 / 44100.0);
        let fundamental_bin = cycles.round() as usize;

        // Full DFT (brute force for small N)
        let mut power_spectrum = vec![0.0f64; total_samples / 2];
        for k in 1..total_samples / 2 {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (n, &s) in normalized.iter().enumerate() {
                let angle = 2.0 * std::f64::consts::PI * k as f64 * n as f64 / total_samples as f64;
                re += s * angle.cos();
                im += s * angle.sin();
            }
            power_spectrum[k] = (re * re + im * im).sqrt();
        }

        let fund_power = power_spectrum[fundamental_bin];
        // Sum harmonic power (2nd through 5th)
        let mut harmonic_power = 0.0f64;
        for h in 2..=5 {
            let bin = fundamental_bin * h;
            if bin < power_spectrum.len() {
                harmonic_power += power_spectrum[bin] * power_spectrum[bin];
            }
        }
        let thd = harmonic_power.sqrt() / fund_power;
        assert!(
            thd < 0.05,
            "THD should be < 5% for a clean sine, got {:.2}%",
            thd * 100.0
        );
    }

    #[test]
    fn test_compute_pure_add_flag() {
        ensure_init();
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;

        let mut output1 = [0i32; N];
        compute_pure(&mut output1, 0, freq, gain, gain, false);

        // Add a second time with add=true
        let mut output2 = output1;
        compute_pure(&mut output2, 0, freq, gain, gain, true);

        // Each sample should be doubled
        for i in 0..N {
            assert_eq!(
                output2[i], output1[i] * 2,
                "add=true should sum: sample {i}: {} vs expected {}",
                output2[i], output1[i] * 2
            );
        }
    }

    // ========================================================================
    // 4. FM modulation — compute()
    // ========================================================================

    #[test]
    fn test_compute_modulation_adds_harmonics() {
        ensure_init();
        // Carrier at 440 Hz modulated by 440 Hz (ratio 1:1)
        // Should produce sidebands at n*440 Hz (harmonics)
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);

        let num_blocks = 32;
        let total_samples = N * num_blocks;
        let gain: i32 = 0;

        // First: pure carrier (no modulation)
        let mut pure_samples = vec![0f64; total_samples];
        let mut phase = 0i32;
        for block in 0..num_blocks {
            let mut output = [0i32; N];
            compute_pure(&mut output, phase, freq, gain, gain, false);
            for i in 0..N {
                pure_samples[block * N + i] = output[i] as f64;
            }
            phase = phase.wrapping_add(freq << LG_N);
        }

        // Then: modulated carrier
        let mut mod_samples = vec![0f64; total_samples];
        let mut carrier_phase = 0i32;
        let mut mod_phase = 0i32;
        for block in 0..num_blocks {
            // Generate modulator
            let mut mod_buf = [0i32; N];
            compute_pure(&mut mod_buf, mod_phase, freq, gain, gain, false);
            mod_phase = mod_phase.wrapping_add(freq << LG_N);
            // Generate carrier with modulation
            let mut output = [0i32; N];
            compute(&mut output, &mod_buf, carrier_phase, freq, gain, gain, false);
            for i in 0..N {
                mod_samples[block * N + i] = output[i] as f64;
            }
            carrier_phase = carrier_phase.wrapping_add(freq << LG_N);
        }

        // Measure spectral content: the modulated signal should have
        // energy at harmonics that the pure signal doesn't
        let cycles = 440.0 * (total_samples as f64 / 44100.0);
        let fund_bin = cycles.round() as usize;
        let dft_bin = |samples: &[f64], bin: usize| -> f64 {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (n, &s) in samples.iter().enumerate() {
                let angle = 2.0 * std::f64::consts::PI * bin as f64 * n as f64 / total_samples as f64;
                re += s * angle.cos();
                im += s * angle.sin();
            }
            (re * re + im * im).sqrt()
        };

        // FM spreads energy from fundamental into sidebands.
        // Measure ratio of fundamental to total harmonic energy.
        let pure_fund = dft_bin(&pure_samples, fund_bin);
        let mod_fund = dft_bin(&mod_samples, fund_bin);

        let mut pure_harm = 0.0f64;
        let mut mod_harm = 0.0f64;
        for h in 2..=8 {
            let bin = fund_bin * h;
            if bin < total_samples / 2 {
                let p = dft_bin(&pure_samples, bin);
                let m = dft_bin(&mod_samples, bin);
                pure_harm += p * p;
                mod_harm += m * m;
            }
        }

        // FM modulation should redistribute energy: the modulated signal
        // has a lower fundamental-to-harmonic ratio
        let pure_ratio = pure_fund / (pure_harm.sqrt() + 1.0);
        let mod_ratio = mod_fund / (mod_harm.sqrt() + 1.0);

        assert!(
            mod_ratio < pure_ratio,
            "FM should spread energy to harmonics: pure fund/harm ratio={pure_ratio:.1}, mod ratio={mod_ratio:.1}"
        );
    }

    // ========================================================================
    // 5. Feedback — compute_fb()
    // ========================================================================

    #[test]
    fn test_feedback_zero_shift_is_pure() {
        ensure_init();
        // With fb_shift=16 (effectively no feedback), compute_fb should
        // produce the same output as compute_pure
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;

        let mut pure_output = [0i32; N];
        compute_pure(&mut pure_output, 0, freq, gain, gain, false);

        let mut fb_output = [0i32; N];
        let mut fb_buf = [0i32; 2];
        compute_fb(&mut fb_output, 0, freq, gain, gain, &mut fb_buf, 16, false);

        // With fb_shift=16, feedback term = (y0+y)>>17 ≈ peak/1024 ≈ 65536.
        // This adds ~65k to phase, causing small output differences.
        let peak = (4096 + 4095) << 13;
        // Allow ~0.5% difference due to residual feedback
        let tolerance = peak / 200;
        for i in 0..N {
            let diff = (pure_output[i] - fb_output[i]).abs();
            assert!(
                diff < tolerance,
                "fb_shift=16 should ≈ pure sine at sample {i}: pure={}, fb={}, diff={diff} (tol={tolerance})",
                pure_output[i], fb_output[i]
            );
        }
    }

    #[test]
    fn test_feedback_adds_harmonics() {
        ensure_init();
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;

        let num_blocks = 32;
        let total_samples = N * num_blocks;

        // Pure sine
        let mut pure_samples = vec![0f64; total_samples];
        let mut phase = 0i32;
        for block in 0..num_blocks {
            let mut output = [0i32; N];
            compute_pure(&mut output, phase, freq, gain, gain, false);
            for i in 0..N {
                pure_samples[block * N + i] = output[i] as f64;
            }
            phase = phase.wrapping_add(freq << LG_N);
        }

        // With feedback (fb_shift = FEEDBACK_BITDEPTH - 7 = 1, high feedback)
        let mut fb_samples = vec![0f64; total_samples];
        let mut phase = 0i32;
        let mut fb_buf = [0i32; 2];
        for block in 0..num_blocks {
            let mut output = [0i32; N];
            compute_fb(&mut output, phase, freq, gain, gain, &mut fb_buf, 1, false);
            for i in 0..N {
                fb_samples[block * N + i] = output[i] as f64;
            }
            phase = phase.wrapping_add(freq << LG_N);
        }

        // The feedback version should have more harmonic content
        let cycles = 440.0 * (total_samples as f64 / 44100.0);
        let fund_bin = cycles.round() as usize;

        let dft_bin = |samples: &[f64], bin: usize| -> f64 {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (n, &s) in samples.iter().enumerate() {
                let angle = 2.0 * std::f64::consts::PI * bin as f64 * n as f64 / total_samples as f64;
                re += s * angle.cos();
                im += s * angle.sin();
            }
            (re * re + im * im).sqrt()
        };

        // Sum power in harmonics 2-6
        let mut pure_harmonic_power = 0.0f64;
        let mut fb_harmonic_power = 0.0f64;
        for h in 2..=6 {
            let bin = fund_bin * h;
            if bin < total_samples / 2 {
                let p = dft_bin(&pure_samples, bin);
                let f = dft_bin(&fb_samples, bin);
                pure_harmonic_power += p * p;
                fb_harmonic_power += f * f;
            }
        }

        assert!(
            fb_harmonic_power > pure_harmonic_power * 10.0,
            "Feedback should add significant harmonics: pure={pure_harmonic_power:.0}, fb={fb_harmonic_power:.0}"
        );
    }

    // ========================================================================
    // 6. Frequency accuracy
    // ========================================================================

    #[test]
    fn test_osc_freq_coarse_ratios() {
        // Coarse=1 should be 1:1, coarse=2 should be 2:1 (octave up)
        let note = 69; // A4
        let f1 = osc_freq(note, 0, 1, 0, 7); // 1x
        let f2 = osc_freq(note, 0, 2, 0, 7); // 2x
        let diff = f2 - f1;
        let octave = 1 << 24;
        assert!(
            (diff - octave).abs() < 2,
            "Coarse 2 vs 1 should be exactly one octave ({octave}), got {diff}"
        );
    }

    #[test]
    fn test_osc_freq_detune_center() {
        // Detune=7 is center (no detuning)
        let note = 69;
        let f_center = osc_freq(note, 0, 1, 0, 7);
        let f_up = osc_freq(note, 0, 1, 0, 8);
        let f_down = osc_freq(note, 0, 1, 0, 6);

        assert!(f_up > f_center, "Detune 8 should be higher than center");
        assert!(f_down < f_center, "Detune 6 should be lower than center");
        // Detuning should be small (< 1 semitone = 1<<24 / 12 ≈ 1398101)
        assert!(
            (f_up - f_center).abs() < 1398101,
            "Detune should be less than a semitone"
        );
    }

    #[test]
    fn test_osc_freq_coarse_half() {
        // Coarse=0 should be 0.5x (one octave down)
        let note = 69;
        let f0 = osc_freq(note, 0, 0, 0, 7); // 0.5x
        let f1 = osc_freq(note, 0, 1, 0, 7); // 1x
        let diff = f1 - f0;
        let octave = 1 << 24;
        assert!(
            (diff - octave).abs() < 2,
            "Coarse 0 (0.5x) vs 1 (1x) should be one octave ({octave}), got {diff}"
        );
    }

    // ========================================================================
    // 7. Scaling functions
    // ========================================================================

    #[test]
    fn test_scale_velocity_max() {
        // Max velocity (127), max sensitivity
        let val = scale_velocity(127, 7);
        // Should produce a positive contribution
        assert!(val > 0, "Max velocity should produce positive scaling, got {val}");
    }

    #[test]
    fn test_scale_velocity_zero() {
        // Zero velocity should produce negative scaling (quieter)
        let val = scale_velocity(0, 7);
        assert!(val < 0, "Zero velocity with sensitivity should produce negative scaling, got {val}");
    }

    #[test]
    fn test_scale_velocity_no_sensitivity() {
        // With sensitivity=0, velocity shouldn't matter
        let val_low = scale_velocity(0, 0);
        let val_high = scale_velocity(127, 0);
        assert_eq!(val_low, val_high, "With sensitivity=0, velocity should have no effect");
    }

    #[test]
    fn test_scale_level_at_breakpoint() {
        // At the breakpoint, scaling should be minimal
        let midinote = 39 + 17; // breakpoint 39 + offset 17 = note 56
        let val = scale_level(midinote, 39, 50, 50, 0, 0);
        // At/near breakpoint, scaling should be very small
        assert!(
            val.abs() < 20,
            "At breakpoint, scaling should be near 0, got {val}"
        );
    }
}
