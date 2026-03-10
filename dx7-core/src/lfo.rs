//! DX7 integer LFO with 6 waveforms.
//!
//! Based on MSFA lfo.cc (Apache 2.0, Google Inc.).
//! All waveform outputs are in Q24 range (0..1<<24).

use crate::generated_tables;
use crate::tables;

/// LFO waveform types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LfoWaveform {
    Triangle,
    SawDown,
    SawUp,
    Square,
    Sine,
    SampleAndHold,
}

impl LfoWaveform {
    pub fn from_u8(v: u8) -> Self {
        match v % 6 {
            0 => LfoWaveform::Triangle,
            1 => LfoWaveform::SawDown,
            2 => LfoWaveform::SawUp,
            3 => LfoWaveform::Square,
            4 => LfoWaveform::Sine,
            5 => LfoWaveform::SampleAndHold,
            _ => unreachable!(),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            LfoWaveform::Triangle => 0,
            LfoWaveform::SawDown => 1,
            LfoWaveform::SawUp => 2,
            LfoWaveform::Square => 3,
            LfoWaveform::Sine => 4,
            LfoWaveform::SampleAndHold => 5,
        }
    }
}

/// LFO parameters (from DX7 patch).
#[derive(Clone, Copy, Debug)]
pub struct LfoParams {
    pub speed: u8,           // 0-99
    pub delay: u8,           // 0-99
    pub pitch_mod_depth: u8, // 0-99
    pub amp_mod_depth: u8,   // 0-99
    pub key_sync: bool,
    pub waveform: LfoWaveform,
}

impl Default for LfoParams {
    fn default() -> Self {
        Self {
            speed: 35,
            delay: 0,
            pitch_mod_depth: 0,
            amp_mod_depth: 0,
            key_sync: true,
            waveform: LfoWaveform::Triangle,
        }
    }
}

/// Static LFO parameters derived from sample rate (shared across all voices).
static mut LFO_UNIT: u32 = 0;
static mut LFO_RATIO: u32 = 0;

/// Initialize LFO statics (called once from init_tables flow).
/// Only 44100 and 48000 Hz are supported.
pub fn init_lfo(sample_rate: u32) {
    unsafe {
        match sample_rate {
            48000 => {
                // N * 25190424 / 48000 + 0.5 = 33587
                LFO_UNIT = 33587;
                // 4437500000 * N / 48000 = 5916666
                LFO_RATIO = 5916666;
            }
            _ => {
                // N * 25190424 / 44100 + 0.5 = 36559
                LFO_UNIT = 36559;
                // 4437500000 * N / 44100 = 6440362
                LFO_RATIO = 6440362;
            }
        }
    }
}

/// DX7 integer LFO state.
pub struct Lfo {
    phase: u32,
    delta: u32,
    waveform: u8,
    randstate: u8,
    sync: bool,
    delaystate: u32,
    delayinc: u32,
    delayinc2: u32,
}

impl Lfo {
    pub fn new() -> Self {
        Self {
            phase: 0,
            delta: 0,
            waveform: 0,
            randstate: 0,
            sync: false,
            delaystate: 0,
            delayinc: 0,
            delayinc2: 0,
        }
    }

    /// Reset LFO with patch parameters.
    /// `params` layout: [speed, delay, pmd, amd, sync, waveform]
    pub fn reset(&mut self, lfo_params: &LfoParams) {
        let rate = lfo_params.speed.min(99) as usize;
        let lforatio = unsafe { LFO_RATIO };
        // LFO_SOURCE_FIXED is Q24; multiply and shift back.
        self.delta = ((generated_tables::LFO_SOURCE_FIXED[rate] as u64 * lforatio as u64) >> 24) as u32;

        let a_raw = 99i32 - lfo_params.delay as i32;
        let unit = unsafe { LFO_UNIT };
        if a_raw == 99 {
            self.delayinc = !0u32;
            self.delayinc2 = !0u32;
        } else {
            let mut a = ((16 + (a_raw & 15)) << (1 + (a_raw >> 4))) as u32;
            self.delayinc = unit.wrapping_mul(a);
            a &= 0xff80;
            if a < 0x80 {
                a = 0x80;
            }
            self.delayinc2 = unit.wrapping_mul(a);
        }

        self.waveform = lfo_params.waveform.to_u8();
        self.sync = lfo_params.key_sync;
    }

    /// Get one LFO sample. Result is 0..1 in Q24.
    #[inline]
    pub fn getsample(&mut self) -> i32 {
        self.phase = self.phase.wrapping_add(self.delta);

        match self.waveform {
            0 => {
                // Triangle
                let mut x = (self.phase >> 7) as i32;
                x ^= -((self.phase >> 31) as i32);
                x & ((1 << 24) - 1)
            }
            1 => {
                // Sawtooth down
                ((!self.phase ^ (1u32 << 31)) >> 8) as i32
            }
            2 => {
                // Sawtooth up
                ((self.phase ^ (1u32 << 31)) >> 8) as i32
            }
            3 => {
                // Square
                (((!self.phase) >> 7) & (1u32 << 24)) as i32
            }
            4 => {
                // Sine
                (1 << 23) + (tables::sin_lookup((self.phase >> 8) as i32) >> 1)
            }
            5 => {
                // Sample & Hold
                if self.phase < self.delta {
                    self.randstate = ((self.randstate as u32 * 179 + 17) & 0xff) as u8;
                }
                let x = (self.randstate ^ 0x80) as i32;
                (x + 1) << 16
            }
            _ => 1 << 23,
        }
    }

    /// Get delay ramp value. Result is 0..1 in Q24.
    #[inline]
    pub fn getdelay(&mut self) -> i32 {
        let delta = if self.delaystate < (1u32 << 31) {
            self.delayinc
        } else {
            self.delayinc2
        };
        let d = self.delaystate as u64 + delta as u64;
        if d > u32::MAX as u64 {
            return 1 << 24;
        }
        self.delaystate = d as u32;
        if (d as u32) < (1u32 << 31) {
            0
        } else {
            ((d as u32) >> 7) as i32 & ((1 << 24) - 1)
        }
    }

    /// Handle key-down event (reset phase if sync, reset delay).
    pub fn keydown(&mut self) {
        if self.sync {
            self.phase = (1u32 << 31) - 1;
        }
        self.delaystate = 0;
    }
}
