//! DX7 pitch envelope generator.
//!
//! Based on MSFA pitchenv.cc (Apache 2.0, Google Inc.).
//! Output is in Q24/octave format.

/// Pitch envelope rate table (maps DX7 rate 0..99 to internal rate).
static PITCHENV_RATE: [u8; 100] = [
    1, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12,
    12, 13, 13, 14, 14, 15, 16, 16, 17, 18, 18, 19, 20, 21, 22, 23, 24,
    25, 26, 27, 28, 30, 31, 33, 34, 36, 37, 38, 39, 41, 42, 44, 46, 47,
    49, 51, 53, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 79, 82,
    85, 88, 91, 94, 98, 102, 106, 110, 115, 120, 125, 130, 135, 141, 147,
    153, 159, 165, 171, 178, 185, 193, 202, 211, 232, 243, 254, 255,
];

/// Pitch envelope level table (maps DX7 level 0..99 to signed offset).
static PITCHENV_TAB: [i8; 100] = [
    -128, -116, -104, -95, -85, -76, -68, -61, -56, -52, -49, -46, -43,
    -41, -39, -37, -35, -33, -32, -31, -30, -29, -28, -27, -26, -25, -24,
    -23, -22, -21, -20, -19, -18, -17, -16, -15, -14, -13, -12, -11, -10,
    -9, -8, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10,
    11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 33, 34, 35, 38, 40, 43, 46, 49, 53, 58, 65, 73,
    82, 92, 103, 115, 127,
];

/// Static unit derived from sample rate.
static mut PITCHENV_UNIT: i32 = 0;

/// Initialize pitch envelope statics (called once from init_tables flow).
/// Only 44100 and 48000 Hz are supported.
pub fn init_pitchenv(sample_rate: u32) {
    unsafe {
        PITCHENV_UNIT = match sample_rate {
            // N * (1<<24) / (21.3 * 48000) + 0.5 = 1049
            48000 => 1049,
            // N * (1<<24) / (21.3 * 44100) + 0.5 = 1143
            _ => 1143,
        };
    }
}

/// DX7 pitch envelope generator.
pub struct PitchEnv {
    rates: [i32; 4],
    levels: [i32; 4],
    level: i32,
    targetlevel: i32,
    rising: bool,
    ix: i32,
    inc: i32,
    down: bool,
}

impl PitchEnv {
    pub fn new() -> Self {
        Self {
            rates: [0; 4],
            levels: [0; 4],
            level: 0,
            targetlevel: 0,
            rising: false,
            ix: 0,
            inc: 0,
            down: true,
        }
    }

    /// Set pitch envelope parameters from DX7 patch.
    pub fn set(&mut self, rates: &[i32; 4], levels: &[i32; 4]) {
        for i in 0..4 {
            self.rates[i] = rates[i];
            self.levels[i] = levels[i];
        }
        self.level = (PITCHENV_TAB[levels[3] as usize] as i32) << 19;
        self.down = true;
        self.advance(0);
    }

    /// Get one sample of pitch envelope (call once per N-sample block).
    /// Returns Q24/octave pitch offset.
    #[inline]
    pub fn getsample(&mut self) -> i32 {
        if self.ix < 3 || (self.ix < 4 && !self.down) {
            if self.rising {
                self.level += self.inc;
                if self.level >= self.targetlevel {
                    self.level = self.targetlevel;
                    self.advance(self.ix + 1);
                }
            } else {
                self.level -= self.inc;
                if self.level <= self.targetlevel {
                    self.level = self.targetlevel;
                    self.advance(self.ix + 1);
                }
            }
        }
        self.level
    }

    /// Handle key down/up.
    pub fn keydown(&mut self, d: bool) {
        if self.down != d {
            self.down = d;
            self.advance(if d { 0 } else { 3 });
        }
    }

    fn advance(&mut self, newix: i32) {
        self.ix = newix;
        if self.ix < 4 {
            let newlevel = self.levels[self.ix as usize];
            self.targetlevel = (PITCHENV_TAB[newlevel as usize] as i32) << 19;
            self.rising = self.targetlevel > self.level;
            let unit = unsafe { PITCHENV_UNIT };
            self.inc = PITCHENV_RATE[self.rates[self.ix as usize] as usize] as i32 * unit;
        }
    }
}
