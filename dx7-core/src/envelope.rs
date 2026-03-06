//! DX7 integer log-domain envelope generator.
//!
//! Ported from Dexed/MSFA env.cc (Apache 2.0, Google Inc. / Pascal Gauthier).
//! Level is stored in Q24/doubling log format. Output is subsampled once per
//! N=64 sample block.

use crate::tables;

/// DX7 envelope parameters: 4 rates and 4 levels (all 0-99).
#[derive(Clone, Copy, Debug)]
pub struct EnvParams {
    pub rates: [u8; 4],
    pub levels: [u8; 4],
}

impl Default for EnvParams {
    fn default() -> Self {
        Self {
            rates: [99, 99, 99, 99],
            levels: [99, 99, 99, 0],
        }
    }
}

/// Level scaling lookup (maps 0..19 output levels to scaled values).
const LEVEL_LUT: [i32; 20] = [
    0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46,
];

/// Empirically measured sample counts at 44.1 kHz for accurate DX7 envelope
/// stage-boundary timing. Gathered from DX7 hardware using two Yamaha TF1 chips.
/// (Ported from Dexed/MSFA env.cc, Apache 2.0.)
const STATICS: [i32; 77] = [
    1764000, 1764000, 1411200, 1411200, 1190700, 1014300, 992250,
    882000, 705600, 705600, 584325, 507150, 502740, 441000, 418950,
    352800, 308700, 286650, 253575, 220500, 220500, 176400, 145530,
    145530, 125685, 110250, 110250, 88200, 88200, 74970, 61740,
    61740, 55125, 48510, 44100, 37485, 31311, 30870, 27562, 27562,
    22050, 18522, 17640, 15435, 14112, 13230, 11025, 9261, 9261, 7717,
    6615, 6615, 5512, 5512, 4410, 3969, 3969, 3439, 2866, 2690, 2249,
    1984, 1896, 1808, 1411, 1367, 1234, 1146, 926, 837, 837, 705,
    573, 573, 529, 441, 441,
];

/// Scale an output level (0..99) to envelope units.
pub fn scaleoutlevel(outlevel: i32) -> i32 {
    if outlevel >= 20 {
        28 + outlevel
    } else {
        LEVEL_LUT[outlevel as usize]
    }
}

/// DX7 envelope generator — integer log-domain.
#[derive(Clone, Debug)]
pub struct Envelope {
    rates: [i32; 4],
    levels: [i32; 4],
    outlevel: i32,
    rate_scaling: i32,
    /// Level in Q24/doubling log format (16 more fractional bits than DX7).
    level: i32,
    targetlevel: i32,
    rising: bool,
    /// Current stage index (0..3 = attack/decay1/decay2/release, 4 = done).
    ix: i32,
    /// Level increment per N-sample block.
    inc: i32,
    /// Accurate envelope: remaining samples to hold at stage boundary.
    staticcount: i32,
    /// Key state: true = held, false = released.
    down: bool,
}

impl Envelope {
    pub fn new() -> Self {
        Self {
            rates: [0; 4],
            levels: [0; 4],
            outlevel: 0,
            rate_scaling: 0,
            level: 0,
            targetlevel: 0,
            rising: false,
            ix: 0,
            inc: 0,
            staticcount: 0,
            down: true,
        }
    }

    /// Initialize envelope with DX7 parameters.
    /// `rates`/`levels`: 0..99 values from patch.
    /// `outlevel`: pre-computed in microsteps (output_level scaled + velocity + level_scaling).
    /// `rate_scaling`: qRate delta from keyboard position.
    pub fn init(&mut self, rates: &[i32; 4], levels: &[i32; 4], outlevel: i32, rate_scaling: i32) {
        for i in 0..4 {
            self.rates[i] = rates[i];
            self.levels[i] = levels[i];
        }
        self.outlevel = outlevel;
        self.rate_scaling = rate_scaling;
        self.level = 0;
        self.down = true;
        self.advance(0);
    }

    /// Get one sample of envelope output (call once per N-sample block).
    /// Returns level in Q24/doubling log format.
    #[inline]
    pub fn getsample(&mut self) -> i32 {
        // Accurate envelope: count down static hold at stage boundaries
        if self.staticcount != 0 {
            self.staticcount -= tables::N as i32;
            if self.staticcount <= 0 {
                self.staticcount = 0;
                let next = self.ix + 1;
                self.advance(next);
            }
        }

        if self.ix < 3 || (self.ix < 4 && !self.down) {
            if self.staticcount != 0 {
                // hold — skip rising/falling while static timer is active
            } else if self.rising {
                const JUMPTARGET: i32 = 1716;
                if self.level < (JUMPTARGET << 16) {
                    self.level = JUMPTARGET << 16;
                }
                self.level +=
                    (((17 << 24) - self.level) >> 24) * self.inc;
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

    /// Handle key down/up transitions.
    pub fn keydown(&mut self, d: bool) {
        if self.down != d {
            self.down = d;
            self.advance(if d { 0 } else { 3 });
        }
    }

    /// Returns true if the envelope is active (not yet finished or L4 > 0).
    pub fn is_active(&self) -> bool {
        self.ix < 4 || self.levels[3] > 0
    }

    fn advance(&mut self, newix: i32) {
        self.ix = newix;
        if self.ix < 4 {
            let newlevel = self.levels[self.ix as usize];
            let actuallevel = scaleoutlevel(newlevel) >> 1;
            let mut actuallevel = (actuallevel << 6) + self.outlevel - 4256;
            if actuallevel < 16 {
                actuallevel = 16;
            }
            self.targetlevel = actuallevel << 16;
            self.rising = self.targetlevel > self.level;

            // Rate computation
            let mut qrate = (self.rates[self.ix as usize] * 41) >> 6;
            qrate += self.rate_scaling;
            if qrate > 63 {
                qrate = 63;
            }

            // Accurate envelope: hold at stage boundary when target == level
            // or on initial attack with level 0 (empirical DX7 hardware timing)
            if self.targetlevel == self.level || (self.ix == 0 && newlevel == 0) {
                let mut staticrate = self.rates[self.ix as usize];
                staticrate += self.rate_scaling;
                if staticrate > 99 { staticrate = 99; }
                let mut sc = if staticrate < 77 {
                    STATICS[staticrate as usize]
                } else {
                    20 * (99 - staticrate)
                };
                if staticrate < 77 && self.ix == 0 && newlevel == 0 {
                    sc /= 20; // attack is scaled faster
                }
                let sr_mul = tables::sr_multiplier() as i64;
                self.staticcount = ((sc as i64 * sr_mul) >> 24) as i32;
            } else {
                self.staticcount = 0;
            }

            self.inc = (4 + (qrate & 3)) << (2 + tables::LG_N + (qrate >> 2));
            // Sample rate compensation
            let sr_mul = tables::sr_multiplier() as i64;
            self.inc = (((self.inc as i64) * sr_mul) >> 24) as i32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_init() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            tables::init_tables(44100);
        });
    }

    #[test]
    fn test_envelope_attack_decay() {
        ensure_init();
        let rates = [99, 99, 99, 99];
        let levels = [99, 99, 99, 0];

        // Compute outlevel for output_level=99, no velocity/scaling
        let outlevel = scaleoutlevel(99) << 5;

        let mut env = Envelope::new();
        env.init(&rates, &levels, outlevel, 0);
        env.keydown(true);

        // Run through many blocks
        let mut last_level = 0i32;
        for _ in 0..1000 {
            last_level = env.getsample();
        }
        // With fast rates, should reach a high level (log domain)
        assert!(last_level > 1 << 22, "Level should be high after fast attack, got {last_level}");

        // Key off
        env.keydown(false);
        for _ in 0..1000 {
            last_level = env.getsample();
        }
        // In log domain, the minimum floor is 16 << 16 = 1048576.
        // After release, level should reach the floor (L4=0 target).
        let floor = 16 << 16; // MSFA envelope minimum
        assert!(
            last_level <= floor,
            "Level should be at floor after release, got {last_level} (floor={floor})"
        );
    }

    #[test]
    fn test_scaleoutlevel() {
        assert_eq!(scaleoutlevel(0), 0);
        assert_eq!(scaleoutlevel(20), 48);
        assert_eq!(scaleoutlevel(99), 127);
    }

    #[test]
    fn test_envelope_sustain_holds() {
        ensure_init();
        // L3=99 means sustain should hold at a high level indefinitely
        let rates = [99, 99, 50, 99]; // R3=50: moderate rate to reach L3
        let levels = [99, 99, 99, 0]; // L3=99: sustain at max

        let outlevel = scaleoutlevel(99) << 5;
        let mut env = Envelope::new();
        env.init(&rates, &levels, outlevel, 0);
        env.keydown(true);

        // Run 2000 blocks to reach sustain
        let mut level = 0;
        for _ in 0..2000 {
            level = env.getsample();
        }
        let sustain_level = level;

        // Run 2000 more blocks — should stay at same level
        for _ in 0..2000 {
            level = env.getsample();
        }
        assert_eq!(
            level, sustain_level,
            "Sustain should hold steady: was {sustain_level}, now {level}"
        );
        assert!(
            sustain_level > 1 << 22,
            "Sustain level should be high, got {sustain_level}"
        );
    }

    #[test]
    fn test_envelope_sustain_zero_decays() {
        ensure_init();
        // L3=0 means the envelope decays to minimum during key-held phase
        let rates = [99, 99, 50, 99]; // R3=50
        let levels = [99, 99, 0, 0];  // L3=0: decay to silence while held

        let outlevel = scaleoutlevel(99) << 5;
        let mut env = Envelope::new();
        env.init(&rates, &levels, outlevel, 0);
        env.keydown(true);

        // Run enough blocks for R3=50 to reach target
        let mut level = 0;
        for _ in 0..5000 {
            level = env.getsample();
        }

        let floor = 16 << 16;
        assert!(
            level <= floor + (1 << 16),
            "With L3=0, envelope should decay to floor. Got {level}, floor={floor}"
        );
    }

    #[test]
    fn test_envelope_different_sustain_levels() {
        ensure_init();
        let outlevel = scaleoutlevel(99) << 5;

        // Test several L3 values to ensure higher L3 = higher sustain
        let mut sustain_levels = Vec::new();
        for &l3 in &[0, 50, 80, 99] {
            let rates = [99, 99, 99, 99];
            let levels = [99, 99, l3, 0];

            let mut env = Envelope::new();
            env.init(&rates, &levels, outlevel, 0);
            env.keydown(true);

            let mut level = 0;
            for _ in 0..3000 {
                level = env.getsample();
            }
            sustain_levels.push((l3, level));
        }

        // Each higher L3 should give a higher sustain level
        for i in 1..sustain_levels.len() {
            let (l3_prev, lev_prev) = sustain_levels[i - 1];
            let (l3_curr, lev_curr) = sustain_levels[i];
            assert!(
                lev_curr > lev_prev,
                "L3={l3_curr} should sustain higher than L3={l3_prev}: {lev_curr} vs {lev_prev}"
            );
        }
    }

    #[test]
    fn test_envelope_release_from_sustain() {
        ensure_init();
        let rates = [99, 99, 99, 50]; // R4=50: moderate release
        let levels = [99, 99, 99, 0]; // Sustain at 99, release to 0

        let outlevel = scaleoutlevel(99) << 5;
        let mut env = Envelope::new();
        env.init(&rates, &levels, outlevel, 0);
        env.keydown(true);

        // Reach sustain
        for _ in 0..2000 {
            env.getsample();
        }
        let sustain_level = env.getsample();
        assert!(sustain_level > 1 << 22, "Should be sustaining high");

        // Release
        env.keydown(false);
        let mut level = sustain_level;
        for _ in 0..5000 {
            level = env.getsample();
        }

        let floor = 16 << 16;
        assert!(
            level <= floor + (1 << 16),
            "After release, should decay to floor. Got {level}"
        );
        assert!(
            level < sustain_level,
            "Release should be lower than sustain: {level} vs {sustain_level}"
        );
    }

    #[test]
    fn test_envelope_slow_rate_is_slower() {
        ensure_init();
        let outlevel = scaleoutlevel(99) << 5;

        // Fast attack
        let mut env_fast = Envelope::new();
        env_fast.init(&[99, 99, 99, 99], &[99, 99, 99, 0], outlevel, 0);
        env_fast.keydown(true);

        // Slow attack
        let mut env_slow = Envelope::new();
        env_slow.init(&[30, 99, 99, 99], &[99, 99, 99, 0], outlevel, 0);
        env_slow.keydown(true);

        // After 50 blocks, fast should be higher than slow
        let mut level_fast = 0;
        let mut level_slow = 0;
        for _ in 0..50 {
            level_fast = env_fast.getsample();
            level_slow = env_slow.getsample();
        }

        assert!(
            level_fast > level_slow,
            "R1=99 should rise faster than R1=30: fast={level_fast}, slow={level_slow}"
        );
    }
}
