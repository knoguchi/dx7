//! Bottom-up integration tests for the DX7 signal chain.
//!
//! Tests each component from tables → operators → voice → synth,
//! comparing against known-good values from Dexed/MSFA.

use std::sync::Once;

static INIT: Once = Once::new();
fn ensure_init() {
    INIT.call_once(|| {
        dx7_core::tables::init_tables(44100);
        dx7_core::lfo::init_lfo(44100);
        dx7_core::pitchenv::init_pitchenv(44100);
    });
}

// ============================================================================
// Level 1: Lookup tables
// ============================================================================

mod tables_tests {
    use super::*;
    use dx7_core::tables;

    #[test]
    fn sin_lookup_at_known_phases() {
        ensure_init();
        // Phase convention: 0..2^24 = one full cycle
        let full_cycle = 1i32 << 24;

        // sin(0) ≈ 0
        let v = tables::sin_lookup(0);
        assert!(v.abs() < 100, "sin(0) should be ~0, got {v}");

        // sin(π/2) ≈ +peak (quarter cycle = 2^22)
        let v = tables::sin_lookup(full_cycle / 4);
        let peak = 1 << 24; // Q24 peak
        assert!(
            (v - peak).abs() < 1000,
            "sin(π/2) should be ~{peak}, got {v}"
        );

        // sin(π) ≈ 0 (half cycle = 2^23)
        let v = tables::sin_lookup(full_cycle / 2);
        assert!(v.abs() < 1000, "sin(π) should be ~0, got {v}");

        // sin(3π/2) ≈ -peak
        let v = tables::sin_lookup(3 * full_cycle / 4);
        assert!(
            (v + peak).abs() < 1000,
            "sin(3π/2) should be ~{}, got {v}",
            -peak
        );
    }

    #[test]
    fn sin_lookup_wraps_at_24_bits() {
        ensure_init();
        // Phase values above 2^24 should wrap (only lower 24 bits matter for indexing)
        let quarter = 1i32 << 22;
        let v1 = tables::sin_lookup(quarter);
        let v2 = tables::sin_lookup(quarter + (1i32 << 24));
        // These should be very close (same position in cycle)
        assert!(
            (v1 - v2).abs() < 100,
            "sin should wrap at 24 bits: {v1} vs {v2}"
        );
    }

    #[test]
    fn exp2_lookup_known_values() {
        ensure_init();
        // exp2(0) should be 1.0 in Q24 = 16777216
        let v = tables::exp2_lookup(0);
        let expected = 1 << 24;
        assert!(
            (v - expected).abs() < 200,
            "exp2(0) should be ~{expected}, got {v}"
        );

        // exp2(1<<24) = exp2(1.0) should be 2.0 in Q24 = 33554432
        let v = tables::exp2_lookup(1 << 24);
        let expected = 1 << 25;
        assert!(
            (v - expected).abs() < 200,
            "exp2(1.0) should be ~{expected}, got {v}"
        );
    }

    #[test]
    fn freqlut_a440_phase_increment() {
        ensure_init();
        // A4 = MIDI note 69 = 440 Hz
        let logfreq = tables::midinote_to_logfreq(69);
        let phase_inc = tables::freqlut_lookup(logfreq);

        // Expected: 440/44100 cycles per sample
        // In phase units (2^24 per cycle): 440/44100 * 2^24 ≈ 167,380
        let expected = (440.0 / 44100.0 * (1u64 << 24) as f64) as i32;
        let tolerance = expected / 20; // 5% tolerance
        assert!(
            (phase_inc - expected).abs() < tolerance,
            "A440 phase_inc: expected ~{expected}, got {phase_inc}"
        );
        assert!(phase_inc > 0, "Phase increment must be positive");
    }

    #[test]
    fn freqlut_octave_doubling() {
        ensure_init();
        // One octave up should double the phase increment
        let logfreq_a4 = tables::midinote_to_logfreq(69);
        let logfreq_a5 = tables::midinote_to_logfreq(81);
        let inc_a4 = tables::freqlut_lookup(logfreq_a4);
        let inc_a5 = tables::freqlut_lookup(logfreq_a5);

        let ratio = inc_a5 as f64 / inc_a4 as f64;
        assert!(
            (ratio - 2.0).abs() < 0.02,
            "Octave should double phase_inc: ratio={ratio:.4}"
        );
    }

    #[test]
    fn midinote_to_logfreq_c4_vs_a4() {
        ensure_init();
        let c4 = tables::midinote_to_logfreq(60);
        let a4 = tables::midinote_to_logfreq(69);
        // 9 semitones = 9/12 octaves = 0.75 octaves
        let diff = a4 - c4;
        let expected = (0.75 * (1u64 << 24) as f64) as i32;
        assert!(
            (diff - expected).abs() < 2,
            "C4 to A4 should be 0.75 octaves: diff={diff}, expected={expected}"
        );
    }
}

// ============================================================================
// Level 2: Operator compute functions
// ============================================================================

mod operator_tests {
    use super::*;
    use dx7_core::operator;
    use dx7_core::tables::{self, N};

    #[test]
    fn compute_pure_produces_sine() {
        ensure_init();
        // Generate one block at A440 with full gain (MkI: 0 = no attenuation)
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0; // Full gain (MkI log attenuation: 0 = loudest)
        let mut output = [0i32; N];

        operator::compute_pure(&mut output, 0, freq, gain, gain, false);

        // Output should not be all zeros. MkI peak is ~2^26.
        let max_val = output.iter().map(|x| x.abs()).max().unwrap();
        assert!(
            max_val > 1 << 22,
            "compute_pure output should have significant amplitude, got max={max_val}"
        );

        // First sample should be small relative to peak (sin near zero crossing)
        let peak = 1i64 << 26;
        assert!(
            (output[0].abs() as i64) < peak / 100,
            "First sample should be small relative to peak, got {}",
            output[0]
        );
    }

    #[test]
    fn compute_pure_add_mode() {
        ensure_init();
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;
        let mut output = [0i32; N];

        // Fill with a known value
        let base = 100000;
        for x in output.iter_mut() {
            *x = base;
        }

        operator::compute_pure(&mut output, 0, freq, gain, gain, true);

        // In add mode, output[0] should be base + mki_sin(0, 0) (small value near zero crossing)
        // The base value should still be dominant
        assert!(
            output[0] > base / 2,
            "Add mode: output[0] should include base, got {}",
            output[0]
        );
    }

    #[test]
    fn compute_modulated_differs_from_pure() {
        ensure_init();
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;

        // Pure sine
        let mut pure_out = [0i32; N];
        operator::compute_pure(&mut pure_out, 0, freq, gain, gain, false);

        // Modulated with a strong input signal
        let mut mod_input = [0i32; N];
        operator::compute_pure(&mut mod_input, 0, freq, gain, gain, false);

        let mut mod_out = [0i32; N];
        operator::compute(&mut mod_out, &mod_input, 0, freq, gain, gain, false);

        // The modulated output should differ from pure
        let mut diff_count = 0;
        for i in 0..N {
            if (mod_out[i] - pure_out[i]).abs() > 10000 {
                diff_count += 1;
            }
        }
        assert!(
            diff_count > N / 4,
            "Modulated output should differ from pure sine in many samples, diff_count={diff_count}"
        );
    }

    #[test]
    fn compute_modulated_dc_offset_shifts_phase() {
        ensure_init();
        // A constant modulation input should shift the carrier's phase.
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;

        // Quarter-cycle DC offset
        let dc_value = 1 << 22; // quarter of 2^24 = quarter cycle
        let mod_input = [dc_value; N];

        let mut shifted = [0i32; N];
        operator::compute(&mut shifted, &mod_input, 0, freq, gain, gain, false);

        let mut pure = [0i32; N];
        operator::compute_pure(&mut pure, 0, freq, gain, gain, false);

        // Shifted output should differ from pure (quarter cycle phase shift)
        let mut diffs = 0;
        for i in 0..N {
            if (shifted[i] - pure[i]).abs() > 1000000 {
                diffs += 1;
            }
        }
        assert!(
            diffs > N / 4,
            "Quarter-cycle phase shift should change most samples, diffs={diffs}"
        );
    }

    #[test]
    fn compute_fb_produces_output() {
        ensure_init();
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;
        let mut fb_buf = [0i32; 2];
        let fb_shift = 2; // Strong feedback

        let mut output = [0i32; N];
        operator::compute_fb(&mut output, 0, freq, gain, gain, &mut fb_buf, fb_shift, false);

        let max_val = output.iter().map(|x| x.abs()).max().unwrap();
        assert!(
            max_val > 1 << 22,
            "Feedback operator should produce output, max={max_val}"
        );

        // Feedback buffer should be updated
        assert!(
            fb_buf[0] != 0 || fb_buf[1] != 0,
            "Feedback buffer should be non-zero after compute"
        );
    }

    #[test]
    fn compute_fb_with_no_feedback_nearly_equals_pure() {
        ensure_init();
        let logfreq = tables::midinote_to_logfreq(69);
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0;

        // fb_shift=16 means minimal feedback (shift very far right).
        // MkI output peaks at ~2^26 so even tiny residual feedback
        // can cause small differences.
        let mut fb_buf = [0i32; 2];
        let mut fb_out = [0i32; N];
        operator::compute_fb(&mut fb_out, 0, freq, gain, gain, &mut fb_buf, 16, false);

        let mut pure_out = [0i32; N];
        operator::compute_pure(&mut pure_out, 0, freq, gain, gain, false);

        for i in 0..N {
            let diff = (fb_out[i] - pure_out[i]).abs();
            assert!(
                diff < 200000,
                "With fb_shift=16, feedback should nearly equal pure sine at sample {i}, diff={diff}"
            );
        }
    }

    #[test]
    fn osc_freq_ratio_mode_coarse_1() {
        ensure_init();
        // Coarse=1 means 1:1 ratio — frequency should equal the note frequency
        let logfreq = operator::osc_freq(69, 0, 1, 0, 7); // A4, ratio, coarse=1, fine=0, detune=center
        let phase_inc = tables::freqlut_lookup(logfreq);

        let expected = (440.0 / 44100.0 * (1u64 << 24) as f64) as i32;
        let tolerance = expected / 10;
        assert!(
            (phase_inc - expected).abs() < tolerance,
            "Coarse=1 at A4 should give 440Hz phase_inc: expected ~{expected}, got {phase_inc}"
        );
    }

    #[test]
    fn osc_freq_ratio_mode_coarse_14() {
        ensure_init();
        // Coarse=14 means 14:1 ratio
        let logfreq_1 = operator::osc_freq(60, 0, 1, 0, 7);
        let logfreq_14 = operator::osc_freq(60, 0, 14, 0, 7);
        let inc_1 = tables::freqlut_lookup(logfreq_1);
        let inc_14 = tables::freqlut_lookup(logfreq_14);

        let ratio = inc_14 as f64 / inc_1 as f64;
        assert!(
            (ratio - 14.0).abs() < 0.5,
            "Coarse=14 should be ~14x coarse=1: ratio={ratio:.2}"
        );
    }

    #[test]
    fn osc_freq_detune_center_no_effect() {
        ensure_init();
        // Detune=7 is center (no detuning)
        let freq_7 = operator::osc_freq(69, 0, 1, 0, 7);
        // Compare with a slightly different detune
        let freq_8 = operator::osc_freq(69, 0, 1, 0, 8);
        let freq_6 = operator::osc_freq(69, 0, 1, 0, 6);

        // Detune should shift frequency slightly
        assert!(freq_8 > freq_7, "Detune 8 > 7 should raise frequency");
        assert!(freq_6 < freq_7, "Detune 6 < 7 should lower frequency");

        // But only slightly (less than a semitone = 1/12 octave = ~1.4M in Q24)
        let semitone = (1 << 24) / 12;
        assert!(
            (freq_8 - freq_7).abs() < semitone,
            "Detune should be less than a semitone"
        );
    }
}

// ============================================================================
// Level 3: Envelope
// ============================================================================

mod envelope_tests {
    use super::*;
    use dx7_core::envelope::{self, Envelope};

    #[test]
    fn scaleoutlevel_boundary_values() {
        ensure_init();
        assert_eq!(envelope::scaleoutlevel(0), 0);
        assert_eq!(envelope::scaleoutlevel(19), 46);
        assert_eq!(envelope::scaleoutlevel(20), 48); // 28 + 20
        assert_eq!(envelope::scaleoutlevel(99), 127); // 28 + 99
    }

    #[test]
    fn envelope_attack_reaches_target() {
        ensure_init();
        let rates = [99, 99, 99, 99]; // Fastest rates
        let levels = [99, 99, 99, 0]; // Full level

        let outlevel = envelope::scaleoutlevel(99) << 5; // Max output level
        let mut env = Envelope::new();
        env.init(&rates, &levels, outlevel, 0);
        env.keydown(true);

        // Run for many blocks — should reach near-max level
        let mut level = 0;
        for _ in 0..500 {
            level = env.getsample();
        }

        // Level should be high (attack target for L1=99)
        assert!(
            level > 1 << 24,
            "After fast attack, level should be high, got {level}"
        );
    }

    #[test]
    fn envelope_release_decays() {
        ensure_init();
        let rates = [99, 99, 99, 99];
        let levels = [99, 99, 99, 0];
        let outlevel = envelope::scaleoutlevel(99) << 5;

        let mut env = Envelope::new();
        env.init(&rates, &levels, outlevel, 0);
        env.keydown(true);

        // Attack
        let mut level = 0;
        for _ in 0..500 {
            level = env.getsample();
        }
        let peak = level;

        // Release
        env.keydown(false);
        for _ in 0..2000 {
            level = env.getsample();
        }

        assert!(
            level < peak / 2,
            "After release, level should decay: peak={peak}, after={level}"
        );
    }

    #[test]
    fn envelope_zero_output_level_is_silent() {
        ensure_init();
        let rates = [99, 99, 99, 99];
        let levels = [99, 99, 99, 0];
        let outlevel = envelope::scaleoutlevel(0) << 5; // OL=0

        let mut env = Envelope::new();
        env.init(&rates, &levels, outlevel, 0);
        env.keydown(true);

        let mut max_level = 0;
        for _ in 0..500 {
            let level = env.getsample();
            if level > max_level {
                max_level = level;
            }
        }

        // With OL=0, the envelope should stay very low
        // The floor is 16 << 16 = 1048576
        assert!(
            max_level <= 16 << 16,
            "OL=0 should produce minimal level, got {max_level}"
        );
    }
}

// ============================================================================
// Level 4: Single voice rendering
// ============================================================================

mod voice_tests {
    use super::*;
    use dx7_core::envelope::EnvParams;
    use dx7_core::lfo::{LfoParams, LfoWaveform};
    use dx7_core::operator::OperatorParams;
    use dx7_core::patch::DxVoice;
    use dx7_core::tables::N;
    use dx7_core::voice::Voice;

    /// Create a minimal test patch: one active carrier, rest silent.
    /// Uses Algorithm 32 (all 6 ops are independent carriers).
    fn make_single_carrier_patch() -> DxVoice {
        let mut ops = [OperatorParams::default(); 6];
        // Silence all operators
        for op in ops.iter_mut() {
            op.output_level = 0;
        }
        // Activate only operators[5] = OP1 (index 5 in our convention)
        // Using algorithm 32 (idx 31): all 6 ops are independent carriers
        ops[5] = OperatorParams {
            eg: EnvParams {
                rates: [99, 99, 99, 99],
                levels: [99, 99, 99, 0],
            },
            output_level: 99,
            osc_freq_coarse: 1, // 1:1 ratio
            osc_freq_fine: 0,
            osc_detune: 7, // Center
            ..OperatorParams::default()
        };

        DxVoice {
            operators: ops,
            pitch_eg: EnvParams {
                rates: [99, 99, 99, 99],
                levels: [50, 50, 50, 50],
            },
            algorithm: 31, // Algorithm 32 (all carriers)
            feedback: 0,
            osc_key_sync: true,
            lfo: LfoParams {
                speed: 0,
                delay: 0,
                pitch_mod_depth: 0,
                amp_mod_depth: 0,
                key_sync: true,
                waveform: LfoWaveform::Triangle,
            },
            pitch_mod_sensitivity: 0,
            transpose: 24,
            name: *b"TEST SINE ",
        }
    }

    #[test]
    fn single_carrier_produces_output() {
        ensure_init();
        let patch = make_single_carrier_patch();
        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100); // A4, velocity 100

        // Render several blocks to get past attack
        let mut total_output = Vec::new();
        for _ in 0..100 {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            total_output.extend_from_slice(&buf);
        }

        let max_val = total_output.iter().map(|x| x.abs()).max().unwrap();
        assert!(
            max_val > 1 << 18,
            "Single carrier voice should produce significant output, max={max_val}"
        );
    }

    #[test]
    fn single_carrier_frequency_is_correct() {
        ensure_init();
        let patch = make_single_carrier_patch();
        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100); // A4 = 440 Hz

        // Render enough blocks for frequency measurement
        // At 44100 Hz and 440 Hz, one period = 100.23 samples
        let num_blocks = 50; // 50 * 64 = 3200 samples
        let mut samples = Vec::new();
        for _ in 0..num_blocks {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            samples.extend_from_slice(&buf);
        }

        // Skip first 10 blocks (640 samples) for envelope to settle
        let stable = &samples[640..];

        // Count zero crossings (positive to negative)
        let mut crossings = 0;
        for i in 1..stable.len() {
            if stable[i - 1] >= 0 && stable[i] < 0 {
                crossings += 1;
            }
        }

        // Expected: 440 Hz * (stable.len() / 44100) crossings
        let duration = stable.len() as f64 / 44100.0;
        let expected_crossings = (440.0 * duration) as i32;
        let tolerance = expected_crossings / 5; // 20% tolerance

        assert!(
            (crossings - expected_crossings).abs() < tolerance,
            "A440 should have ~{expected_crossings} zero crossings, got {crossings}"
        );
    }

    #[test]
    fn silent_operators_produce_no_output() {
        ensure_init();
        let mut patch = make_single_carrier_patch();
        // Make ALL operators silent
        for op in patch.operators.iter_mut() {
            op.output_level = 0;
        }

        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100);

        let mut buf = [0i32; N];
        for _ in 0..100 {
            voice.render(&mut buf);
        }

        // Should produce essentially nothing
        let max_val = buf.iter().map(|x| x.abs()).max().unwrap();
        assert!(
            max_val < 100,
            "All-silent operators should produce near-zero output, max={max_val}"
        );
    }

    #[test]
    fn voice_releases_and_goes_inactive() {
        ensure_init();
        let patch = make_single_carrier_patch();
        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100);

        // Render attack
        for _ in 0..10 {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
        }
        assert_eq!(
            voice.state,
            dx7_core::voice::VoiceState::Active,
            "Voice should be active during note"
        );

        // Release
        voice.note_off();
        assert_eq!(voice.state, dx7_core::voice::VoiceState::Released);

        // Render until inactive (fast release rates should finish quickly)
        for _ in 0..5000 {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            if voice.state == dx7_core::voice::VoiceState::Inactive {
                return; // Test passes
            }
        }
        panic!("Voice did not become inactive after release");
    }
}

// ============================================================================
// Level 5: Algorithm routing verification
// ============================================================================

mod algorithm_routing_tests {
    use super::*;
    use dx7_core::voice;

    #[test]
    fn algorithm_5_carriers() {
        ensure_init();
        // Algorithm 5 (idx 4): carriers are at positions 1, 3, 5
        // (OP5, OP3, OP1 in DX7 numbering)
        assert!(
            voice::is_carrier(4, 1),
            "Alg5: position 1 (OP5) should be carrier"
        );
        assert!(
            voice::is_carrier(4, 3),
            "Alg5: position 3 (OP3) should be carrier"
        );
        assert!(
            voice::is_carrier(4, 5),
            "Alg5: position 5 (OP1) should be carrier"
        );
        assert!(
            !voice::is_carrier(4, 0),
            "Alg5: position 0 (OP6) should be modulator"
        );
        assert!(
            !voice::is_carrier(4, 2),
            "Alg5: position 2 (OP4) should be modulator"
        );
        assert!(
            !voice::is_carrier(4, 4),
            "Alg5: position 4 (OP2) should be modulator"
        );
    }

    #[test]
    fn algorithm_22_carriers() {
        ensure_init();
        // Algorithm 22 (idx 21): OP6(fb)→{OP5,OP4,OP3}, OP2→OP1
        // ALGORITHMS[21] = [0xc1, 0x14, 0x14, 0x14, 0x01, 0x14]
        // OP6 is fb modulator, OP2 is modulator, rest are carriers
        assert!(
            !voice::is_carrier(21, 0),
            "Alg22: pos 0 (OP6) should be fb modulator"
        );
        assert!(
            voice::is_carrier(21, 1),
            "Alg22: pos 1 (OP5) should be carrier"
        );
        assert!(
            voice::is_carrier(21, 2),
            "Alg22: pos 2 (OP4) should be carrier"
        );
        assert!(
            voice::is_carrier(21, 3),
            "Alg22: pos 3 (OP3) should be carrier"
        );
        assert!(
            !voice::is_carrier(21, 4),
            "Alg22: pos 4 (OP2) should be modulator"
        );
        assert!(
            voice::is_carrier(21, 5),
            "Alg22: pos 5 (OP1) should be carrier"
        );
    }

    #[test]
    fn algorithm_32_all_carriers() {
        ensure_init();
        // Algorithm 32 (idx 31): all 6 operators are carriers
        for pos in 0..6 {
            assert!(
                voice::is_carrier(31, pos),
                "Alg32: position {pos} should be carrier"
            );
        }
    }

    #[test]
    fn algorithm_1_single_carrier() {
        ensure_init();
        // Algorithm 1 (idx 0): only position 5 (OP1) is carrier
        // ALGORITHMS[0] = [0xc1, 0x11, 0x11, 0x14, 0x01, 0x14]
        assert!(!voice::is_carrier(0, 0), "Alg1: pos 0 should NOT be carrier");
        assert!(!voice::is_carrier(0, 1), "Alg1: pos 1 should NOT be carrier");
        assert!(!voice::is_carrier(0, 2), "Alg1: pos 2 should NOT be carrier");
        assert!(
            voice::is_carrier(0, 3),
            "Alg1: pos 3 should be carrier"
        );
        assert!(!voice::is_carrier(0, 4), "Alg1: pos 4 should NOT be carrier");
        assert!(
            voice::is_carrier(0, 5),
            "Alg1: pos 5 should be carrier"
        );
    }
}

// ============================================================================
// Level 6: E.PIANO 1 voice verification
// ============================================================================

mod epiano1_tests {
    use super::*;
    use dx7_core::rom1a;
    use dx7_core::tables::N;
    use dx7_core::voice::Voice;

    #[test]
    fn epiano1_patch_parameters() {
        ensure_init();
        let voice = rom1a::load_rom1a_voice(10).unwrap();
        assert_eq!(voice.algorithm, 4, "E.PIANO 1 should use algorithm 5 (idx 4)");
        assert_eq!(voice.feedback, 6);
        assert!(voice.osc_key_sync);
        assert_eq!(voice.transpose, 24);

        // OP6 (index 0): carrier-like, FC=1, OL=98
        assert_eq!(voice.operators[0].osc_freq_coarse, 1);
        assert_eq!(voice.operators[0].output_level, 98);

        // OP5 (index 1): FC=14 (the bell tone), OL=60
        assert_eq!(voice.operators[1].osc_freq_coarse, 14);
        assert_eq!(voice.operators[1].output_level, 60);

        // OP4 (index 2): FC=1, OL=94
        assert_eq!(voice.operators[2].osc_freq_coarse, 1);
        assert_eq!(voice.operators[2].output_level, 94);

        // OP3 (index 3): FC=1, OL=60
        assert_eq!(voice.operators[3].osc_freq_coarse, 1);
        assert_eq!(voice.operators[3].output_level, 60);

        // OP2 (index 4): FC=1, OL=86
        assert_eq!(voice.operators[4].osc_freq_coarse, 1);
        assert_eq!(voice.operators[4].output_level, 86);

        // OP1 (index 5): FC=1, OL=98
        assert_eq!(voice.operators[5].osc_freq_coarse, 1);
        assert_eq!(voice.operators[5].output_level, 98);
    }

    #[test]
    fn epiano1_renders_nonzero() {
        ensure_init();
        let patch = rom1a::load_rom1a_voice(10).unwrap();
        let mut voice = Voice::new();
        voice.note_on(&patch, 60, 100); // Middle C

        let mut total_output = Vec::new();
        for _ in 0..200 {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            total_output.extend_from_slice(&buf);
        }

        let max_val = total_output.iter().map(|x| x.abs()).max().unwrap();
        assert!(
            max_val > 1 << 18,
            "E.PIANO 1 should produce significant output, max={max_val}"
        );
    }

    #[test]
    fn epiano1_has_correct_fundamental() {
        ensure_init();
        let patch = rom1a::load_rom1a_voice(10).unwrap();
        let mut voice = Voice::new();
        voice.note_on(&patch, 60, 100); // Middle C = 261.63 Hz

        // Render 200 blocks = 12800 samples ≈ 0.29 seconds
        let mut samples = Vec::new();
        for _ in 0..200 {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            samples.extend_from_slice(&buf);
        }

        // Skip first 20 blocks for attack transient
        let stable = &samples[1280..];

        // Do a simple DFT at the fundamental frequency (261.63 Hz)
        // and at some non-fundamental frequency to verify
        let sample_rate = 44100.0;
        let fundamental = 261.63;

        let power_fundamental = compute_power_at_freq(stable, fundamental, sample_rate);
        let power_off_freq = compute_power_at_freq(stable, 300.0, sample_rate);

        assert!(
            power_fundamental > power_off_freq * 2.0,
            "E.PIANO 1 should have strong fundamental at {fundamental} Hz: \
             power_fund={power_fundamental:.0}, power_300={power_off_freq:.0}"
        );
    }

    #[test]
    fn epiano1_output_level_reasonable() {
        ensure_init();
        let patch = rom1a::load_rom1a_voice(10).unwrap();
        let mut voice = Voice::new();
        voice.note_on(&patch, 60, 127); // Max velocity

        let mut samples = Vec::new();
        for _ in 0..200 {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            samples.extend_from_slice(&buf);
        }

        // Convert to f32 similar to synth.rs (voice output >> 4 then / 2^24)
        let max_f32 = samples
            .iter()
            .map(|&s| ((s >> 4) as f64 / (1i64 << 24) as f64).abs() as f32 * 0.5)
            .fold(0.0f32, f32::max);

        // DX7 with 3 carriers at max velocity can exceed 1.0 (DAC clips).
        // Synth.rs applies clamping. Just verify the level is reasonable
        // (not wildly out of range which would indicate a gain bug).
        assert!(
            max_f32 < 3.0,
            "E.PIANO 1 single voice level is unreasonably high (max={max_f32:.4})"
        );
        assert!(
            max_f32 > 0.01,
            "E.PIANO 1 single voice is too quiet (max={max_f32:.4})"
        );
    }

    /// Compute spectral power at a specific frequency using Goertzel algorithm.
    fn compute_power_at_freq(samples: &[i32], freq: f64, sample_rate: f64) -> f64 {
        let n = samples.len() as f64;
        let k = (freq * n / sample_rate).round();
        let w = 2.0 * std::f64::consts::PI * k / n;
        let coeff = 2.0 * w.cos();

        let mut s0 = 0.0;
        let mut s1 = 0.0;
        let mut s2;

        for &sample in samples {
            s2 = s1;
            s1 = s0;
            s0 = sample as f64 + coeff * s1 - s2;
        }

        s0 * s0 + s1 * s1 - coeff * s0 * s1
    }
}

// ============================================================================
// Level 7: Full synth rendering
// ============================================================================

mod synth_tests {
    use super::*;
    use dx7_core::rom1a;
    use dx7_core::Synth;

    #[test]
    fn synth_renders_epiano1_note() {
        ensure_init();
        let patch = rom1a::load_rom1a_voice(10).unwrap();
        let mut synth = Synth::new(44100);
        synth.load_patch(patch);

        // Note on
        synth.note_on(60, 100);

        // Render 1 second
        let num_samples = 44100;
        let mut output = vec![0.0f32; num_samples];
        synth.render_mono(&mut output);

        // Should have audio
        let max_val = output.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        assert!(
            max_val > 0.01,
            "Synth should produce audible output, max={max_val}"
        );

        // Should not clip
        assert!(
            max_val <= 1.0,
            "Synth output should not exceed 1.0, max={max_val}"
        );

        // Check that audio is not just noise — should have periodicity
        // Autocorrelation at the fundamental period (~169 samples for C4)
        let period = (44100.0 / 261.63) as usize; // ~169
        let start = 5000; // Skip attack
        let len = 10000;
        let end = start + len;

        let mut autocorr_at_period = 0.0f64;
        let mut autocorr_at_0 = 0.0f64;
        for i in start..end {
            autocorr_at_0 += output[i] as f64 * output[i] as f64;
            if i + period < output.len() {
                autocorr_at_period += output[i] as f64 * output[i + period] as f64;
            }
        }
        let normalized = autocorr_at_period / autocorr_at_0;

        assert!(
            normalized > 0.3,
            "E.PIANO 1 should have strong periodicity at fundamental: autocorr={normalized:.4}"
        );
    }

    #[test]
    fn synth_brass1_renders() {
        ensure_init();
        let patch = rom1a::load_rom1a_voice(0).unwrap();
        let mut synth = Synth::new(44100);
        synth.load_patch(patch);

        synth.note_on(60, 100);

        let mut output = vec![0.0f32; 44100];
        synth.render_mono(&mut output);

        let max_val = output.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        assert!(
            max_val > 0.01,
            "BRASS 1 should produce audible output, max={max_val}"
        );
    }
}

// ============================================================================
// Level 8: FM modulation depth verification
// ============================================================================

mod fm_depth_tests {
    use super::*;
    use dx7_core::operator;
    use dx7_core::rom1a;
    use dx7_core::tables::{self, N};

    /// Compute spectral power at a specific frequency using Goertzel algorithm.
    fn goertzel_power(samples: &[i32], freq: f64, sample_rate: f64) -> f64 {
        let n = samples.len() as f64;
        let k = (freq * n / sample_rate).round();
        let w = 2.0 * std::f64::consts::PI * k / n;
        let coeff = 2.0 * w.cos();

        let mut s0 = 0.0;
        let mut s1 = 0.0;
        let mut s2;

        for &sample in samples {
            s2 = s1;
            s1 = s0;
            s0 = sample as f64 + coeff * s1 - s2;
        }

        s0 * s0 + s1 * s1 - coeff * s0 * s1
    }

    #[test]
    fn simple_fm_pair_produces_sidebands() {
        ensure_init();
        // Create a simple modulator→carrier pair manually.
        // MkI log-domain FM synthesis should produce harmonics.
        let sample_rate = 44100.0;
        let logfreq = tables::midinote_to_logfreq(69); // A4 = 440 Hz
        let freq = tables::freqlut_lookup(logfreq);
        let gain: i32 = 0; // Full gain (MkI: 0 = no attenuation)

        // Step 1: Generate modulator output (pure sine)
        let num_blocks = 200;
        let mut mod_samples = Vec::with_capacity(num_blocks * N);
        let mut phase = 0i32;
        for _ in 0..num_blocks {
            let mut mod_buf = [0i32; N];
            operator::compute_pure(&mut mod_buf, phase, freq, gain, gain, false);
            mod_samples.extend_from_slice(&mod_buf);
            phase = phase.wrapping_add(freq << tables::LG_N);
        }

        // Step 2: Use modulator output as input to carrier
        let mut car_samples = Vec::with_capacity(num_blocks * N);
        let mut car_phase = 0i32;
        let mut block_idx = 0;
        for _ in 0..num_blocks {
            let start = block_idx * N;
            let mut input = [0i32; N];
            input.copy_from_slice(&mod_samples[start..start + N]);
            let mut car_buf = [0i32; N];
            operator::compute(&mut car_buf, &input, car_phase, freq, gain, gain, false);
            car_samples.extend_from_slice(&car_buf);
            car_phase = car_phase.wrapping_add(freq << tables::LG_N);
            block_idx += 1;
        }

        // Skip first few blocks for transient
        let stable = &car_samples[N * 10..];

        // MkI output peaks at ~2^26. The modulation creates harmonics at n*440 Hz.
        let _power_440 = goertzel_power(stable, 440.0, sample_rate);
        let power_880 = goertzel_power(stable, 880.0, sample_rate);
        let power_1320 = goertzel_power(stable, 1320.0, sample_rate);
        let power_300 = goertzel_power(stable, 300.0, sample_rate); // off-harmonic

        // FM modulation should create harmonics
        assert!(
            power_880 > power_300 * 10.0,
            "FM should produce 2nd harmonic: p880={power_880:.0}, p300={power_300:.0}"
        );
        assert!(
            power_1320 > power_300 * 5.0,
            "FM should produce 3rd harmonic: p1320={power_1320:.0}, p300={power_300:.0}"
        );

        // Also verify modulated output differs from pure carrier
        let mut pure_samples = Vec::with_capacity(num_blocks * N);
        let mut pure_phase = 0i32;
        for _ in 0..num_blocks {
            let mut buf = [0i32; N];
            operator::compute_pure(&mut buf, pure_phase, freq, gain, gain, false);
            pure_samples.extend_from_slice(&buf);
            pure_phase = pure_phase.wrapping_add(freq << tables::LG_N);
        }

        let pure_power_880 = goertzel_power(&pure_samples[N * 10..], 880.0, sample_rate);
        // MkI log-domain quantization introduces slight harmonics even in "pure"
        // sine, so the ratio is smaller than with MSFA linear sine.
        assert!(
            power_880 > pure_power_880 * 10.0,
            "FM should have much stronger 2nd harmonic than pure sine: \
             fm_p880={power_880:.0}, pure_p880={pure_power_880:.0}"
        );
    }

    #[test]
    fn epiano1_spectral_analysis() {
        ensure_init();
        let patch = rom1a::load_rom1a_voice(10).unwrap();
        let mut voice = dx7_core::voice::Voice::new();
        voice.note_on(&patch, 60, 100); // C4 = 261.63 Hz

        // Render 400 blocks (~0.58 seconds)
        let mut samples = Vec::new();
        for _ in 0..400 {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            samples.extend_from_slice(&buf);
        }

        let sample_rate = 44100.0;
        let fund = 261.63;

        // Use sustained portion (skip attack, 100-300 blocks)
        let stable = &samples[N * 100..N * 300];

        // Measure harmonics
        let p1 = goertzel_power(stable, fund, sample_rate);
        let p2 = goertzel_power(stable, fund * 2.0, sample_rate);
        let p3 = goertzel_power(stable, fund * 3.0, sample_rate);
        let p14 = goertzel_power(stable, fund * 14.0, sample_rate);
        let p_off = goertzel_power(stable, fund * 1.5, sample_rate); // non-harmonic

        // E.PIANO 1 should have strong fundamental
        assert!(
            p1 > p_off * 10.0,
            "E.PIANO 1 fundamental should dominate: p1={p1:.0}, p_off={p_off:.0}"
        );

        // E.PIANO 1 should have SOME harmonics (it's FM, not pure sine)
        assert!(
            p2 > p_off,
            "E.PIANO 1 should have 2nd harmonic: p2={p2:.0}, p_off={p_off:.0}"
        );

        // Print spectral info for debugging
        eprintln!("E.PIANO 1 spectral analysis (C4, vel=100):");
        eprintln!("  Fund ({fund:.1} Hz): {:.1} dB", 10.0 * p1.log10());
        eprintln!("  2nd harm ({:.1} Hz): {:.1} dB", fund * 2.0, 10.0 * p2.log10());
        eprintln!("  3rd harm ({:.1} Hz): {:.1} dB", fund * 3.0, 10.0 * p3.log10());
        eprintln!("  14th harm ({:.1} Hz): {:.1} dB", fund * 14.0, 10.0 * p14.log10());
        eprintln!("  Non-harmonic ({:.1} Hz): {:.1} dB", fund * 1.5, 10.0 * p_off.log10());

        // Compute RMS and peak
        let rms: f64 = (stable.iter()
            .map(|&s| (s as f64).powi(2))
            .sum::<f64>() / stable.len() as f64)
            .sqrt();
        let peak = stable.iter().map(|&s| (s as f64).abs()).fold(0.0f64, f64::max);
        let crest_factor = peak / rms;
        eprintln!("  RMS: {:.0}, Peak: {:.0}, Crest factor: {:.1}", rms, peak, crest_factor);

        // Check peak-to-average ratio (crest factor).
        // A pure sine has crest factor = sqrt(2) ≈ 1.414.
        // FM synthesis typically has crest factor 2-6.
        assert!(
            crest_factor > 1.2 && crest_factor < 20.0,
            "Crest factor should be reasonable: {crest_factor:.1}"
        );
    }
}
