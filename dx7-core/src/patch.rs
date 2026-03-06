//! DX7 voice patch parameters and SysEx parsing.

use crate::envelope::EnvParams;
use crate::lfo::{LfoParams, LfoWaveform};
use crate::operator::{OperatorParams, ScalingCurve};

/// Normalize a SysEx parameter value to its valid range.
/// Matches Dexed's `normparm()`: values within range pass through,
/// out-of-range values are rescaled from 0-255 to 0-max.
/// Note: Dexed has a bug where memcpy overwrites normparm'd operator params,
/// so operator EG rates/levels/depths are NOT actually normalized in Dexed.
/// We apply normparm to global params (pitch EG, algorithm) where Dexed does too.
fn normparm(value: u8, max: u8) -> u8 {
    if value <= max {
        value
    } else {
        (value as u16 * max as u16 / 255) as u8
    }
}

/// Complete DX7 voice parameters.
#[derive(Clone, Debug)]
pub struct DxVoice {
    pub operators: [OperatorParams; 6],
    pub pitch_eg: EnvParams,
    pub algorithm: u8,              // 0-31
    pub feedback: u8,               // 0-7
    pub osc_key_sync: bool,
    pub lfo: LfoParams,
    pub pitch_mod_sensitivity: u8,  // 0-7
    pub transpose: u8,              // 0-48 (24=C3)
    pub name: [u8; 10],
}

impl Default for DxVoice {
    fn default() -> Self {
        Self::init_voice()
    }
}

impl DxVoice {
    /// Create the default INIT VOICE patch (simple sine on OP1).
    pub fn init_voice() -> Self {
        let mut ops: [OperatorParams; 6] = [
            OperatorParams::default(),
            OperatorParams::default(),
            OperatorParams::default(),
            OperatorParams::default(),
            OperatorParams::default(),
            OperatorParams::default(),
        ];

        // Only OP1 (index 5) is active with full level, rest are silent
        // Index 0 = OP6, index 5 = OP1 (matching MSFA/Dexed convention)
        for op in ops.iter_mut().take(5) {
            op.output_level = 0;
        }

        Self {
            operators: ops,
            pitch_eg: EnvParams {
                rates: [99, 99, 99, 99],
                levels: [50, 50, 50, 50],
            },
            algorithm: 0,           // Algorithm 1
            feedback: 0,
            osc_key_sync: true,
            lfo: LfoParams::default(),
            pitch_mod_sensitivity: 0,
            transpose: 24,          // C3
            name: *b"INIT VOICE",
        }
    }

    /// Get the voice name as a string.
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name).unwrap_or("??????????")
    }

    /// Parse a single voice from unpacked format (155 bytes).
    /// This is the format used in single voice SysEx dumps.
    /// Operators are stored OP6-first: index 0 = OP6, matching MSFA/Dexed convention.
    pub fn from_unpacked(data: &[u8; 155]) -> Self {
        let mut ops = [OperatorParams::default(); 6];

        // Each operator is 21 bytes, stored OP6 first (index 0 = OP6)
        for i in 0..6 {
            let op_idx = i; // index 0 = OP6, index 5 = OP1
            let base = i * 21;
            ops[op_idx] = OperatorParams {
                eg: EnvParams {
                    rates: [data[base], data[base + 1], data[base + 2], data[base + 3]],
                    levels: [data[base + 4], data[base + 5], data[base + 6], data[base + 7]],
                },
                kbd_level_scaling_break_point: data[base + 8],
                kbd_level_scaling_left_depth: data[base + 9],
                kbd_level_scaling_right_depth: data[base + 10],
                kbd_level_scaling_left_curve: ScalingCurve::from_u8(data[base + 11]),
                kbd_level_scaling_right_curve: ScalingCurve::from_u8(data[base + 12]),
                kbd_rate_scaling: data[base + 13],
                amp_mod_sensitivity: data[base + 14],
                key_velocity_sensitivity: data[base + 15],
                output_level: data[base + 16],
                osc_mode: data[base + 17],
                osc_freq_coarse: data[base + 18],
                osc_freq_fine: data[base + 19],
                osc_detune: data[base + 20],
            };
        }

        let gb = 126; // Global params start at byte 126

        DxVoice {
            operators: ops,
            pitch_eg: EnvParams {
                rates: [data[gb], data[gb + 1], data[gb + 2], data[gb + 3]],
                levels: [data[gb + 4], data[gb + 5], data[gb + 6], data[gb + 7]],
            },
            algorithm: data[gb + 8],
            feedback: data[gb + 9],
            osc_key_sync: data[gb + 10] != 0,
            lfo: LfoParams {
                speed: data[gb + 11],
                delay: data[gb + 12],
                pitch_mod_depth: data[gb + 13],
                amp_mod_depth: data[gb + 14],
                key_sync: data[gb + 15] != 0,
                waveform: LfoWaveform::from_u8(data[gb + 16]),
            },
            pitch_mod_sensitivity: data[gb + 17],
            transpose: data[gb + 18],
            name: {
                let mut name = [b' '; 10];
                name.copy_from_slice(&data[gb + 19..gb + 29]);
                name
            },
        }
    }

    /// Serialize to packed format (128 bytes, same as bulk dump per-voice).
    pub fn to_packed(&self) -> [u8; 128] {
        let mut d = [0u8; 128];

        for i in 0..6 {
            let op = &self.operators[i];
            let base = i * 17;

            // EG rates and levels (bytes 0-7)
            d[base]     = op.eg.rates[0];
            d[base + 1] = op.eg.rates[1];
            d[base + 2] = op.eg.rates[2];
            d[base + 3] = op.eg.rates[3];
            d[base + 4] = op.eg.levels[0];
            d[base + 5] = op.eg.levels[1];
            d[base + 6] = op.eg.levels[2];
            d[base + 7] = op.eg.levels[3];

            d[base + 8]  = op.kbd_level_scaling_break_point;
            d[base + 9]  = op.kbd_level_scaling_left_depth;
            d[base + 10] = op.kbd_level_scaling_right_depth;

            // Byte 11: left_curve[1:0] | right_curve[3:2]
            d[base + 11] = op.kbd_level_scaling_left_curve.to_u8()
                         | (op.kbd_level_scaling_right_curve.to_u8() << 2);

            // Byte 12: rate_scaling[2:0] | detune[6:3]
            d[base + 12] = (op.kbd_rate_scaling & 0x07)
                         | ((op.osc_detune & 0x0F) << 3);

            // Byte 13: amp_mod_sens[1:0] | key_vel_sens[4:2]
            d[base + 13] = (op.amp_mod_sensitivity & 0x03)
                         | ((op.key_velocity_sensitivity & 0x07) << 2);

            d[base + 14] = op.output_level;

            // Byte 15: osc_mode[0] | freq_coarse[5:1]
            d[base + 15] = (op.osc_mode & 0x01)
                         | ((op.osc_freq_coarse & 0x1F) << 1);

            d[base + 16] = op.osc_freq_fine;
        }

        let gb = 102;

        // Pitch EG
        d[gb]     = self.pitch_eg.rates[0];
        d[gb + 1] = self.pitch_eg.rates[1];
        d[gb + 2] = self.pitch_eg.rates[2];
        d[gb + 3] = self.pitch_eg.rates[3];
        d[gb + 4] = self.pitch_eg.levels[0];
        d[gb + 5] = self.pitch_eg.levels[1];
        d[gb + 6] = self.pitch_eg.levels[2];
        d[gb + 7] = self.pitch_eg.levels[3];

        // Algorithm
        d[gb + 8] = self.algorithm & 0x1F;

        // OscKeySync[3] | Feedback[2:0]
        d[gb + 9] = (self.feedback & 0x07)
                   | (if self.osc_key_sync { 0x08 } else { 0 });

        // LFO
        d[gb + 10] = self.lfo.speed;
        d[gb + 11] = self.lfo.delay;
        d[gb + 12] = self.lfo.pitch_mod_depth;
        d[gb + 13] = self.lfo.amp_mod_depth;

        // LFO PitchModSens[6:4] | LFO Wave[3:1] | LFO Sync[0]
        d[gb + 14] = (if self.lfo.key_sync { 1 } else { 0 })
                    | ((self.lfo.waveform.to_u8() & 0x07) << 1)
                    | ((self.pitch_mod_sensitivity & 0x07) << 4);

        d[gb + 15] = self.transpose;

        // Name (bytes 118-127)
        d[gb + 16..gb + 26].copy_from_slice(&self.name);

        d
    }

    /// Convert to Dexed VMEM (unpacked) format: 156 bytes.
    /// 6 operators × 21 bytes + 30 global bytes.
    /// Op order: index 0 = OP6, index 5 = OP1 (same as MSFA convention).
    pub fn to_unpacked(&self) -> [u8; 156] {
        let mut d = [0u8; 156];
        for i in 0..6 {
            let op = &self.operators[i];
            let off = i * 21;
            d[off]      = op.eg.rates[0];
            d[off + 1]  = op.eg.rates[1];
            d[off + 2]  = op.eg.rates[2];
            d[off + 3]  = op.eg.rates[3];
            d[off + 4]  = op.eg.levels[0];
            d[off + 5]  = op.eg.levels[1];
            d[off + 6]  = op.eg.levels[2];
            d[off + 7]  = op.eg.levels[3];
            d[off + 8]  = op.kbd_level_scaling_break_point;
            d[off + 9]  = op.kbd_level_scaling_left_depth;
            d[off + 10] = op.kbd_level_scaling_right_depth;
            d[off + 11] = op.kbd_level_scaling_left_curve.to_u8();
            d[off + 12] = op.kbd_level_scaling_right_curve.to_u8();
            d[off + 13] = op.kbd_rate_scaling;
            d[off + 14] = op.amp_mod_sensitivity;
            d[off + 15] = op.key_velocity_sensitivity;
            d[off + 16] = op.output_level;
            d[off + 17] = op.osc_mode;
            d[off + 18] = op.osc_freq_coarse;
            d[off + 19] = op.osc_freq_fine;
            d[off + 20] = op.osc_detune;
        }
        // Pitch EG
        d[126] = self.pitch_eg.rates[0];
        d[127] = self.pitch_eg.rates[1];
        d[128] = self.pitch_eg.rates[2];
        d[129] = self.pitch_eg.rates[3];
        d[130] = self.pitch_eg.levels[0];
        d[131] = self.pitch_eg.levels[1];
        d[132] = self.pitch_eg.levels[2];
        d[133] = self.pitch_eg.levels[3];
        // Global
        d[134] = self.algorithm;
        d[135] = self.feedback;
        d[136] = if self.osc_key_sync { 1 } else { 0 };
        d[137] = self.lfo.speed;
        d[138] = self.lfo.delay;
        d[139] = self.lfo.pitch_mod_depth;
        d[140] = self.lfo.amp_mod_depth;
        d[141] = if self.lfo.key_sync { 1 } else { 0 };
        d[142] = self.lfo.waveform.to_u8();
        d[143] = self.pitch_mod_sensitivity;
        d[144] = self.transpose;
        d[145..155].copy_from_slice(&self.name);
        d
    }

    /// Parse a single voice from packed format (128 bytes per voice in bulk dump).
    pub fn from_packed(data: &[u8; 128]) -> Self {
        let mut ops = [OperatorParams::default(); 6];

        // Each operator is 17 bytes in packed format, stored OP6 first (index 0 = OP6)
        for i in 0..6 {
            let op_idx = i; // index 0 = OP6, index 5 = OP1
            let base = i * 17;

            // Operator bytes 0-10 pass through with 7-bit mask (matching Dexed's
            // memcpy that overwrites normparm'd values — see Dexed issue #215).
            let eg = EnvParams {
                rates: [
                    data[base] & 0x7F,
                    data[base + 1] & 0x7F,
                    data[base + 2] & 0x7F,
                    data[base + 3] & 0x7F,
                ],
                levels: [
                    data[base + 4] & 0x7F,
                    data[base + 5] & 0x7F,
                    data[base + 6] & 0x7F,
                    data[base + 7] & 0x7F,
                ],
            };

            let bp = data[base + 8] & 0x7F;
            let ld = data[base + 9] & 0x7F;
            let rd = data[base + 10] & 0x7F;

            // Byte 11: left_curve[1:0] | right_curve[3:2]
            let curves = data[base + 11] & 0x0F;
            let left_curve = ScalingCurve::from_u8(curves & 0x03);
            let right_curve = ScalingCurve::from_u8((curves >> 2) & 0x03);

            // Byte 12: rate_scaling[2:0] | detune[6:3]
            let rs_det = data[base + 12] & 0x7F;
            let rate_scaling = rs_det & 0x07;
            let detune = (rs_det >> 3) & 0x0F;

            // Byte 13: amp_mod_sens[1:0] | key_vel_sens[4:2]
            let ams_kvs = data[base + 13] & 0x1F;
            let amp_mod_sensitivity = ams_kvs & 0x03;
            let key_velocity_sensitivity = (ams_kvs >> 2) & 0x07;

            let output_level = data[base + 14] & 0x7F;

            // Byte 15: osc_mode[0] | freq_coarse[5:1]
            let mode_coarse = data[base + 15] & 0x3F;
            let osc_mode = mode_coarse & 0x01;
            let freq_coarse = (mode_coarse >> 1) & 0x1F;

            let freq_fine = data[base + 16] & 0x7F;

            ops[op_idx] = OperatorParams {
                eg,
                kbd_level_scaling_break_point: bp,
                kbd_level_scaling_left_depth: ld,
                kbd_level_scaling_right_depth: rd,
                kbd_level_scaling_left_curve: left_curve,
                kbd_level_scaling_right_curve: right_curve,
                kbd_rate_scaling: rate_scaling,
                amp_mod_sensitivity,
                key_velocity_sensitivity,
                output_level,
                osc_mode,
                osc_freq_coarse: freq_coarse,
                osc_freq_fine: freq_fine,
                osc_detune: detune,
            };
        }

        let gb = 102; // Global params at byte 102

        // Byte 110 (gb+8): Algorithm[4:0] — 5 bits, value 0-31
        let algorithm = data[gb + 8] & 0x1F;

        // Byte 111 (gb+9): OscKeySync[3] | Feedback[2:0]
        let fb_sync = data[gb + 9];
        let feedback = fb_sync & 0x07;
        let osc_key_sync = (fb_sync >> 3) & 0x01 != 0;

        // Byte 116 (gb+14): LFO PitchModSens[6:4] | LFO Wave[3:1] | LFO Sync[0]
        let ls_lw_pms = data[gb + 14];
        let lfo_key_sync = ls_lw_pms & 0x01 != 0;
        let lfo_waveform = LfoWaveform::from_u8((ls_lw_pms >> 1) & 0x07);
        let pitch_mod_sensitivity = (ls_lw_pms >> 4) & 0x07;

        DxVoice {
            operators: ops,
            // Pitch EG: normparm to 99 (Dexed does apply this)
            pitch_eg: EnvParams {
                rates: [
                    normparm(data[gb] & 0x7F, 99),
                    normparm(data[gb + 1] & 0x7F, 99),
                    normparm(data[gb + 2] & 0x7F, 99),
                    normparm(data[gb + 3] & 0x7F, 99),
                ],
                levels: [
                    normparm(data[gb + 4] & 0x7F, 99),
                    normparm(data[gb + 5] & 0x7F, 99),
                    normparm(data[gb + 6] & 0x7F, 99),
                    normparm(data[gb + 7] & 0x7F, 99),
                ],
            },
            algorithm,
            feedback,
            osc_key_sync,
            lfo: LfoParams {
                speed: normparm(data[gb + 10], 99),
                delay: normparm(data[gb + 11], 99),
                pitch_mod_depth: normparm(data[gb + 12], 99),
                amp_mod_depth: normparm(data[gb + 13], 99),
                key_sync: lfo_key_sync,
                waveform: lfo_waveform,
            },
            pitch_mod_sensitivity,
            transpose: normparm(data[gb + 15], 48),
            name: {
                let mut name = [b' '; 10];
                let name_start = gb + 16;
                let name_end = (name_start + 10).min(128);
                let len = name_end - name_start;
                name[..len].copy_from_slice(&data[name_start..name_end]);
                name
            },
        }
    }

    /// Parse a 32-voice bulk dump SysEx message.
    /// Expected format: F0 43 0s 09 20 00 <4096 bytes> <checksum> F7
    /// Returns up to 32 voices, or an error message.
    #[cfg(feature = "std")]
    pub fn parse_bulk_dump(data: &[u8]) -> Result<Vec<DxVoice>, &'static str> {
        if data.len() < 4104 {
            return Err("Data too short for bulk dump");
        }

        // Check SysEx header
        if data[0] != 0xF0 {
            return Err("Missing SysEx start byte (F0)");
        }
        if data[1] != 0x43 {
            return Err("Not a Yamaha SysEx message");
        }
        // data[2] = sub-status + channel (0s where s=channel)
        if data[3] != 0x09 {
            return Err("Not a 32-voice bulk dump (format byte)");
        }
        // Byte count: 0x20 0x00 = 8192? Actually 4096 = 0x10 0x00
        // DX7 uses: byte_count_msb=0x20, byte_count_lsb=0x00 → 0x2000 = 8192
        // But actual data is 4096 bytes. Let's be flexible.

        let voice_data_start = 6;
        let voice_data_end = voice_data_start + 4096;

        if data.len() < voice_data_end + 2 {
            return Err("Data too short");
        }

        // Verify checksum: two's complement of sum of voice data, AND 0x7F
        let sum: u8 = data[voice_data_start..voice_data_end]
            .iter()
            .fold(0u8, |acc, &b| acc.wrapping_add(b));
        let expected_checksum = (!sum).wrapping_add(1) & 0x7F;
        let actual_checksum = data[voice_data_end];

        if actual_checksum != expected_checksum {
            // Be lenient — some SysEx files have different checksum conventions
            // Just warn but continue
        }

        // Check SysEx end
        if data[voice_data_end + 1] != 0xF7 {
            // Also be lenient here
        }

        // Parse 32 voices, each 128 bytes
        let mut voices = Vec::with_capacity(32);
        for i in 0..32 {
            let start = voice_data_start + i * 128;
            let mut voice_data = [0u8; 128];
            voice_data.copy_from_slice(&data[start..start + 128]);
            voices.push(DxVoice::from_packed(&voice_data));
        }

        Ok(voices)
    }

    /// Raw packed 128 bytes of "Flunk bass" (voice 25 from vrc107b.syx).
    pub const FLUNK_BASS_PACKED: [u8; 128] = [
        0x62, 0x62, 0x3E, 0x62, 0x63, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x14, 0x63, 0x0C,
        0x00, 0x62, 0x62, 0x26, 0x62, 0x63, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3D, 0x1C, 0x46,
        0x12, 0x00, 0x3B, 0x62, 0x29, 0x47, 0x63, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x35, 0x1C,
        0x3F, 0x12, 0x00, 0x3B, 0x62, 0x62, 0x47, 0x63, 0x63, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3D,
        0x00, 0x4B, 0x00, 0x00, 0x3B, 0x62, 0x16, 0x47, 0x7F, 0x56, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x25, 0x14, 0x48, 0x02, 0x00, 0x62, 0x40, 0x21, 0x47, 0x63, 0x56, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x08, 0x08, 0x63, 0x02, 0x00, 0x62, 0x62, 0x62, 0x62, 0x32, 0x32, 0x32, 0x32, 0x10, 0x0F,
        0x23, 0x00, 0x00, 0x00, 0x31, 0x0C, 0x46, 0x6C, 0x75, 0x6E, 0x6B, 0x20, 0x62, 0x61, 0x73, 0x73,
    ];
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn test_init_voice() {
        let voice = DxVoice::init_voice();
        assert_eq!(voice.algorithm, 0);
        assert_eq!(voice.feedback, 0);
        assert_eq!(voice.name_str(), "INIT VOICE");
        // operators[5] is OP1 (active), operators[0] is OP6 (silent)
        assert_eq!(voice.operators[5].output_level, 99);
        assert_eq!(voice.operators[0].output_level, 0);
    }

    #[test]
    fn test_unpacked_roundtrip() {
        // Create a known unpacked voice data block
        let mut data = [0u8; 155];
        // OP6 (first in data): set some distinctive values
        data[0] = 50; // EG rate 1
        data[16] = 80; // output level

        // Global params at byte 126
        data[126 + 8] = 4; // algorithm 5 (0-indexed = 4)
        data[126 + 9] = 3; // feedback

        // Name
        for (i, &c) in b"TEST PATCH".iter().enumerate() {
            data[126 + 19 + i] = c;
        }

        let voice = DxVoice::from_unpacked(&data);
        assert_eq!(voice.algorithm, 4);
        assert_eq!(voice.feedback, 3);
        assert_eq!(voice.name_str(), "TEST PATCH");
        // OP6 data is at index 0 (first in data = index 0)
        assert_eq!(voice.operators[0].eg.rates[0], 50); // OP6
        assert_eq!(voice.operators[0].output_level, 80);
    }

    #[test]
    fn test_flunk_bass_from_packed() {
        let v = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);

        assert_eq!(v.name_str(), "Flunk bass");
        assert_eq!(v.algorithm, 16, "Algorithm 17 (0-indexed 16)");
        assert_eq!(v.feedback, 7);
        assert!(v.osc_key_sync);
        assert_eq!(v.transpose, 12);

        // OP1 (index 5): carrier, OL=99, coarse=1, fine=0, detune=1, KVS=2
        let op1 = &v.operators[5];
        assert_eq!(op1.output_level, 99);
        assert_eq!(op1.osc_freq_coarse, 1);
        assert_eq!(op1.osc_freq_fine, 0);
        assert_eq!(op1.osc_detune, 1);
        assert_eq!(op1.key_velocity_sensitivity, 2);
        assert_eq!(op1.osc_mode, 0);
        assert_eq!(op1.eg.rates, [98, 64, 33, 71]);
        assert_eq!(op1.eg.levels, [99, 86, 0, 0]);

        // OP2 (index 4): modulator, OL=72, coarse=1
        let op2 = &v.operators[4];
        assert_eq!(op2.output_level, 72);
        assert_eq!(op2.osc_freq_coarse, 1);
        assert_eq!(op2.key_velocity_sensitivity, 5);
        assert_eq!(op2.eg.levels, [127, 86, 0, 0]);

        // OP3 (index 3): modulator, OL=75, coarse=0 (sub-octave 0.5x)
        let op3 = &v.operators[3];
        assert_eq!(op3.output_level, 75);
        assert_eq!(op3.osc_freq_coarse, 0);
        assert_eq!(op3.osc_detune, 7);
        assert_eq!(op3.eg.levels, [99, 99, 99, 0]);

        // OP6 (index 0): feedback source, OL=99, coarse=6
        let op6 = &v.operators[0];
        assert_eq!(op6.output_level, 99);
        assert_eq!(op6.osc_freq_coarse, 6);
        assert_eq!(op6.key_velocity_sensitivity, 5);
        assert_eq!(op6.kbd_rate_scaling, 4);

        // OP5 (index 1): OL=70, coarse=9
        assert_eq!(v.operators[1].output_level, 70);
        assert_eq!(v.operators[1].osc_freq_coarse, 9);

        // OP4 (index 2): OL=63, coarse=9
        assert_eq!(v.operators[2].output_level, 63);
        assert_eq!(v.operators[2].osc_freq_coarse, 9);

        // LFO
        assert_eq!(v.lfo.speed, 35);
        assert_eq!(v.lfo.delay, 0);
        assert_eq!(v.pitch_mod_sensitivity, 3);
    }

    #[test]
    fn test_packed_roundtrip() {
        // Parse Flunk bass, re-pack, re-parse, compare
        let v1 = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);
        let packed = v1.to_packed();
        let v2 = DxVoice::from_packed(&packed);

        assert_eq!(v2.name_str(), v1.name_str());
        assert_eq!(v2.algorithm, v1.algorithm);
        assert_eq!(v2.feedback, v1.feedback);
        assert_eq!(v2.osc_key_sync, v1.osc_key_sync);
        assert_eq!(v2.transpose, v1.transpose);
        for i in 0..6 {
            assert_eq!(v2.operators[i].output_level, v1.operators[i].output_level,
                "OP{} output_level mismatch", 6 - i);
            assert_eq!(v2.operators[i].osc_freq_coarse, v1.operators[i].osc_freq_coarse,
                "OP{} freq_coarse mismatch", 6 - i);
            assert_eq!(v2.operators[i].osc_detune, v1.operators[i].osc_detune,
                "OP{} detune mismatch", 6 - i);
            assert_eq!(v2.operators[i].key_velocity_sensitivity, v1.operators[i].key_velocity_sensitivity,
                "OP{} KVS mismatch", 6 - i);
        }
    }
}
