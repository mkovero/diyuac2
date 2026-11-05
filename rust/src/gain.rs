//! UAC2 Feature Unit state: master mute and volume, applied digitally to
//! the samples.
//!
//! Volume is in 1/256 dB (UAC2 CUR/RANGE layout), from -100 dB to 0 dB in
//! 0.5 dB steps. At 0 dB and unmuted the samples pass untouched, so the
//! stream stays bit-exact; any other setting raises a stats flag
//! ([`Gain::unity`]) so measurement tools can refuse to run.

/// 0.5 dB in 1/256 dB.
pub const VOL_RES: i16 = 128;
pub const VOL_MIN: i16 = -100 * 256;
pub const VOL_MAX: i16 = 0;
const STEPS: usize = ((VOL_MAX - VOL_MIN) / VOL_RES) as usize + 1;

/// 10^(-0.5 / 20): one 0.5 dB step down.
const STEP_RATIO: f64 = 0.944_060_876_285_923_4;
const ONE_Q30: f64 = (1u32 << 30) as f64;

/// Linear gain in Q2.30 for each step below 0 dB (index 0 = 0 dB).
static TABLE: [u32; STEPS] = {
    let mut t = [0u32; STEPS];
    let mut g = 1.0f64;
    let mut i = 0;
    while i < STEPS {
        t[i] = (g * ONE_Q30 + 0.5) as u32;
        g *= STEP_RATIO;
        i += 1;
    }
    t
};

pub struct Gain {
    mute: bool,
    /// 1/256 dB, a multiple of VOL_RES within [VOL_MIN, VOL_MAX].
    volume: i16,
    /// Q2.30; 0 when muted.
    factor: u32,
}

impl Gain {
    pub const fn new() -> Self {
        Gain { mute: false, volume: 0, factor: 1 << 30 }
    }

    pub fn mute(&self) -> bool {
        self.mute
    }

    pub fn volume(&self) -> i16 {
        self.volume
    }

    /// 0 dB and unmuted: samples pass bit-exact.
    pub fn unity(&self) -> bool {
        !self.mute && self.volume == 0
    }

    pub fn set_mute(&mut self, mute: bool) {
        self.mute = mute;
        self.update();
    }

    /// Clamped to the range and rounded to the nearest step; GET_CUR
    /// returns the value in effect.
    pub fn set_volume(&mut self, v: i16) {
        let v = v.clamp(VOL_MIN, VOL_MAX) as i32;
        let steps = (-v + VOL_RES as i32 / 2) / VOL_RES as i32;
        self.volume = (-steps * VOL_RES as i32) as i16;
        self.update();
    }

    fn update(&mut self) {
        let steps = (-(self.volume as i32) / VOL_RES as i32) as usize;
        self.factor = if self.mute { 0 } else { TABLE[steps] };
    }

    /// One sample (any 32-bit container: 24-in-32 audio is left-justified,
    /// so it scales the same).
    #[inline(always)]
    pub fn apply(&self, s: u32) -> u32 {
        ((s as i32 as i64 * self.factor as i64) >> 30) as i32 as u32
    }
}
