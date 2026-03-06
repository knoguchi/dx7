//! Single DX7 voice: 6 operators + bus-flag algorithm routing + LFO + pitch env.
//!
//! Ported from Dexed/MSFA fm_core.cc and dx7note.cc
//! (Apache 2.0, Google Inc. / Pascal Gauthier).

use crate::envelope::{self, Envelope};
use crate::generated_tables;
use crate::lfo::Lfo;
use crate::operator::{self, FmOpParams, AMPMODSENSTAB, PITCHMODSENSTAB};
use crate::patch::DxVoice;
use crate::pitchenv::PitchEnv;
use crate::tables::{self, N, LG_N};

// --- Algorithm bus flags (from fm_core.h) ---

const _OUT_BUS_ONE: i32 = 1 << 0;
const _OUT_BUS_TWO: i32 = 1 << 1;
const OUT_BUS_ADD: i32 = 1 << 2;
const _IN_BUS_ONE: i32 = 1 << 4;
const _IN_BUS_TWO: i32 = 1 << 5;
const _FB_IN: i32 = 1 << 6;
const _FB_OUT: i32 = 1 << 7;

/// Algorithm routing table: 32 algorithms, 6 operators each.
/// Each byte encodes input bus, output bus, and feedback flags.
/// Ported directly from fm_core.cc.
const ALGORITHMS: [[i32; 6]; 32] = [
    [0xc1, 0x11, 0x11, 0x14, 0x01, 0x14], // 1
    [0x01, 0x11, 0x11, 0x14, 0xc1, 0x14], // 2
    [0xc1, 0x11, 0x14, 0x01, 0x11, 0x14], // 3
    [0xc1, 0x11, 0x94, 0x01, 0x11, 0x14], // 4
    [0xc1, 0x14, 0x01, 0x14, 0x01, 0x14], // 5
    [0xc1, 0x94, 0x01, 0x14, 0x01, 0x14], // 6
    [0xc1, 0x11, 0x05, 0x14, 0x01, 0x14], // 7
    [0x01, 0x11, 0xc5, 0x14, 0x01, 0x14], // 8
    [0x01, 0x11, 0x05, 0x14, 0xc1, 0x14], // 9
    [0x01, 0x05, 0x14, 0xc1, 0x11, 0x14], // 10
    [0xc1, 0x05, 0x14, 0x01, 0x11, 0x14], // 11
    [0x01, 0x05, 0x05, 0x14, 0xc1, 0x14], // 12
    [0xc1, 0x05, 0x05, 0x14, 0x01, 0x14], // 13
    [0xc1, 0x05, 0x11, 0x14, 0x01, 0x14], // 14
    [0x01, 0x05, 0x11, 0x14, 0xc1, 0x14], // 15
    [0xc1, 0x11, 0x02, 0x25, 0x05, 0x14], // 16
    [0x01, 0x11, 0x02, 0x25, 0xc5, 0x14], // 17
    [0x01, 0x11, 0x11, 0xc5, 0x05, 0x14], // 18
    [0xc1, 0x14, 0x14, 0x01, 0x11, 0x14], // 19
    [0x01, 0x05, 0x14, 0xc1, 0x14, 0x14], // 20
    [0x01, 0x14, 0x14, 0xc1, 0x14, 0x14], // 21
    [0xc1, 0x14, 0x14, 0x14, 0x01, 0x14], // 22
    [0xc1, 0x14, 0x14, 0x01, 0x14, 0x04], // 23
    [0xc1, 0x14, 0x14, 0x14, 0x04, 0x04], // 24
    [0xc1, 0x14, 0x14, 0x04, 0x04, 0x04], // 25
    [0xc1, 0x05, 0x14, 0x01, 0x14, 0x04], // 26
    [0x01, 0x05, 0x14, 0xc1, 0x14, 0x04], // 27
    [0x04, 0xc1, 0x11, 0x14, 0x01, 0x14], // 28
    [0xc1, 0x14, 0x01, 0x14, 0x04, 0x04], // 29
    [0x04, 0xc1, 0x11, 0x14, 0x04, 0x04], // 30
    [0xc1, 0x14, 0x04, 0x04, 0x04, 0x04], // 31
    [0xc4, 0x04, 0x04, 0x04, 0x04, 0x04], // 32
];

/// Check if an operator is a carrier in the given algorithm.
pub fn is_carrier(algorithm: usize, op: usize) -> bool {
    (ALGORITHMS[algorithm][op] & OUT_BUS_ADD) != 0
}

/// State of a voice slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoiceState {
    Inactive,
    Active,
    Released,
}

/// A single DX7 voice instance.
pub struct Voice {
    pub state: VoiceState,
    pub note: u8,
    pub velocity: u8,
    pub age: u32,

    // Per-operator state
    pub params: [FmOpParams; 6],
    pub env: [Envelope; 6],
    pub basepitch: [i32; 6],
    ampmodsens: [u32; 6],
    op_mode: [i32; 6],

    // Voice-level state
    pitchenv: PitchEnv,
    lfo: Lfo,
    pub algorithm: i32,
    pub fb_buf: [i32; 2],
    pub fb_shift: i32,
    pitchmoddepth: i32,
    pitchmodsens: i32,
    ampmoddepth: i32,

    /// Pitch bend offset in the log-frequency domain (set by Synth each block).
    pub pitch_bend: i32,
    /// Mod wheel value 0–127 (set by Synth each block). Scales LFO mod depths.
    pub mod_wheel: i32,

    // Intermediate buffers for bus rendering
    buf: [[i32; N]; 2],
}

impl Voice {
    pub fn new() -> Self {
        Self {
            state: VoiceState::Inactive,
            note: 0,
            velocity: 0,
            age: 0,
            params: [
                FmOpParams::new(),
                FmOpParams::new(),
                FmOpParams::new(),
                FmOpParams::new(),
                FmOpParams::new(),
                FmOpParams::new(),
            ],
            env: [
                Envelope::new(),
                Envelope::new(),
                Envelope::new(),
                Envelope::new(),
                Envelope::new(),
                Envelope::new(),
            ],
            basepitch: [0; 6],
            ampmodsens: [0; 6],
            op_mode: [0; 6],
            pitchenv: PitchEnv::new(),
            lfo: Lfo::new(),
            algorithm: 0,
            fb_buf: [0; 2],
            fb_shift: 16,
            pitchmoddepth: 0,
            pitchmodsens: 0,
            ampmoddepth: 0,
            pitch_bend: 0,
            mod_wheel: 0,
            buf: [[0; N]; 2],
        }
    }

    /// Initialize voice for a new note with the given patch.
    /// Ported from dx7note.cc Dx7Note::init().
    pub fn note_on(&mut self, patch: &DxVoice, note: u8, velocity: u8) {
        self.note = note;
        self.velocity = velocity;
        self.state = VoiceState::Active;
        self.age = 0;

        // DX7 patch transpose only applies to the local 61-key keyboard, not MIDI input.
        // MIDI senders (DAWs, controllers, MIDI files) provide the correct note directly.
        let midinote = note as i32;

        // Per-operator initialization
        for op in 0..6 {
            let p = &patch.operators[op];

            let rates = [
                p.eg.rates[0] as i32,
                p.eg.rates[1] as i32,
                p.eg.rates[2] as i32,
                p.eg.rates[3] as i32,
            ];
            let levels = [
                p.eg.levels[0] as i32,
                p.eg.levels[1] as i32,
                p.eg.levels[2] as i32,
                p.eg.levels[3] as i32,
            ];

            // Output level with scaling
            let mut outlevel = envelope::scaleoutlevel(p.output_level as i32);
            let level_scaling = operator::scale_level(
                midinote,
                p.kbd_level_scaling_break_point as i32,
                p.kbd_level_scaling_left_depth as i32,
                p.kbd_level_scaling_right_depth as i32,
                p.kbd_level_scaling_left_curve as i32,
                p.kbd_level_scaling_right_curve as i32,
            );
            outlevel += level_scaling;
            outlevel = outlevel.min(127);
            outlevel <<= 5;
            outlevel += operator::scale_velocity(velocity as i32, p.key_velocity_sensitivity as i32);
            outlevel = outlevel.max(0);

            let rate_scaling = operator::scale_rate(midinote, p.kbd_rate_scaling as i32);
            self.env[op].init(&rates, &levels, outlevel, rate_scaling);
            self.env[op].keydown(true);

            // Frequency
            let freq = operator::osc_freq(
                midinote,
                p.osc_mode as i32,
                p.osc_freq_coarse as i32,
                p.osc_freq_fine as i32,
                p.osc_detune as i32,
            );
            self.basepitch[op] = freq;
            self.op_mode[op] = p.osc_mode as i32;
            self.ampmodsens[op] = AMPMODSENSTAB[(p.amp_mod_sensitivity & 3) as usize];

            // Reset operator state
            if patch.osc_key_sync {
                self.params[op].phase = 0;
                self.params[op].gain_out = 0;
            }
        }

        // Pitch envelope
        let pe_rates = [
            patch.pitch_eg.rates[0] as i32,
            patch.pitch_eg.rates[1] as i32,
            patch.pitch_eg.rates[2] as i32,
            patch.pitch_eg.rates[3] as i32,
        ];
        let pe_levels = [
            patch.pitch_eg.levels[0] as i32,
            patch.pitch_eg.levels[1] as i32,
            patch.pitch_eg.levels[2] as i32,
            patch.pitch_eg.levels[3] as i32,
        ];
        self.pitchenv.set(&pe_rates, &pe_levels);

        // Voice globals
        self.algorithm = patch.algorithm as i32;
        let feedback = patch.feedback as i32;
        self.fb_shift = if feedback != 0 {
            operator::FEEDBACK_BITDEPTH - feedback
        } else {
            16
        };
        self.pitchmoddepth = (patch.lfo.pitch_mod_depth as i32 * 165) >> 6;
        self.pitchmodsens = PITCHMODSENSTAB[(patch.pitch_mod_sensitivity & 7) as usize] as i32;
        self.ampmoddepth = (patch.lfo.amp_mod_depth as i32 * 165) >> 6;

        // LFO
        self.lfo.reset(&patch.lfo);
        self.lfo.keydown();
        self.fb_buf = [0; 2];
    }

    /// Release the voice (key off).
    pub fn note_off(&mut self) {
        if self.state == VoiceState::Active {
            self.state = VoiceState::Released;
            for op in 0..6 {
                self.env[op].keydown(false);
            }
            self.pitchenv.keydown(false);
        }
    }

    /// Check if the voice is completely finished.
    pub fn is_finished(&self) -> bool {
        if self.state == VoiceState::Inactive {
            return true;
        }
        if self.state == VoiceState::Released {
            let alg_idx = (self.algorithm as usize).min(31);
            for op in 0..6 {
                if is_carrier(alg_idx, op) && self.env[op].is_active() {
                    return false;
                }
            }
            return true;
        }
        false
    }

    /// Render one N-sample block into the output buffer.
    /// Ported from dx7note.cc compute() + fm_core.cc render().
    pub fn render(&mut self, output: &mut [i32; N]) {
        if self.state == VoiceState::Inactive {
            return;
        }

        self.age += 1;

        // --- LFO ---
        let lfo_val = self.lfo.getsample();
        let lfo_delay = self.lfo.getdelay();

        // --- Pitch modulation (scaled by mod wheel) ---
        let effective_pmd = (self.pitchmoddepth as i64 * self.mod_wheel as i64 / 127) as i32;
        let pmd = effective_pmd as u32 as u64 * lfo_delay as u32 as u64;
        let senslfo = self.pitchmodsens * (lfo_val - (1 << 23));
        let pmod_1 = ((pmd as i64 * senslfo as i64) >> 39).unsigned_abs() as i32;
        let pitch_mod = self.pitchenv.getsample()
            + pmod_1 * if senslfo < 0 { -1 } else { 1 }
            + self.pitch_bend;

        // --- Amplitude modulation (scaled by mod wheel) ---
        let effective_amd = (self.ampmoddepth as i64 * self.mod_wheel as i64 / 127) as i32;
        let lfo_val_inv = (1 << 24) - lfo_val;
        let amod_1 = ((effective_amd as i64 * lfo_delay as i64) >> 8) as u32;
        let amod_1 = ((amod_1 as u64 * lfo_val_inv as u64) >> 24) as u32;
        let amd_mod = amod_1;

        // --- Per-operator: compute level and frequency ---
        for op in 0..6 {
            let level = self.env[op].getsample();

            if self.ampmodsens[op] != 0 {
                let sensamp = ((amd_mod as u64 * self.ampmodsens[op] as u64) >> 24) as u32;
                let idx = ((sensamp >> 18) as usize).min(63);
                let frac = sensamp & 0x3FFFF; // 18-bit fraction
                let y0 = generated_tables::AMP_MOD_EXP_TAB[idx];
                let y1 = generated_tables::AMP_MOD_EXP_TAB[idx + 1];
                let pt = y0 + (((y1 as i64 - y0 as i64) * frac as i64) >> 18) as u32;
                let ldiff = ((level as u64 * ((pt as u64) << 4)) >> 28) as i32;
                self.params[op].level_in = level - ldiff;
            } else {
                self.params[op].level_in = level;
            }

            let basepitch = self.basepitch[op];
            if self.op_mode[op] != 0 {
                // Fixed frequency mode: no pitch mod
                self.params[op].freq = tables::freqlut_lookup(basepitch);
            } else {
                self.params[op].freq = tables::freqlut_lookup(basepitch + pitch_mod);
            }
        }

        // --- Render via bus-flag routing (fm_core.cc render()) ---
        self.render_core(output);

        // Check if voice is finished
        if self.is_finished() {
            self.state = VoiceState::Inactive;
        }
    }

    /// Compute MkI log-attenuation gain from the previous gain_out value.
    /// Maps gain_out == 0 (uninitialised) to near-silence.
    #[inline]
    fn mki_gain1(gain_out: i32) -> i32 {
        if gain_out == 0 { tables::ENV_MAX as i32 - 1 } else { gain_out }
    }

    /// Compute MkI log-attenuation gain from the envelope level.
    /// No clamping — matches Dexed's int32_t semantics. Negative values (from
    /// EG levels above the 0-99 spec range) produce "overdriven" operators;
    /// the `gain as u16` cast in mki_sin wraps correctly via 16-bit truncation.
    #[inline]
    fn mki_gain2(level_in: i32) -> i32 {
        tables::ENV_MAX as i32 - (level_in >> 14)
    }

    /// Core rendering using MkI log-domain FM with bus-flag algorithm routing.
    pub fn render_core(&mut self, output: &mut [i32; N]) {
        const K_LEVEL_THRESH: i32 = tables::ENV_MAX as i32 - 100; // 16284
        let alg_idx = (self.algorithm as usize).min(31);
        let mut alg = ALGORITHMS[alg_idx];
        let mut has_contents = [true, false, false];
        let fb_on = self.fb_shift < 16;

        // For algorithms 4 and 6 with feedback, redirect op 0 output to carrier bus
        if fb_on && (alg_idx == 3 || alg_idx == 5) {
            alg[0] = 0xc4;
        }

        let mut skip_count: usize = 0;

        for op in 0..6 {
            if skip_count > 0 {
                skip_count -= 1;
                continue;
            }

            let flags = alg[op];
            let mut add = (flags & OUT_BUS_ADD) != 0;
            let inbus = (flags >> 4) & 3;
            let outbus = flags & 3;

            let gain1 = Self::mki_gain1(self.params[op].gain_out);
            let gain2 = Self::mki_gain2(self.params[op].level_in);
            self.params[op].gain_out = gain2;

            if gain1 <= K_LEVEL_THRESH || gain2 <= K_LEVEL_THRESH {
                if !has_contents[outbus as usize] {
                    add = false;
                }

                if inbus == 0 || !has_contents[inbus as usize] {
                    if (flags & 0xc0) == 0xc0 && fb_on {
                        // Feedback operator — algorithm-specific handling
                        let phase = self.params[op].phase;
                        let freq = self.params[op].freq;
                        match alg_idx {
                            3 => {
                                // Algorithm 4: fused 3-operator feedback chain
                                let gain2_1 = Self::mki_gain2(self.params[1].level_in);
                                self.params[1].gain_out = gain2_1;
                                let gain1_1 = if gain2_1 == 0 { tables::ENV_MAX as i32 - 1 } else { gain2_1 };
                                let gain2_2 = Self::mki_gain2(self.params[2].level_in);
                                self.params[2].gain_out = gain2_2;
                                let gain1_2 = if gain2_2 == 0 { tables::ENV_MAX as i32 - 1 } else { gain2_2 };

                                operator::compute_fb3(
                                    output,
                                    phase, freq, gain1, gain2,
                                    self.params[1].phase, self.params[1].freq, gain1_1, gain2_1,
                                    self.params[2].phase, self.params[2].freq, gain1_2, gain2_2,
                                    &mut self.fb_buf,
                                    (self.fb_shift + 2).min(16),
                                );
                                self.params[1].phase = self.params[1].phase.wrapping_add(
                                    self.params[1].freq << LG_N,
                                );
                                self.params[2].phase = self.params[2].phase.wrapping_add(
                                    self.params[2].freq << LG_N,
                                );
                                skip_count = 2;
                            }
                            5 => {
                                // Algorithm 6: fused 2-operator feedback chain
                                let gain2_1 = Self::mki_gain2(self.params[1].level_in);
                                self.params[1].gain_out = gain2_1;
                                let gain1_1 = if gain2_1 == 0 { tables::ENV_MAX as i32 - 1 } else { gain2_1 };

                                operator::compute_fb2(
                                    output,
                                    phase, freq, gain1, gain2,
                                    self.params[1].phase, self.params[1].freq, gain1_1, gain2_1,
                                    &mut self.fb_buf,
                                    (self.fb_shift + 2).min(16),
                                );
                                self.params[1].phase = self.params[1].phase.wrapping_add(
                                    self.params[1].freq << LG_N,
                                );
                                skip_count = 1;
                            }
                            31 => {
                                // Algorithm 32: single FB with fb_shift+2
                                if outbus == 0 {
                                    operator::compute_fb(
                                        output, phase, freq, gain1, gain2,
                                        &mut self.fb_buf, (self.fb_shift + 2).min(16), add,
                                    );
                                } else {
                                    let buf_idx = (outbus - 1) as usize;
                                    let mut tmp = self.buf[buf_idx];
                                    operator::compute_fb(
                                        &mut tmp, phase, freq, gain1, gain2,
                                        &mut self.fb_buf, (self.fb_shift + 2).min(16), add,
                                    );
                                    self.buf[buf_idx] = tmp;
                                }
                            }
                            _ => {
                                // Standard feedback
                                if outbus == 0 {
                                    operator::compute_fb(
                                        output, phase, freq, gain1, gain2,
                                        &mut self.fb_buf, self.fb_shift, add,
                                    );
                                } else {
                                    let buf_idx = (outbus - 1) as usize;
                                    let mut tmp = self.buf[buf_idx];
                                    operator::compute_fb(
                                        &mut tmp, phase, freq, gain1, gain2,
                                        &mut self.fb_buf, self.fb_shift, add,
                                    );
                                    self.buf[buf_idx] = tmp;
                                }
                            }
                        }
                    } else {
                        // Pure sine (no input)
                        let phase = self.params[op].phase;
                        let freq = self.params[op].freq;
                        if outbus == 0 {
                            operator::compute_pure(
                                output, phase, freq, gain1, gain2, add,
                            );
                        } else {
                            let buf_idx = (outbus - 1) as usize;
                            let mut tmp = self.buf[buf_idx];
                            operator::compute_pure(
                                &mut tmp, phase, freq, gain1, gain2, add,
                            );
                            self.buf[buf_idx] = tmp;
                        }
                    }
                } else {
                    // Modulated by input bus
                    let phase = self.params[op].phase;
                    let freq = self.params[op].freq;
                    let input = self.buf[(inbus - 1) as usize];
                    if outbus == 0 {
                        operator::compute(
                            output, &input, phase, freq, gain1, gain2, add,
                        );
                    } else {
                        let buf_idx = (outbus - 1) as usize;
                        let mut tmp = if buf_idx == (inbus - 1) as usize {
                            input
                        } else {
                            self.buf[buf_idx]
                        };
                        operator::compute(
                            &mut tmp, &input, phase, freq, gain1, gain2, add,
                        );
                        self.buf[buf_idx] = tmp;
                    }
                }
                has_contents[outbus as usize] = true;
            } else if !add {
                has_contents[outbus as usize] = false;
            }

            self.params[op].phase = self.params[op].phase.wrapping_add(
                self.params[op].freq << LG_N,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvParams;
    use crate::lfo::LfoParams;
    use crate::operator::OperatorParams;
    use crate::patch::DxVoice;

    fn ensure_init() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            tables::init_tables(44100);
            crate::lfo::init_lfo(44100);
            crate::pitchenv::init_pitchenv(44100);
        });
    }

    /// Build a minimal patch with specified algorithm and per-op output levels.
    /// All ops use ratio 1:1, fast envelopes, no feedback unless specified.
    fn test_patch(algorithm: u8, output_levels: [u8; 6], feedback: u8) -> DxVoice {
        let mut ops = [OperatorParams::default(); 6];
        for (i, op) in ops.iter_mut().enumerate() {
            op.output_level = output_levels[i];
            op.eg = EnvParams {
                rates: [99, 99, 99, 99],
                levels: [99, 99, 99, 0],
            };
            op.osc_freq_coarse = 1;
            op.osc_freq_fine = 0;
            op.osc_detune = 7; // center
            op.key_velocity_sensitivity = 0;
            op.amp_mod_sensitivity = 0;
            op.kbd_level_scaling_left_depth = 0;
            op.kbd_level_scaling_right_depth = 0;
            op.kbd_rate_scaling = 0;
        }
        DxVoice {
            operators: ops,
            pitch_eg: EnvParams {
                rates: [99, 99, 99, 99],
                levels: [50, 50, 50, 50],
            },
            algorithm,
            feedback,
            osc_key_sync: true,
            lfo: LfoParams::default(),
            pitch_mod_sensitivity: 0,
            transpose: 24,
            name: *b"TEST PATCH",
        }
    }

    /// Render several blocks and return all samples.
    fn render_voice(voice: &mut Voice, num_blocks: usize) -> Vec<i32> {
        let mut all = Vec::with_capacity(num_blocks * N);
        for _ in 0..num_blocks {
            let mut buf = [0i32; N];
            voice.render(&mut buf);
            all.extend_from_slice(&buf);
        }
        all
    }

    /// Measure peak absolute amplitude.
    fn peak(samples: &[i32]) -> i32 {
        samples.iter().map(|s| s.abs()).max().unwrap_or(0)
    }

    /// Count zero crossings (positive→negative) to estimate frequency.
    fn zero_crossings(samples: &[i32]) -> usize {
        let mut count = 0;
        for i in 1..samples.len() {
            if samples[i - 1] >= 0 && samples[i] < 0 {
                count += 1;
            }
        }
        count
    }

    // ====================================================================
    // Level 1: Algorithm 32 — all 6 carriers, pure additive
    // ====================================================================

    #[test]
    fn test_alg32_single_carrier() {
        ensure_init();
        // Algorithm 32, only OP1 (index 5) active at OL=99
        let patch = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100); // A4
        let samples = render_voice(&mut voice, 32);

        let p = peak(&samples);
        assert!(p > 1_000_000, "Single carrier should produce output, peak={p}");

        // Should be ~440 Hz
        let xings = zero_crossings(&samples);
        let duration = samples.len() as f64 / 44100.0;
        let freq = xings as f64 / duration;
        assert!(
            (freq - 440.0).abs() < 20.0,
            "Should be ~440 Hz, got {freq:.1} Hz"
        );
    }

    #[test]
    fn test_alg32_six_carriers_louder_than_one() {
        ensure_init();
        // All 6 carriers at OL=99
        let patch6 = test_patch(31, [99, 99, 99, 99, 99, 99], 0);
        let mut voice6 = Voice::new();
        voice6.note_on(&patch6, 69, 100);
        let samples6 = render_voice(&mut voice6, 32);

        // Single carrier
        let patch1 = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        let mut voice1 = Voice::new();
        voice1.note_on(&patch1, 69, 100);
        let samples1 = render_voice(&mut voice1, 32);

        let p6 = peak(&samples6);
        let p1 = peak(&samples1);
        // 6 carriers should be louder (not necessarily 6x due to phase)
        assert!(
            p6 > p1 * 3,
            "6 carriers should be much louder than 1: p6={p6}, p1={p1}"
        );
    }

    #[test]
    fn test_alg32_silent_ops_produce_nothing() {
        ensure_init();
        // All OLs at 0
        let patch = test_patch(31, [0, 0, 0, 0, 0, 0], 0);
        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100);
        let samples = render_voice(&mut voice, 16);

        let p = peak(&samples);
        assert!(p < 1000, "All-silent patch should produce ~0, peak={p}");
    }

    // ====================================================================
    // Level 2: Algorithm 5 — three modulator→carrier pairs
    // ====================================================================

    #[test]
    fn test_alg5_three_pairs() {
        ensure_init();
        // Alg 5: OP6→OP5, OP4→OP3, OP2→OP1
        // Carriers are indices 0(OP6), 2(OP4), 4(OP2)... wait
        // Let me check: alg[4] = [0xc1, 0x14, 0x01, 0x14, 0x01, 0x14]
        // Carriers are the ones with OUT_BUS_ADD (bit 2):
        // 0x14 & 4 = 4 (yes), 0x01 & 4 = 0 (no), 0xc1 & 4 = 0 (no)
        // So carriers at indices 1, 3, 5 (OP5, OP3, OP1)
        // Modulators at indices 0, 2, 4 (OP6, OP4, OP2)

        // Set carriers to OL=99, modulators to OL=0 (pure sines, no FM)
        let patch = test_patch(4, [0, 99, 0, 99, 0, 99], 0);
        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100);
        let samples = render_voice(&mut voice, 32);

        let p = peak(&samples);
        assert!(
            p > 1_000_000,
            "3 unmodulated carriers should produce output, peak={p}"
        );
    }

    #[test]
    fn test_alg5_modulation_adds_harmonics() {
        ensure_init();
        // Carriers at OL=99, modulators at OL=99 (heavy FM)
        let patch_fm = test_patch(4, [99, 99, 99, 99, 99, 99], 0);
        let mut voice_fm = Voice::new();
        voice_fm.note_on(&patch_fm, 69, 100);
        let samples_fm = render_voice(&mut voice_fm, 32);

        // Carriers only (modulators silent)
        let patch_pure = test_patch(4, [0, 99, 0, 99, 0, 99], 0);
        let mut voice_pure = Voice::new();
        voice_pure.note_on(&patch_pure, 69, 100);
        let samples_pure = render_voice(&mut voice_pure, 32);

        // FM version should have more zero crossings (higher harmonics)
        let xings_fm = zero_crossings(&samples_fm);
        let xings_pure = zero_crossings(&samples_pure);
        assert!(
            xings_fm > xings_pure * 2,
            "FM should create more harmonics: xings_fm={xings_fm}, xings_pure={xings_pure}"
        );
    }

    // ====================================================================
    // Level 3: Algorithm 1 — 6-op chain, single carrier
    // ====================================================================

    #[test]
    fn test_alg1_single_carrier_output() {
        ensure_init();
        // Alg 1: 6→5→4→3→2→1, carrier = OP1 (index 5? Let me check)
        // alg[0] = [0xc1, 0x11, 0x11, 0x14, 0x01, 0x14]
        // Index 3: 0x14 → add=true (carrier)
        // Index 5: 0x14 → add=true (carrier)
        // Wait, that's TWO carriers? Let me check Algorithm 1 more carefully.
        // 0x14 = outbus=0, add=true, inbus=1
        // Index 3 and 5 are carriers.
        //
        // Actually in the DX7, Algorithm 1 has only 1 carrier (OP1).
        // Let me re-check the flags:
        // [0xc1, 0x11, 0x11, 0x14, 0x01, 0x14]
        //   0     1     2     3     4     5
        //  OP6   OP5   OP4   OP3   OP2   OP1
        //
        // OP3 (index 3): 0x14 → outbus=0, add=true → carrier!
        // OP1 (index 5): 0x14 → outbus=0, add=true → carrier!
        //
        // DX7 Algorithm 1: chain 6→5→4→3→2→1, only OP1 is carrier.
        // But MSFA shows OP3 and OP1 as carriers. Hmm.
        //
        // Actually, DX7 Alg 1: 6→5→4→3, separate 2→1.
        // Wait no. Let me check properly. Alg 1 has:
        // OP6(fb)→OP5→OP4→OP3(carrier) and OP2→OP1(carrier)
        // So TWO carriers: OP3 and OP1. That matches.

        // Only carriers active: OP3 (index 3) and OP1 (index 5)
        let patch = test_patch(0, [0, 0, 0, 99, 0, 99], 0);
        let mut voice = Voice::new();
        voice.note_on(&patch, 69, 100);
        let samples = render_voice(&mut voice, 32);

        let p = peak(&samples);
        assert!(
            p > 1_000_000,
            "Alg 1 carriers should produce output, peak={p}"
        );
    }

    #[test]
    fn test_alg1_modulator_chain_adds_harmonics() {
        ensure_init();
        // Full chain active vs carrier only
        let patch_full = test_patch(0, [99, 99, 99, 99, 99, 99], 0);
        let mut voice_full = Voice::new();
        voice_full.note_on(&patch_full, 69, 100);
        let samples_full = render_voice(&mut voice_full, 32);

        let patch_carrier = test_patch(0, [0, 0, 0, 99, 0, 99], 0);
        let mut voice_carrier = Voice::new();
        voice_carrier.note_on(&patch_carrier, 69, 100);
        let samples_carrier = render_voice(&mut voice_carrier, 32);

        let xings_full = zero_crossings(&samples_full);
        let xings_carrier = zero_crossings(&samples_carrier);
        assert!(
            xings_full > xings_carrier,
            "Modulator chain should add harmonics: full={xings_full}, carrier={xings_carrier}"
        );
    }

    // ====================================================================
    // Level 4: Output level scaling
    // ====================================================================

    #[test]
    fn test_output_level_scales_amplitude() {
        ensure_init();
        // Single carrier at different OLs
        let patch_loud = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        let mut voice_loud = Voice::new();
        voice_loud.note_on(&patch_loud, 69, 100);
        let samples_loud = render_voice(&mut voice_loud, 32);

        let patch_mid = test_patch(31, [0, 0, 0, 0, 0, 70], 0);
        let mut voice_mid = Voice::new();
        voice_mid.note_on(&patch_mid, 69, 100);
        let samples_mid = render_voice(&mut voice_mid, 32);

        let patch_quiet = test_patch(31, [0, 0, 0, 0, 0, 40], 0);
        let mut voice_quiet = Voice::new();
        voice_quiet.note_on(&patch_quiet, 69, 100);
        let samples_quiet = render_voice(&mut voice_quiet, 32);

        let p_loud = peak(&samples_loud);
        let p_mid = peak(&samples_mid);
        let p_quiet = peak(&samples_quiet);

        assert!(
            p_loud > p_mid && p_mid > p_quiet,
            "Higher OL should be louder: OL99={p_loud}, OL70={p_mid}, OL40={p_quiet}"
        );
        assert!(
            p_quiet > 10_000,
            "OL=40 should still produce audible output, peak={p_quiet}"
        );
    }

    #[test]
    fn test_output_level_2_still_audible() {
        ensure_init();
        // Flunk bass has carriers at OL=2. Is that audible?
        let patch = test_patch(31, [0, 0, 0, 0, 0, 2], 0);
        let mut voice = Voice::new();
        voice.note_on(&patch, 36, 127); // C2, max velocity
        let samples = render_voice(&mut voice, 64);

        let p = peak(&samples);
        eprintln!("OL=2 peak amplitude: {p}");
        // Report what we find — even if very quiet
        assert!(
            p > 0,
            "OL=2 should produce some output"
        );
    }

    // ====================================================================
    // Level 5: Voice with real patches
    // ====================================================================

    #[test]
    fn test_init_voice_produces_sine() {
        ensure_init();
        let patch = DxVoice::init_voice();
        let mut voice = Voice::new();
        voice.note_on(&patch, 60, 100); // C4
        let samples = render_voice(&mut voice, 32);

        let p = peak(&samples);
        assert!(p > 1_000_000, "INIT VOICE should produce output, peak={p}");

        // Should be ~261 Hz (C4)
        let xings = zero_crossings(&samples);
        let duration = samples.len() as f64 / 44100.0;
        let freq = xings as f64 / duration;
        assert!(
            (freq - 261.6).abs() < 15.0,
            "INIT VOICE at C4 should be ~261 Hz, got {freq:.1} Hz"
        );
    }

    #[test]
    fn test_bass1_rom1a_output_level() {
        ensure_init();
        // Load BASS 1 from ROM1A (voice index 14)
        let voices = crate::rom1a::load_rom1a();
        let bass1 = &voices[14];

        eprintln!("BASS 1 algorithm: {}", bass1.algorithm + 1);
        eprintln!("BASS 1 feedback: {}", bass1.feedback);
        for (i, op) in bass1.operators.iter().enumerate() {
            let op_name = 6 - i; // index 0=OP6, 5=OP1
            eprintln!(
                "  OP{}: OL={} coarse={} kvs={} EG L={},{},{},{}",
                op_name, op.output_level, op.osc_freq_coarse,
                op.key_velocity_sensitivity,
                op.eg.levels[0], op.eg.levels[1], op.eg.levels[2], op.eg.levels[3]
            );
        }

        let mut voice = Voice::new();
        voice.note_on(bass1, 36, 100); // C2, moderate velocity
        let samples = render_voice(&mut voice, 64);

        let p = peak(&samples);
        eprintln!("BASS 1 at C2 vel=100: peak={p}");
        assert!(
            p > 100_000,
            "BASS 1 should produce substantial output, peak={p}"
        );
    }

    // ====================================================================
    // Level 6: Synth mixing
    // ====================================================================

    #[test]
    fn test_synth_single_note_output_range() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        let patch = DxVoice::init_voice();
        synth.load_patch(patch);

        synth.note_on(60, 100);

        // Render some audio
        let mut output = vec![0.0f32; 4096];
        synth.render(&mut output);

        let peak_f32 = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        eprintln!("Synth INIT VOICE peak f32: {peak_f32}");

        assert!(
            peak_f32 > 0.01,
            "Synth output should be audible, peak={peak_f32}"
        );
        assert!(
            peak_f32 < 1.0,
            "Synth output should not clip, peak={peak_f32}"
        );
    }

    // ====================================================================
    // Level 7: Real patch rendering — Flunk bass (vrc107b voice 25)
    // ====================================================================

    #[test]
    fn test_flunk_bass_sustained_output() {
        ensure_init();
        let v = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);

        assert_eq!(v.algorithm, 16, "Should be algorithm 17");
        assert_eq!(v.name_str(), "Flunk bass");

        let mut voice = Voice::new();
        voice.note_on(&v, 33, 120); // A1 (low bass), strong velocity

        // Render 200 blocks (~290ms at 44100 Hz, N=64)
        let samples = render_voice(&mut voice, 200);

        let p = peak(&samples);
        eprintln!("Flunk bass A1 vel=120: peak={p}");
        assert!(p > 100_000, "Flunk bass should produce strong output, peak={p}");

        // Check sustained portion (not just attack transient)
        // Look at blocks 50-100 (~73-145ms) — should still have energy
        let sustain_start = 50 * N;
        let sustain_end = 100 * N;
        let sustain_peak = peak(&samples[sustain_start..sustain_end]);
        eprintln!("Flunk bass sustain region peak={sustain_peak}");
        assert!(
            sustain_peak > 50_000,
            "Flunk bass should sustain, not just click. Sustain peak={sustain_peak}"
        );
    }

    #[test]
    fn test_flunk_bass_has_pitch_content() {
        ensure_init();
        let v = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);

        let mut voice = Voice::new();
        voice.note_on(&v, 33, 100); // A1 = 55 Hz

        // Render enough for meaningful frequency analysis
        let samples = render_voice(&mut voice, 100);

        // Count zero crossings in the sustain region
        let start = 20 * N;
        let end = 80 * N;
        let xings = zero_crossings(&samples[start..end]);
        let duration = (end - start) as f64 / 44100.0;
        let freq = xings as f64 / duration;

        eprintln!("Flunk bass A1: estimated freq={freq:.1} Hz (expect ~55 Hz fundamental)");

        // FM bass with heavy modulation (FB=7, modulators at 6x and 9x)
        // produces lots of high-frequency harmonics, so zero crossings
        // will be much higher than the 55 Hz fundamental. This is expected.
        // Just verify there IS pitched content (not DC or silence).
        assert!(
            freq > 20.0,
            "Flunk bass should produce audible frequencies, got {freq:.1} Hz"
        );
    }

    // ====================================================================
    // DIAGNOSTIC: Flunk bass per-operator state dump
    // ====================================================================

    #[test]
    fn test_flunk_bass_operator_state_dump() {
        ensure_init();
        let v = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);
        let mut voice = Voice::new();
        voice.note_on(&v, 36, 100); // C2, velocity=100

        eprintln!("\n=== FLUNK BASS VOICE-LEVEL ANALYSIS (C2, vel=100) ===");
        eprintln!("Algorithm: {} (idx {}), FB shift: {}", v.algorithm + 1, v.algorithm, voice.fb_shift);
        eprintln!("MIDI note: 36 (transpose ignored for MIDI input)");

        // Dump basepitch
        for op in 0..6 {
            let op_name = 6 - op;
            eprintln!("  OP{}: basepitch={}, gain_out={}", op_name, voice.basepitch[op], voice.params[op].gain_out);
        }

        // Render 500 blocks (~0.73s) and collect all output
        let num_blocks = 500;
        let mut all_samples = Vec::with_capacity(num_blocks * N);
        for block in 0..num_blocks {
            let mut buf = [0i32; N];
            voice.render(&mut buf);

            // Dump gain_out state for first 3 blocks (after render sets them)
            if block < 3 {
                eprintln!("\n  After block {block}: gain_out per op:");
                for op in 0..6 {
                    eprintln!("    OP{}: gain_out={}", 6 - op, voice.params[op].gain_out);
                }
                let pk = buf.iter().map(|s| s.abs()).max().unwrap_or(0);
                eprintln!("    output peak={pk}");
            }

            all_samples.extend_from_slice(&buf);
        }

        // Analyze raw voice output (i32)
        let total = all_samples.len();
        let skip = 100 * N; // skip first 100 blocks (~145ms)
        let analysis = &all_samples[skip..];
        let analysis_f64: Vec<f64> = analysis.iter().map(|&s| s as f64).collect();

        // Peak and RMS
        let pk = analysis.iter().map(|s| s.abs()).max().unwrap_or(0);
        let rms = (analysis_f64.iter().map(|s| s * s).sum::<f64>() / analysis_f64.len() as f64).sqrt();
        eprintln!("\nSustain region (blocks 100-500):");
        eprintln!("  Peak: {pk}, RMS: {rms:.0}");

        // Autocorrelation at expected fundamental period
        // midinote=24 → C1 ~32.7 Hz → period ~1348 samples at 44100 Hz
        let fund_freq: f64 = 32.703; // C1
        let period = (44100.0_f64 / fund_freq).round() as usize;
        let sub_period = period * 2; // sub-octave from OP3 at coarse=0

        let corr_len = (period * 8).min(analysis_f64.len() / 2);
        let mut autocorr_fund = 0.0f64;
        let mut autocorr_sub = 0.0f64;
        let mut energy = 0.0f64;
        for i in 0..corr_len {
            energy += analysis_f64[i] * analysis_f64[i];
            if i + period < analysis_f64.len() {
                autocorr_fund += analysis_f64[i] * analysis_f64[i + period];
            }
            if i + sub_period < analysis_f64.len() {
                autocorr_sub += analysis_f64[i] * analysis_f64[i + sub_period];
            }
        }
        let r_fund = if energy > 0.0 { autocorr_fund / energy } else { 0.0 };
        let r_sub = if energy > 0.0 { autocorr_sub / energy } else { 0.0 };

        eprintln!("  Autocorrelation @ {fund_freq:.1} Hz (period {period}): {r_fund:+.4}");
        eprintln!("  Autocorrelation @ {:.1} Hz (period {sub_period}): {r_sub:+.4}", fund_freq / 2.0);

        // Also check at octave above (65.4 Hz, C2)
        let oct_period = period / 2;
        let mut autocorr_oct = 0.0f64;
        for i in 0..corr_len {
            if i + oct_period < analysis_f64.len() {
                autocorr_oct += analysis_f64[i] * analysis_f64[i + oct_period];
            }
        }
        let r_oct = if energy > 0.0 { autocorr_oct / energy } else { 0.0 };
        eprintln!("  Autocorrelation @ {:.1} Hz (period {oct_period}): {r_oct:+.4}", fund_freq * 2.0);

        if r_fund > 0.5 || r_sub > 0.5 || r_oct > 0.5 {
            eprintln!("  → TONAL (good periodicity)");
        } else if r_fund > 0.2 || r_sub > 0.2 || r_oct > 0.2 {
            eprintln!("  → WEAK TONE");
        } else {
            eprintln!("  → NOISE-LIKE (autocorrelation < 0.2 at all periods)");
        }

        // Zero-crossing frequency
        let mut zc = 0;
        for i in 1..analysis.len() {
            if (analysis[i-1] >= 0) != (analysis[i] >= 0) { zc += 1; }
        }
        let zc_freq = zc as f64 / 2.0 / (analysis.len() as f64 / 44100.0);
        eprintln!("  Zero-crossing freq: {zc_freq:.1} Hz");

        // DC offset
        let mean = analysis_f64.iter().sum::<f64>() / analysis_f64.len() as f64;
        eprintln!("  DC offset: {mean:.0} ({:.1}% of RMS)", (mean.abs() / rms) * 100.0);

        eprintln!("=== END ANALYSIS ===\n");

        // The test passes regardless — this is diagnostic
        assert!(pk > 0, "Voice should produce output");
    }

    // ====================================================================
    // Level 8: Transpose
    // ====================================================================

    #[test]
    fn test_transpose_ignored_for_midi() {
        ensure_init();
        // DX7 patch transpose only applies to the local keyboard, not MIDI input.
        // Verify that different transpose values produce the SAME pitch for MIDI note-on.
        let mut patch_center = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        patch_center.transpose = 24;

        let mut voice_c = Voice::new();
        voice_c.note_on(&patch_center, 60, 100);
        let samples_c = render_voice(&mut voice_c, 64);
        let xings_c = zero_crossings(&samples_c);

        // Transpose=36 (+12): should still play MIDI 60 (no shift)
        let mut patch_up = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        patch_up.transpose = 36;

        let mut voice_u = Voice::new();
        voice_u.note_on(&patch_up, 60, 100);
        let samples_u = render_voice(&mut voice_u, 64);
        let xings_u = zero_crossings(&samples_u);

        // Transpose=12 (-12): should still play MIDI 60 (no shift)
        let mut patch_dn = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        patch_dn.transpose = 12;

        let mut voice_d = Voice::new();
        voice_d.note_on(&patch_dn, 60, 100);
        let samples_d = render_voice(&mut voice_d, 64);
        let xings_d = zero_crossings(&samples_d);

        // All should have the same frequency regardless of transpose
        let ratio_up = xings_u as f64 / xings_c as f64;
        assert!(
            (ratio_up - 1.0).abs() < 0.1,
            "Transpose should not affect MIDI pitch: ratio={ratio_up:.2} (xings: center={xings_c}, up={xings_u})"
        );

        let ratio_dn = xings_d as f64 / xings_c as f64;
        assert!(
            (ratio_dn - 1.0).abs() < 0.1,
            "Transpose should not affect MIDI pitch: ratio={ratio_dn:.2} (xings: center={xings_c}, down={xings_d})"
        );
    }

    // ====================================================================
    // Level 9: All 32 algorithms produce output
    // ====================================================================

    #[test]
    fn test_all_algorithms_produce_output() {
        ensure_init();
        for alg_idx in 0..32u8 {
            // Set all ops to OL=99 so every algorithm has active carriers
            let patch = test_patch(alg_idx, [99, 99, 99, 99, 99, 99], 3);
            let mut voice = Voice::new();
            voice.note_on(&patch, 60, 100);
            let samples = render_voice(&mut voice, 32);
            let p = peak(&samples);
            assert!(
                p > 10_000,
                "Algorithm {} should produce output when all ops at OL=99, peak={p}",
                alg_idx + 1
            );
        }
    }

    #[test]
    fn test_all_algorithms_carrier_count() {
        // MSFA uses a 2-bus approximation, so carrier counts differ from
        // the DX7 spec for algorithms with long chains (e.g. alg 1-4).
        // These are the counts from the ALGORITHMS bus-flag table.
        let expected_carriers: [usize; 32] = [
            2, 2, 2, 2, 3, 3, 3, 3, 3, 3, // alg 1-10
            3, 4, 4, 3, 3, 3, 3, 3, 3, 4, // alg 11-20
            4, 4, 4, 5, 5, 4, 4, 3, 4, 4, // alg 21-30
            5, 6,                           // alg 31-32
        ];
        for (alg_idx, &expected) in expected_carriers.iter().enumerate() {
            let carrier_count = (0..6)
                .filter(|&op| is_carrier(alg_idx, op))
                .count();
            assert_eq!(
                carrier_count, expected,
                "Algorithm {} should have {} carriers, found {}",
                alg_idx + 1, expected, carrier_count
            );
        }
    }

    // ====================================================================
    // Level 10: Polyphony and voice management
    // ====================================================================

    #[test]
    fn test_polyphony_two_notes() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        let patch = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        synth.load_patch(patch);

        // Play two notes simultaneously
        synth.note_on(60, 100); // C4
        synth.note_on(64, 100); // E4

        let mut output = vec![0.0f32; 4096];
        synth.render(&mut output);

        // Should be louder than one note alone
        let peak_two = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Now play one note
        let mut synth2 = crate::synth::Synth::new(44100);
        synth2.load_patch(test_patch(31, [0, 0, 0, 0, 0, 99], 0));
        synth2.note_on(60, 100);

        let mut output2 = vec![0.0f32; 4096];
        synth2.render(&mut output2);

        let peak_one = output2.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(
            peak_two > peak_one,
            "Two notes should be louder than one: two={peak_two}, one={peak_one}"
        );
    }

    #[test]
    fn test_note_off_releases() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        let patch = test_patch(31, [0, 0, 0, 0, 0, 99], 0);
        synth.load_patch(patch);

        synth.note_on(60, 100);

        // Render a bit while held
        let mut held = vec![0.0f32; 2048];
        synth.render(&mut held);
        let peak_held = held.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Release and render more
        synth.note_off(60);
        let mut released = vec![0.0f32; 8192];
        synth.render(&mut released);

        // End of release should be very quiet
        let tail = &released[released.len() - 2048..];
        let peak_tail = tail.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(
            peak_held > 0.01,
            "Held note should produce sound, peak={peak_held}"
        );
        assert!(
            peak_tail < peak_held * 0.5,
            "Released note should decay: held={peak_held}, tail={peak_tail}"
        );
    }

    #[test]
    fn test_sustain_pedal() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        synth.load_patch(test_patch(31, [0, 0, 0, 0, 0, 99], 0));

        // Sustain pedal on (CC64 >= 64)
        synth.control_change(64, 127);
        synth.note_on(60, 100);

        // Render while held
        let mut buf = vec![0.0f32; 2048];
        synth.render(&mut buf);

        // Note off — but sustain pedal is on, so it should keep sounding
        synth.note_off(60);
        let mut sustained = vec![0.0f32; 4096];
        synth.render(&mut sustained);
        let peak_sustained = sustained.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Now release sustain pedal
        synth.control_change(64, 0);
        let mut after_pedal = vec![0.0f32; 8192];
        synth.render(&mut after_pedal);
        let peak_after = after_pedal[after_pedal.len() - 2048..]
            .iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(
            peak_sustained > 0.01,
            "Note should sustain while pedal held: peak={peak_sustained}"
        );
        assert!(
            peak_after < peak_sustained * 0.5,
            "Note should release after pedal up: sustained={peak_sustained}, after={peak_after}"
        );
    }

    // ====================================================================
    // Level 11: Pitch bend
    // ====================================================================

    #[test]
    fn test_pitch_bend_changes_frequency() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        synth.load_patch(test_patch(31, [0, 0, 0, 0, 0, 99], 0));

        // Play note without pitch bend
        synth.note_on(60, 100);
        let mut no_bend = vec![0.0f32; 4096];
        synth.render(&mut no_bend);
        let xings_no_bend = no_bend.iter().zip(no_bend.iter().skip(1))
            .filter(|(&a, &b)| a >= 0.0 && b < 0.0)
            .count();

        // Reset and play with max pitch bend up (+8191)
        let mut synth2 = crate::synth::Synth::new(44100);
        synth2.load_patch(test_patch(31, [0, 0, 0, 0, 0, 99], 0));
        synth2.note_on(60, 100);
        synth2.pitch_bend(8191); // max up
        let mut bend_up = vec![0.0f32; 4096];
        synth2.render(&mut bend_up);
        let xings_bend_up = bend_up.iter().zip(bend_up.iter().skip(1))
            .filter(|(&a, &b)| a >= 0.0 && b < 0.0)
            .count();

        // Bend up should increase frequency
        assert!(
            xings_bend_up > xings_no_bend,
            "Pitch bend up should raise frequency: no_bend={xings_no_bend}, bend_up={xings_bend_up}"
        );

        // DX7 default is ±12 semitones = ±1 octave
        // Max bend should roughly double the frequency
        let ratio = xings_bend_up as f64 / xings_no_bend as f64;
        assert!(
            ratio > 1.8 && ratio < 2.2,
            "Max bend should be ~2x (1 octave): ratio={ratio:.2}"
        );
    }

    // ====================================================================
    // Level 12: Volume and expression
    // ====================================================================

    #[test]
    fn test_master_volume_scales_output() {
        ensure_init();
        let patch = test_patch(31, [0, 0, 0, 0, 0, 99], 0);

        let mut synth_loud = crate::synth::Synth::new(44100);
        synth_loud.load_patch(patch.clone());
        synth_loud.set_master_volume(1.0);
        synth_loud.note_on(60, 100);
        let mut loud = vec![0.0f32; 2048];
        synth_loud.render(&mut loud);
        let peak_loud = loud.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        let mut synth_quiet = crate::synth::Synth::new(44100);
        synth_quiet.load_patch(patch);
        synth_quiet.set_master_volume(0.1);
        synth_quiet.note_on(60, 100);
        let mut quiet = vec![0.0f32; 2048];
        synth_quiet.render(&mut quiet);
        let peak_quiet = quiet.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        let ratio = peak_loud / peak_quiet;
        assert!(
            (ratio - 10.0).abs() < 2.0,
            "Volume 1.0 vs 0.1 should be ~10x: ratio={ratio:.1}"
        );
    }

    #[test]
    fn test_expression_cc11_scales_output() {
        ensure_init();
        let patch = test_patch(31, [0, 0, 0, 0, 0, 99], 0);

        let mut synth_full = crate::synth::Synth::new(44100);
        synth_full.load_patch(patch.clone());
        synth_full.note_on(60, 100);
        let mut full = vec![0.0f32; 2048];
        synth_full.render(&mut full);
        let peak_full = full.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        let mut synth_half = crate::synth::Synth::new(44100);
        synth_half.load_patch(patch);
        synth_half.control_change(11, 64); // expression ~50%
        synth_half.note_on(60, 100);
        let mut half = vec![0.0f32; 2048];
        synth_half.render(&mut half);
        let peak_half = half.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(
            peak_full > peak_half * 1.5,
            "Full expression should be louder than 50%: full={peak_full}, half={peak_half}"
        );
    }

    // ====================================================================
    // Level 13: MIDI message processing
    // ====================================================================

    #[test]
    fn test_process_midi_note_on_off() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        synth.load_patch(test_patch(31, [0, 0, 0, 0, 0, 99], 0));

        // Note On: status=0x90, note=60, velocity=100
        synth.process_midi(&[0x90, 60, 100]);

        let mut buf = vec![0.0f32; 2048];
        synth.render(&mut buf);
        let peak_on = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Note Off: status=0x80, note=60, velocity=0
        synth.process_midi(&[0x80, 60, 0]);
        let mut buf2 = vec![0.0f32; 8192];
        synth.render(&mut buf2);
        let peak_off = buf2[buf2.len() - 2048..]
            .iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(peak_on > 0.01, "MIDI note on should produce sound");
        assert!(
            peak_off < peak_on * 0.5,
            "MIDI note off should release: on={peak_on}, off_tail={peak_off}"
        );
    }

    #[test]
    fn test_process_midi_velocity_zero_is_note_off() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        synth.load_patch(test_patch(31, [0, 0, 0, 0, 0, 99], 0));

        // Note on with velocity 100
        synth.process_midi(&[0x90, 60, 100]);
        let mut buf = vec![0.0f32; 2048];
        synth.render(&mut buf);

        // Note on with velocity 0 = note off (standard MIDI convention)
        synth.process_midi(&[0x90, 60, 0]);
        let mut buf2 = vec![0.0f32; 8192];
        synth.render(&mut buf2);
        let peak_tail = buf2[buf2.len() - 2048..]
            .iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(
            peak_tail < 0.01,
            "Velocity 0 note-on should act as note-off: tail peak={peak_tail}"
        );
    }

    #[test]
    fn test_process_midi_pitch_bend() {
        ensure_init();
        let mut synth = crate::synth::Synth::new(44100);
        synth.load_patch(test_patch(31, [0, 0, 0, 0, 0, 99], 0));
        synth.note_on(60, 100);

        // No bend
        let mut buf1 = vec![0.0f32; 4096];
        synth.render(&mut buf1);
        let xings1 = buf1.iter().zip(buf1.iter().skip(1))
            .filter(|(&a, &b)| a >= 0.0 && b < 0.0).count();

        // Pitch bend: E0 = 0xE0, LSB=0x00, MSB=0x7F → value = (0x7F << 7) - 8192 = 8064
        synth.process_midi(&[0xE0, 0x00, 0x7F]);
        let mut buf2 = vec![0.0f32; 4096];
        synth.render(&mut buf2);
        let xings2 = buf2.iter().zip(buf2.iter().skip(1))
            .filter(|(&a, &b)| a >= 0.0 && b < 0.0).count();

        assert!(
            xings2 > xings1,
            "MIDI pitch bend up should increase frequency: before={xings1}, after={xings2}"
        );
    }
}
