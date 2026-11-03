//! UAC2 function and vendor control interface.
//!
//! R2: no audio hardware yet. Playback data (EP 0x01) is looped back into
//! the capture stream (EP 0x82), so a host can check the iso path bit for
//! bit. The device clock is the host's microframe clock: every microframe
//! the capture endpoint sends exactly rate / 8000 frames and the feedback
//! endpoint reports that nominal value, so playback and capture run at the
//! same rate and the loopback FIFO level stays constant.

use crate::usb::descriptors::{
    self as desc, EP_AUDIO, EP_CAPTURE, EP_SIZE, FB_SIZE, FRAME_BYTES, ITF_AC, ITF_AS_IN, ITF_AS_OUT,
    ITF_VENDOR,
};
use crate::usb::hw::{IsoRing, QhList};
use crate::usb::{KIND_CLASS, KIND_VENDOR, RCPT_INTERFACE, Reply, Request};

/// Sample rates, ascending.
pub const RATES: &[u32] = &[48_000];

// UAC2 requests and clock source control selectors
const REQ_CUR: u8 = 0x01;
const REQ_RANGE: u8 = 0x02;
const CS_SAM_FREQ_CONTROL: u8 = 0x01;
const CS_CLOCK_VALID_CONTROL: u8 = 0x02;

// Vendor protocol (docs/vendor-protocol.md)
const VENDOR_PROTO_VERSION: (u8, u8) = (0, 2);
/// 0.3.x = Rust port. 0.3.1: R2 (loopback, no audio hardware).
const FW_VERSION: (u8, u8, u8) = (0, 3, 1);
const INFO_FLAG_STUB: u16 = 0x0001;

const VREQ_GET_INFO: u8 = 0x00;
const VREQ_GET_SCRATCH: u8 = 0x01;
const VREQ_SET_SCRATCH: u8 = 0x02;
const VREQ_GET_STATS: u8 = 0x03;
const VREQ_CLEAR_STATS: u8 = 0x04;

const SCRATCH_SIZE: usize = 32;

const STATS_FLAG_PLAY_ACTIVE: u16 = 1 << 1;
const STATS_FLAG_REC_ACTIVE: u16 = 1 << 2;
const STATS_FLAG_PLAY_PRIMED: u16 = 1 << 3;

/// dTDs per data endpoint ring.
const RING_TDS: usize = 8;
/// Capture packets queued ahead of the host (microframes of latency).
const IN_DEPTH: usize = 2;
/// Loopback FIFO, bytes (power of two).
const FIFO_SIZE: usize = 8192;
/// Loopback starts once this much playback data is buffered: 2 ms at 48 kHz.
const PRIME_BYTES: usize = 96 * FRAME_BYTES;

/// Feedback value for a rate, 16.16 samples per microframe.
pub const fn feedback_nominal(rate: u32) -> u32 {
    (rate << 16) / 8000
}

struct Fifo {
    buf: [u8; FIFO_SIZE],
    rd: usize,
    wr: usize,
}

impl Fifo {
    const fn new() -> Self {
        Fifo { buf: [0; FIFO_SIZE], rd: 0, wr: 0 }
    }
    fn clear(&mut self) {
        self.rd = 0;
        self.wr = 0;
    }
    fn len(&self) -> usize {
        self.wr.wrapping_sub(self.rd)
    }
    /// Push as much as fits; returns bytes dropped.
    fn push(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(FIFO_SIZE - self.len());
        for &b in &data[..n] {
            self.buf[self.wr % FIFO_SIZE] = b;
            self.wr = self.wr.wrapping_add(1);
        }
        data.len() - n
    }
    fn pop(&mut self, out: &mut [u8]) {
        for b in out.iter_mut() {
            *b = self.buf[self.rd % FIFO_SIZE];
            self.rd = self.rd.wrapping_add(1);
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Stats {
    underruns: u32,
    overruns: u32,
    lead_min: i32,
    lead_max: i32,
}

impl Stats {
    const fn new() -> Self {
        Stats { underruns: 0, overruns: 0, lead_min: i32::MAX, lead_max: i32::MIN }
    }
}

pub struct Audio {
    rate: u32,
    alt_out: u8,
    alt_in: u8,
    out_ring: IsoRing<RING_TDS, EP_SIZE>,
    fb_ring: IsoRing<2, FB_SIZE>,
    in_ring: IsoRing<RING_TDS, EP_SIZE>,
    fifo: Fifo,
    primed: bool,
    starved: bool,
    stats: Stats,
    scratch: [u8; SCRATCH_SIZE],
}

impl Audio {
    pub const fn new() -> Self {
        Audio {
            rate: RATES[0],
            alt_out: 0,
            alt_in: 0,
            out_ring: IsoRing::new(EP_AUDIO, false),
            fb_ring: IsoRing::new(EP_AUDIO, true),
            in_ring: IsoRing::new(EP_CAPTURE, true),
            fifo: Fifo::new(),
            primed: false,
            starved: false,
            stats: Stats::new(),
            scratch: [0; SCRATCH_SIZE],
        }
    }

    /// Bus reset or configuration change: all streams off. The controller
    /// has already disabled and flushed the endpoints on bus reset.
    pub fn reset(&mut self) {
        if self.alt_out != 0 {
            self.out_ring.stop();
            self.fb_ring.stop();
        }
        if self.alt_in != 0 {
            self.in_ring.stop();
        }
        self.alt_out = 0;
        self.alt_in = 0;
        self.fifo.clear();
        self.primed = false;
        self.starved = false;
    }

    pub fn get_interface(&self, itf: u8) -> Option<u8> {
        match itf {
            ITF_AC | ITF_VENDOR => Some(0),
            ITF_AS_OUT => Some(self.alt_out),
            ITF_AS_IN => Some(self.alt_in),
            _ => None,
        }
    }

    pub fn set_interface(&mut self, itf: u8, alt: u8, qhs: &mut QhList) -> bool {
        match (itf, alt) {
            (ITF_AC | ITF_VENDOR, 0) => true,
            (ITF_AS_OUT, 0) => {
                if self.alt_out != 0 {
                    self.out_ring.stop();
                    self.fb_ring.stop();
                }
                self.alt_out = 0;
                true
            }
            (ITF_AS_OUT, 1) => {
                self.alt_out = 1;
                self.fifo.clear();
                self.primed = false;
                self.starved = false;
                self.stats.lead_min = i32::MAX;
                self.stats.lead_max = i32::MIN;
                self.out_ring.start_out(qhs);
                self.fb_ring.start_in(qhs, FB_SIZE as u16);
                self.service(qhs);
                true
            }
            (ITF_AS_IN, 0) => {
                if self.alt_in != 0 {
                    self.in_ring.stop();
                }
                self.alt_in = 0;
                true
            }
            (ITF_AS_IN, 1) => {
                self.alt_in = 1;
                self.in_ring.start_in(qhs, EP_SIZE as u16);
                self.service(qhs);
                true
            }
            _ => false,
        }
    }

    fn frame_bytes_per_uframe(&self) -> usize {
        (self.rate / 8000) as usize * FRAME_BYTES
    }

    /// Endpoint completions (USB ISR): move playback packets into the
    /// loopback FIFO, keep feedback and capture packets queued.
    pub fn service(&mut self, qhs: &mut QhList) {
        if self.alt_out != 0 {
            let fifo = &mut self.fifo;
            let stats = &mut self.stats;
            let starved = &mut self.starved;
            let looping = self.alt_in != 0;
            self.out_ring.service_out(qhs, |data| {
                if data.is_empty() || !looping {
                    return;
                }
                if *starved {
                    stats.underruns = stats.underruns.wrapping_add(1);
                    *starved = false;
                }
                if fifo.push(data) != 0 {
                    stats.overruns = stats.overruns.wrapping_add(1);
                }
            });
            let fb = feedback_nominal(self.rate).to_le_bytes();
            self.fb_ring.service_in(qhs, 1, |buf| {
                buf[..4].copy_from_slice(&fb);
                4
            });
        }
        if self.alt_in != 0 {
            let n = self.frame_bytes_per_uframe();
            let fifo = &mut self.fifo;
            let primed = &mut self.primed;
            let starved = &mut self.starved;
            let stats = &mut self.stats;
            let playing = self.alt_out != 0;
            self.in_ring.service_in(qhs, IN_DEPTH, |buf| {
                if !*primed && playing && fifo.len() >= PRIME_BYTES {
                    *primed = true;
                }
                if *primed && fifo.len() >= n {
                    fifo.pop(&mut buf[..n]);
                    let lead = (fifo.len() / FRAME_BYTES) as i32;
                    stats.lead_min = stats.lead_min.min(lead);
                    stats.lead_max = stats.lead_max.max(lead);
                } else {
                    if *primed {
                        // Ran dry: counted as an underrun only if data resumes.
                        *primed = false;
                        *starved = true;
                        fifo.clear();
                    }
                    buf[..n].fill(0);
                }
                n
            });
        }
    }

    // --- control requests ------------------------------------------------

    /// Class and vendor SETUP. IN data goes to `buf`.
    pub fn control(&mut self, req: &Request, buf: &mut [u8], _qhs: &mut QhList) -> Reply {
        if req.recipient() != RCPT_INTERFACE {
            return Reply::Stall;
        }
        let itf = req.index as u8;
        let entity = (req.index >> 8) as u8;
        match req.kind() {
            KIND_CLASS if itf == ITF_AC && entity == desc::ENTITY_CLOCK => {
                if req.dir_in() {
                    self.clock_get(req, buf)
                } else {
                    Reply::Out
                }
            }
            KIND_VENDOR if itf == ITF_VENDOR => self.vendor(req, buf),
            _ => Reply::Stall,
        }
    }

    /// OUT data stage finished.
    pub fn control_out(&mut self, req: &Request, data: &[u8]) -> Reply {
        let itf = req.index as u8;
        match req.kind() {
            KIND_CLASS if itf == ITF_AC => {
                let selector = (req.value >> 8) as u8;
                if req.request == REQ_CUR && selector == CS_SAM_FREQ_CONTROL && data.len() == 4 {
                    let rate = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                    if RATES.contains(&rate) {
                        self.rate = rate;
                        return Reply::Ack;
                    }
                }
                Reply::Stall
            }
            KIND_VENDOR if itf == ITF_VENDOR && req.request == VREQ_SET_SCRATCH => {
                self.scratch = [0; SCRATCH_SIZE];
                let n = data.len().min(SCRATCH_SIZE);
                self.scratch[..n].copy_from_slice(&data[..n]);
                Reply::Ack
            }
            _ => Reply::Stall,
        }
    }

    fn clock_get(&self, req: &Request, buf: &mut [u8]) -> Reply {
        let selector = (req.value >> 8) as u8;
        match (selector, req.request) {
            (CS_SAM_FREQ_CONTROL, REQ_CUR) => {
                buf[..4].copy_from_slice(&self.rate.to_le_bytes());
                Reply::In(4)
            }
            (CS_SAM_FREQ_CONTROL, REQ_RANGE) => {
                // Layout 3 parameter block: wNumSubRanges, then {min, max, res}.
                let n = RATES.len().min(8);
                buf[0..2].copy_from_slice(&(n as u16).to_le_bytes());
                for (i, &r) in RATES.iter().take(n).enumerate() {
                    let o = 2 + 12 * i;
                    buf[o..o + 4].copy_from_slice(&r.to_le_bytes());
                    buf[o + 4..o + 8].copy_from_slice(&r.to_le_bytes());
                    buf[o + 8..o + 12].fill(0);
                }
                Reply::In(2 + 12 * n)
            }
            (CS_CLOCK_VALID_CONTROL, REQ_CUR) => {
                buf[0] = 1;
                Reply::In(1)
            }
            _ => Reply::Stall,
        }
    }

    fn vendor(&mut self, req: &Request, buf: &mut [u8]) -> Reply {
        match (req.request, req.dir_in()) {
            (VREQ_GET_INFO, true) => {
                let flags = INFO_FLAG_STUB.to_le_bytes();
                buf[..8].copy_from_slice(&[
                    VENDOR_PROTO_VERSION.0,
                    VENDOR_PROTO_VERSION.1,
                    FW_VERSION.0,
                    FW_VERSION.1,
                    FW_VERSION.2,
                    0,
                    flags[0],
                    flags[1],
                ]);
                Reply::In(8)
            }
            (VREQ_GET_SCRATCH, true) => {
                buf[..SCRATCH_SIZE].copy_from_slice(&self.scratch);
                Reply::In(SCRATCH_SIZE)
            }
            (VREQ_SET_SCRATCH, false) if (req.length as usize) <= SCRATCH_SIZE => {
                if req.length == 0 {
                    self.scratch = [0; SCRATCH_SIZE];
                    Reply::Ack
                } else {
                    Reply::Out
                }
            }
            (VREQ_GET_STATS, true) => {
                self.stats_bytes(&mut buf[..40]);
                Reply::In(40)
            }
            (VREQ_CLEAR_STATS, false) if req.length == 0 => {
                self.stats = Stats::new();
                Reply::Ack
            }
            _ => Reply::Stall,
        }
    }

    /// vendor_stats_t (40 bytes). R2 mapping: play_underruns and
    /// rec_overruns are loopback FIFO underruns/overruns, tx/rx_fifo_errors
    /// are iso dTD errors on the OUT and IN data endpoints, lead is the
    /// loopback FIFO level in frames.
    fn stats_bytes(&self, st: &mut [u8]) {
        let (lead_min, lead_max) = if self.stats.lead_min <= self.stats.lead_max {
            (self.stats.lead_min, self.stats.lead_max)
        } else {
            (0, 0)
        };
        let words: [u32; 9] = [
            self.rate,
            feedback_nominal(self.rate),
            self.stats.underruns,
            self.stats.overruns,
            0,
            self.out_ring.errors,
            self.in_ring.errors,
            lead_min as u32,
            lead_max as u32,
        ];
        for (i, w) in words.iter().enumerate() {
            st[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        let flags = (if self.alt_out != 0 { STATS_FLAG_PLAY_ACTIVE } else { 0 })
            | (if self.alt_in != 0 { STATS_FLAG_REC_ACTIVE } else { 0 })
            | (if self.primed { STATS_FLAG_PLAY_PRIMED } else { 0 });
        st[36..38].copy_from_slice(&flags.to_le_bytes());
        st[38..40].fill(0);
    }
}
