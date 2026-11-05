//! Playback rate feedback, measured from the audio interface. Port of
//! `firmware/app/feedback.c`; see docs/feedback.md.
//!
//! In the USB SOF interrupt the number of words the SAI transmitter has
//! shifted out is sampled together with the microframe number (FRINDEX).
//! Feedback is the difference over a sliding window of about one second:
//!
//!   fb (16.16 samples per microframe) = d_words / channels * 65536 / d_microframes
//!
//! The window slides one 2 ms step at a time, so consecutive values do not
//! share quantisation: the host's running total of requested samples follows
//! the device's consumed total within one window.
//!
//! The division's remainder is carried to the next value (error diffusion):
//! at 16 channels, d_words / 16 * 65536 / 8192 = d_words / 2, and plain
//! truncation lost half an LSB whenever d_words was odd. That bias (about
//! 0.03 Hz at 192 kHz) made the host send one frame a minute too few, and
//! the playback lead fell 120 frames in a 2 h soak. In stereo the division
//! is exact (d_words * 4) for a full window.
//!
//! Startup: the window grows from 16 ms to 1024 ms after reset; until 16 ms
//! of history exist the nominal value is sent.

const UF_MASK: u32 = 0x3FFF; // FRINDEX: 14-bit microframe counter
const STEP_UF: u32 = 16; // sample every 2 ms
const HIST: usize = 512; // 512 * 2 ms = 1024 ms window
const MIN_WIN_UF: u32 = 128; // first measured value after 16 ms
const MAX_GAP_UF: u32 = 256; // SOF gap (suspend, bus reset): restart
const CLAMP_PPM: u64 = 1000; // limit to nominal +- 0.1 %

#[derive(Clone, Copy)]
struct Snap {
    uf: u16,
    words: u32,
}

pub struct Feedback {
    hist: [Snap; HIST],
    n_hist: usize,
    head: usize,
    last_uf: u32,
    reset_req: bool,
    nominal: u32,
    value: u32,
    /// Words per frame on the SAI.
    channels: u32,
    /// Remainder carried between values, as a 0.32 fraction of an LSB.
    frac: u32,
}

/// Nominal feedback for a rate, 16.16 samples per microframe.
pub const fn nominal(rate: u32) -> u32 {
    (((rate as u64) << 16) / 8000) as u32
}

impl Feedback {
    pub const fn new(rate: u32) -> Self {
        Feedback {
            hist: [Snap { uf: 0, words: 0 }; HIST],
            n_hist: 0,
            head: 0,
            last_uf: 0,
            reset_req: true,
            nominal: nominal(rate),
            value: nominal(rate),
            channels: 2,
            frac: 0,
        }
    }

    /// Restart measurement (stream start, rate or format change).
    pub fn reset(&mut self, rate: u32, channels: usize) {
        self.channels = channels as u32;
        self.nominal = nominal(rate);
        self.value = self.nominal;
        self.frac = 0;
        self.reset_req = true;
    }

    /// Current value to send.
    pub fn value(&self) -> u32 {
        self.value
    }

    /// SOF interrupt: `frindex` is the microframe counter, `words_played`
    /// reads the SAI transmitter's word count (only called when sampling).
    pub fn sof(&mut self, frindex: u32, words_played: impl FnOnce() -> u32) {
        let uf = frindex & UF_MASK;
        if self.reset_req {
            self.reset_req = false;
            self.n_hist = 0;
            self.head = 0;
        } else {
            let gap = uf.wrapping_sub(self.last_uf) & UF_MASK;
            if gap < STEP_UF {
                return;
            }
            if gap > MAX_GAP_UF {
                self.n_hist = 0;
                self.head = 0;
            }
        }

        let words = words_played();
        self.last_uf = uf;

        if self.n_hist > 0 {
            let old = self.hist[if self.n_hist < HIST { 0 } else { self.head }];
            let d_uf = uf.wrapping_sub(old.uf as u32) & UF_MASK;
            if d_uf >= MIN_WIN_UF {
                let num = (words.wrapping_sub(old.words) as u64) << 16;
                let den = self.channels as u64 * d_uf as u64;
                let (frac, carry) = self.frac.overflowing_add(((num % den << 32) / den) as u32);
                self.frac = frac;
                let fb = num / den + carry as u64;
                let lim = self.nominal as u64 * CLAMP_PPM / 1_000_000;
                let fb = fb.clamp(self.nominal as u64 - lim, self.nominal as u64 + lim);
                self.value = fb as u32;
            }
        }

        self.hist[self.head] = Snap { uf: uf as u16, words };
        self.head = (self.head + 1) % HIST;
        if self.n_hist < HIST {
            self.n_hist += 1;
        }
    }
}
