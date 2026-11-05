//! UAC2 function and vendor control interface: the audio data path.
//!
//! The SAI frame clock (audio PLL, MCU is I2S master) is the device clock.
//! Everything runs in the USB ISR:
//!
//! - Playback: each iso OUT packet is written straight into the SAI TX DMA
//!   ring, a little ahead of the DMA. Frames the DMA has fetched are zeroed
//!   every microframe, so a starved ring plays silence.
//! - Capture: each iso IN packet is filled straight from the SAI RX DMA ring.
//! - Feedback: measured from the SAI transmitter at SOF ([`crate::feedback`]).

use crate::audio_hw::{self, RING_WORDS};
use crate::codec::{self, Codec};
use crate::feedback::Feedback;
use crate::gain::{self, Gain};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::usb::descriptors::{
    self as desc, BYTES_PER_SAMPLE, EP_AUDIO, EP_CAPTURE, EP_SIZE, EP_SIZE_16CH, EP_MULT_16CH, FB_SIZE, ITF_AC,
    ITF_AS_IN, ITF_AS_OUT, ITF_DFU, ITF_VENDOR, ISO_BUF_SIZE,
};
use crate::usb::hw::{IsoRing, QhList};
use crate::usb::{Function, KIND_CLASS, KIND_VENDOR, RCPT_INTERFACE, Reply, Request};
use diyuac2_common::profile;

// DFU 1.1 class requests (runtime interface)
const DFU_DETACH: u8 = 0x00;
const DFU_GETSTATUS: u8 = 0x03;
const DFU_GETSTATE: u8 = 0x05;
const DFU_STATE_APP_IDLE: u8 = 0;

/// Set when the host sent DFU_DETACH and its status stage finished; the
/// main loop then restarts into the bootloader.
pub static DETACH_REQUESTED: AtomicBool = AtomicBool::new(false);

// UAC2 requests and clock source control selectors
const REQ_CUR: u8 = 0x01;
const REQ_RANGE: u8 = 0x02;
const CS_SAM_FREQ_CONTROL: u8 = 0x01;
const CS_CLOCK_VALID_CONTROL: u8 = 0x02;
// Feature unit control selectors
const FU_MUTE_CONTROL: u8 = 0x01;
const FU_VOLUME_CONTROL: u8 = 0x02;

// Vendor protocol (docs/vendor-protocol.md)
const VENDOR_PROTO_VERSION: (u8, u8) = (0, 3);
/// 0.3.x = Rust port. 0.3.2: R3 (SAI + WM8960, measured feedback).
/// 0.3.3: 44.1 kHz family, Feature Units (digital volume/mute).
/// 0.4.0: DFU. 0.4.1: alt 2, 16 channels (SAI TDM, high-bandwidth iso).
const FW_VERSION: (u8, u8, u8) = (0, 4, 1);
/// Same as the C firmware: commands beyond the transport test are drafts.
const INFO_FLAG_STUB: u16 = 0x0001;
/// The link is full speed: the audio function is not offered.
const INFO_FLAG_FULL_SPEED: u16 = 0x0002;

const VREQ_GET_INFO: u8 = 0x00;
const VREQ_GET_SCRATCH: u8 = 0x01;
const VREQ_SET_SCRATCH: u8 = 0x02;
const VREQ_GET_STATS: u8 = 0x03;
const VREQ_CLEAR_STATS: u8 = 0x04;
/// vendor_stats_t: 40 bytes up to 0.4.1, then USB error counters.
const STATS_LEN: usize = 60;
const VREQ_DBG_CODEC_WRITE: u8 = 0x10;
const VREQ_DBG_ISR_STATS: u8 = 0x11;
const VREQ_DBG_LOOPBACK: u8 = 0x12;

/// Sample rates offered (clock source RANGE). The SAI/USB path runs all of
/// them; the WM8960 is only specified up to 48 kHz ([`codec::RATES`]).
pub const RATES: &[u32] = &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Rate after power-up.
pub const DEFAULT_RATE: u32 = 48_000;

const SCRATCH_SIZE: usize = 32;

const STATS_FLAG_CODEC_OK: u16 = 1 << 0;
const STATS_FLAG_PLAY_ACTIVE: u16 = 1 << 1;
const STATS_FLAG_REC_ACTIVE: u16 = 1 << 2;
const STATS_FLAG_PLAY_PRIMED: u16 = 1 << 3;
/// The rate is outside the codec's specification: analog I/O is not valid.
const STATS_FLAG_CODEC_OUT_OF_SPEC: u16 = 1 << 4;
/// A Feature Unit is muted or not at 0 dB: the path is not bit-exact.
const STATS_FLAG_GAIN: u16 = 1 << 6;
/// Debug loopback: capture carries the playback data back (DBG_LOOPBACK).
const STATS_FLAG_LOOPBACK: u16 = 1 << 5;
/// The SAI runs the 16-channel TDM format (alt 2): the codec gets no
/// usable signal.
const STATS_FLAG_TDM16: u16 = 1 << 7;

/// dTDs per data endpoint ring: OUT keeps all queued, IN keeps IN_DEPTH.
const RING_TDS: usize = 8;
const IN_TDS: usize = 4;
/// Capture packets queued ahead of the host (microframes of latency).
const IN_DEPTH: usize = 2;

/// Smallest ring, in frames (16 channels).
const MIN_RING_FRAMES: u32 = (RING_WORDS / 16) as u32;
/// Playback frames written ahead of the DMA fetch position never come closer
/// than GUARD (the DMA runs up to a FIFO watermark ahead of the SAI) and
/// never further than MAX_LEAD.
const PLAY_GUARD: u32 = 16;
const PLAY_MAX_LEAD: u32 = 768;
/// Playback starts this far ahead of the DMA: 2 ms at the stream rate.
const PLAY_PRIME_MS: u32 = 2;
/// Played frames are zeroed this far behind the DMA, so the debug loopback
/// can still read them. Silence on starvation still holds: the frames the
/// DMA reaches after the writer are a ring lap old, and with
/// lead <= PLAY_MAX_LEAD they are more than ZERO_LAG behind.
const ZERO_LAG: u32 = 256;
const _: () = assert!(PLAY_MAX_LEAD + ZERO_LAG <= MIN_RING_FRAMES);

struct Stats {
    play_underruns: u32,
    rec_overruns: u32,
    rec_drops: u32,
    tx_fifo_errors: u32,
    rx_fifo_errors: u32,
    lead_min: i32,
    lead_max: i32,
}

impl Stats {
    const fn new() -> Self {
        Stats {
            play_underruns: 0,
            rec_overruns: 0,
            rec_drops: 0,
            tx_fifo_errors: 0,
            rx_fifo_errors: 0,
            lead_min: i32::MAX,
            lead_max: i32::MIN,
        }
    }
    fn lead_reset(&mut self) {
        self.lead_min = i32::MAX;
        self.lead_max = i32::MIN;
    }
}

struct Play {
    /// Next frame position to write in the TX ring.
    wr: u32,
    /// TX ring is zeroed up to here (behind the DMA).
    zeroed: u32,
    primed: bool,
    /// The ring ran dry inside a stream; counted when data resumes.
    starved: bool,
}

/// Data path state, kept apart from the endpoint rings so the ring service
/// closures can borrow it while the rings are borrowed too.
struct Path {
    rate: u32,
    /// Channels per frame, both directions (the SAI format).
    channels: usize,
    play: Play,
    /// Next frame position to read from the RX ring.
    rec_rd: u32,
    rec_restart: bool,
    /// Capture packets filled since the restart.
    rec_fills: u32,
    /// Debug: capture reads the TX ring (what the SAI sent) instead of RX.
    loopback: bool,
    /// Feature Units 0x02 (playback) and 0x12 (capture).
    play_gain: Gain,
    rec_gain: Gain,
    stats: Stats,
}

impl Path {
    fn restart(&mut self) {
        self.play = Play { wr: 0, zeroed: audio_hw::tx_frames_fetched(), primed: false, starved: false };
        self.rec_restart = true;
    }

    fn frame_bytes(&self) -> usize {
        self.channels * BYTES_PER_SAMPLE
    }

    fn ring_mask(&self) -> u32 {
        (RING_WORDS / self.channels) as u32 - 1
    }

    /// Frames per microframe at the current rate, rounded down.
    fn frames_per_uframe(&self) -> u32 {
        self.rate / 8000
    }

    /// Most frames in one capture packet: the nominal count rounded up
    /// (44.1 kHz alternates 5 and 6), plus one to catch up.
    fn max_frames_per_uframe(&self) -> u32 {
        self.rate.div_ceil(8000) + 1
    }

    /// SOF: keep the played part of the TX ring silent, starvation check,
    /// SAI error flags.
    fn sof(&mut self) {
        let rd = audio_hw::tx_frames_fetched();
        self.zero_played(rd);
        if self.play.primed && (self.play.wr.wrapping_sub(rd) as i32) < 0 {
            // The DMA overtook the writer: silence is being played.
            self.play.primed = false;
            self.play.starved = true;
        }
        audio_hw::poll_errors(&mut self.stats.tx_fifo_errors, &mut self.stats.rx_fifo_errors);
    }

    /// Clear what the TX DMA has fetched since the last call.
    fn zero_played(&mut self, rd: u32) {
        let end = rd.wrapping_sub(ZERO_LAG);
        if (end.wrapping_sub(self.play.zeroed) as i32) <= 0 {
            return;
        }
        let ring = audio_hw::tx_ring();
        let (ch, frames) = (self.channels, RING_WORDS / self.channels);
        let mut pos = end.wrapping_sub(end.wrapping_sub(self.play.zeroed).min(frames as u32));
        while pos != end {
            let i = (pos & self.ring_mask()) as usize;
            let chunk = (end.wrapping_sub(pos) as usize).min(frames - i);
            ring[i * ch..(i + chunk) * ch].fill(0);
            pos = pos.wrapping_add(chunk as u32);
        }
        self.play.zeroed = end;
    }

    /// One playback packet into the TX ring.
    fn play_packet(&mut self, data: &[u8]) {
        let frames = (data.len() / self.frame_bytes()) as u32;
        if frames == 0 {
            return;
        }
        let rd = audio_hw::tx_frames_fetched();
        self.zero_played(rd);

        if !self.play.primed {
            // A gap inside a stream is an underrun; the stream start is not.
            if self.play.starved {
                self.stats.play_underruns = self.stats.play_underruns.wrapping_add(1);
                self.play.starved = false;
            }
            self.play.primed = true;
            self.play.wr = rd.wrapping_add(self.rate / 1000 * PLAY_PRIME_MS + PLAY_GUARD);
        }

        let lead = self.play.wr.wrapping_sub(rd) as i32;
        if lead < 0 || lead as u32 + frames > PLAY_MAX_LEAD {
            // Overtaken between SOFs, or the host sends far too much: restart.
            self.play.primed = false;
            self.play.starved = lead < 0;
            return;
        }

        let ring = audio_hw::tx_ring();
        let mut src = data.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let unity = self.play_gain.unity();
        let (ch, mask) = (self.channels, self.ring_mask());
        for _ in 0..frames {
            let i = (self.play.wr & mask) as usize * ch;
            for c in 0..ch {
                let s = src.next().unwrap_or(0);
                ring[i + c] = if unity { s } else { self.play_gain.apply(s) };
            }
            self.play.wr = self.play.wr.wrapping_add(1);
        }

        let lead = lead + frames as i32;
        self.stats.lead_min = self.stats.lead_min.min(lead);
        self.stats.lead_max = self.stats.lead_max.max(lead);
    }

    /// Fill one capture packet from the RX ring; returns its length.
    fn rec_packet(&mut self, buf: &mut [u8]) -> usize {
        // Loopback: TX frames the DMA has fetched (they are zeroed only
        // ZERO_LAG later), paced by the same SAI frame clock as RX.
        let wr = if self.loopback { audio_hw::tx_frames_fetched() } else { audio_hw::rx_frames_written() };
        let nominal = self.frames_per_uframe();
        if self.rec_restart {
            self.rec_restart = false;
            self.rec_fills = 0;
            self.rec_rd = wr.wrapping_sub(nominal);
        }
        self.rec_fills = self.rec_fills.saturating_add(1);
        let mut n = wr.wrapping_sub(self.rec_rd);
        // The first IN_DEPTH packets are filled when the alt setting is
        // selected; the host may start polling later than the ring lasts
        // (5 ms at 16 channels, 192 kHz). Resync without counting.
        if n > self.ring_mask() + 1 - PLAY_GUARD && self.rec_fills <= IN_DEPTH as u32 + 1 {
            self.rec_rd = wr.wrapping_sub(nominal);
            n = nominal;
        }
        if n > self.ring_mask() + 1 - PLAY_GUARD {
            self.stats.rec_overruns = self.stats.rec_overruns.wrapping_add(1);
            self.rec_rd = wr.wrapping_sub(nominal);
            n = nominal;
        }
        // Normally `nominal` or one more; never more than fits a packet.
        let take = n.min(self.max_frames_per_uframe()).min((buf.len() / self.frame_bytes()) as u32);
        let ring: &[u32] = if self.loopback { audio_hw::tx_ring() } else { audio_hw::rx_ring() };
        let unity = self.rec_gain.unity();
        let (ch, mask) = (self.channels, self.ring_mask());
        for f in 0..take as usize {
            let i = (self.rec_rd & mask) as usize * ch;
            for c in 0..ch {
                let o = (f * ch + c) * 4;
                let s = ring[i + c];
                let s = if unity { s } else { self.rec_gain.apply(s) };
                buf[o..o + 4].copy_from_slice(&s.to_le_bytes());
            }
            self.rec_rd = self.rec_rd.wrapping_add(1);
        }
        take as usize * self.frame_bytes()
    }
}

/// Microframes in which an iso data endpoint (bInterval 1) saw no
/// transaction inside a stream: the host did not send an OUT packet or did
/// not poll IN. Per stream, `microframes elapsed - dTDs retired` stays in a
/// small jitter band (a completion may be serviced after the SOF). It is
/// evaluated only at SOFs where new dTDs have completed, so the host's
/// silence before the stream stops does not count; each step above the
/// highest value seen so far is a missed microframe. Counting starts at the
/// first completion and settles for SETTLE completions.
struct MissCounter {
    running: bool,
    last_frindex: u32,
    elapsed: u32,
    done0: u32,
    last_n: u32,
    dmax: i32,
    missed: u32,
}

impl MissCounter {
    const SETTLE: u32 = 64;

    const fn new() -> Self {
        MissCounter { running: false, last_frindex: 0, elapsed: 0, done0: 0, last_n: 0, dmax: i32::MIN, missed: 0 }
    }

    /// Stream start: `done` is the ring's retired count now.
    fn start(&mut self, done: u32) {
        self.running = false;
        self.done0 = done;
        self.last_n = 0;
        self.dmax = i32::MIN;
    }

    fn sof(&mut self, frindex: u32, done: u32) {
        let n = done.wrapping_sub(self.done0);
        if !self.running {
            if n == 0 {
                return;
            }
            self.running = true;
            self.last_frindex = frindex;
            self.elapsed = n;
        }
        // FRINDEX counts microframes in 14 bits; robust to a missed SOF IRQ.
        self.elapsed = self.elapsed.wrapping_add(frindex.wrapping_sub(self.last_frindex) & 0x3FFF);
        self.last_frindex = frindex;
        if n == self.last_n {
            return;
        }
        self.last_n = n;
        let d = self.elapsed.wrapping_sub(n) as i32;
        if n < Self::SETTLE || self.dmax == i32::MIN {
            self.dmax = self.dmax.max(d);
            return;
        }
        if d > self.dmax {
            self.missed = self.missed.wrapping_add((d - self.dmax) as u32);
            self.dmax = d;
        }
    }
}

pub struct Audio {
    alt_out: u8,
    alt_in: u8,
    // The rings hold the dTDs and buffers the controller works on: they
    // live in the USB static and must never be moved.
    out_ring: IsoRing<RING_TDS, ISO_BUF_SIZE>,
    fb_ring: IsoRing<2, FB_SIZE>,
    in_ring: IsoRing<IN_TDS, ISO_BUF_SIZE>,
    path: Path,
    out_miss: MissCounter,
    in_miss: MissCounter,
    feedback: Feedback,
    codec: Option<Codec>,
    codec_ok: bool,
    full_speed: bool,
    scratch: [u8; SCRATCH_SIZE],
}

impl Audio {
    pub const fn new() -> Self {
        Audio {
            alt_out: 0,
            alt_in: 0,
            out_ring: IsoRing::new(EP_AUDIO, false),
            fb_ring: IsoRing::new(EP_AUDIO, true),
            in_ring: IsoRing::new(EP_CAPTURE, true),
            path: Path {
                rate: DEFAULT_RATE,
                channels: 2,
                play: Play { wr: 0, zeroed: 0, primed: false, starved: false },
                rec_rd: 0,
                rec_restart: true,
                rec_fills: 0,
                loopback: false,
                play_gain: Gain::new(),
                rec_gain: Gain::new(),
                stats: Stats::new(),
            },
            out_miss: MissCounter::new(),
            in_miss: MissCounter::new(),
            feedback: Feedback::new(DEFAULT_RATE),
            codec: None,
            codec_ok: false,
            full_speed: false,
            scratch: [0; SCRATCH_SIZE],
        }
    }

    /// Hand over the codec. The audio interface already runs at `rate`.
    pub fn attach(&mut self, codec: Codec, codec_ok: bool, rate: u32) {
        self.codec = Some(codec);
        self.codec_ok = codec_ok;
        self.path.rate = rate;
        self.path.restart();
    }


    fn set_rate(&mut self, rate: u32) -> bool {
        if rate == self.path.rate {
            return true;
        }
        if !audio_hw::start(rate, self.path.channels) {
            return false;
        }
        if self.codec_ok
            && let Some(codec) = self.codec.as_mut()
        {
            codec.set_rate(rate);
        }
        self.path.rate = rate;
        self.path.restart();
        self.feedback.reset(rate, self.path.channels);
        true
    }

    /// Bus reset or configuration change: all streams off.
    fn streams_off(&mut self) {
        if self.alt_out != 0 {
            self.out_ring.stop();
            self.fb_ring.stop();
        }
        if self.alt_in != 0 {
            self.in_ring.stop();
        }
        self.alt_out = 0;
        self.alt_in = 0;
        self.path.play.primed = false;
        self.path.play.starved = false;
    }

    /// Switch the SAI format for streaming alt setting `alt` (1: 2
    /// channels, 2: 16). Both directions share the format: refused while
    /// the other direction (`other_alt`) streams in a different one.
    fn set_format(&mut self, alt: u8, other_alt: u8) -> bool {
        let channels = alt_channels(alt);
        if channels == self.path.channels {
            return true;
        }
        if other_alt != 0 && alt_channels(other_alt) != channels {
            return false;
        }
        if !audio_hw::start(self.path.rate, channels) {
            return false;
        }
        self.path.channels = channels;
        self.path.restart();
        self.feedback.reset(self.path.rate, channels);
        true
    }

    fn alt(&self, itf: u8) -> Option<u8> {
        match itf {
            ITF_AC | ITF_VENDOR | ITF_DFU => Some(0),
            ITF_AS_OUT => Some(self.alt_out),
            ITF_AS_IN => Some(self.alt_in),
            _ => None,
        }
    }

    fn set_alt(&mut self, itf: u8, alt: u8, qhs: &mut QhList) -> bool {
        match (itf, alt) {
            (ITF_AC | ITF_VENDOR | ITF_DFU, 0) => true,
            (ITF_AS_OUT, 0) => {
                if self.alt_out != 0 {
                    self.out_ring.stop();
                    self.fb_ring.stop();
                }
                self.alt_out = 0;
                self.path.play.primed = false;
                self.path.play.starved = false;
                true
            }
            (ITF_AS_OUT, 1 | 2) => {
                if !self.set_format(alt, self.alt_in) {
                    return false;
                }
                self.alt_out = alt;
                self.path.play.primed = false;
                self.path.play.starved = false;
                self.path.stats.lead_reset();
                self.feedback.reset(self.path.rate, self.path.channels);
                let (max_packet, mult) = alt_packets(alt);
                self.out_ring.start_out(qhs, max_packet, mult);
                self.out_miss.start(self.out_ring.completed);
                self.fb_ring.start_in(qhs, FB_SIZE as u16, 1);
                self.pump(qhs);
                true
            }
            (ITF_AS_IN, 0) => {
                if self.alt_in != 0 {
                    self.in_ring.stop();
                }
                self.alt_in = 0;
                true
            }
            (ITF_AS_IN, 1 | 2) => {
                if !self.set_format(alt, self.alt_out) {
                    return false;
                }
                self.alt_in = alt;
                self.path.rec_restart = true;
                let (max_packet, mult) = alt_packets(alt);
                self.in_ring.start_in(qhs, max_packet, mult);
                self.in_miss.start(self.in_ring.completed);
                self.pump(qhs);
                true
            }
            _ => false,
        }
    }

    /// SOF (every microframe): feedback measurement, keep the played part
    /// of the TX ring silent, starvation check, SAI error flags.
    fn on_sof(&mut self, frindex: u32) {
        self.path.sof();
        if self.alt_out != 0 {
            self.feedback.sof(frindex, audio_hw::tx_words_played);
            self.out_miss.sof(frindex, self.out_ring.completed);
        }
        if self.alt_in != 0 {
            self.in_miss.sof(frindex, self.in_ring.completed);
        }
    }

    /// Endpoint completions (USB ISR).
    fn pump(&mut self, qhs: &mut QhList) {
        let path = &mut self.path;
        if self.alt_out != 0 {
            self.out_ring.service_out(qhs, |data| path.play_packet(data));
            let fb = self.feedback.value().to_le_bytes();
            self.fb_ring.service_in(qhs, 1, |buf| {
                buf[..4].copy_from_slice(&fb);
                4
            });
        }
        if self.alt_in != 0 {
            self.in_ring.service_in(qhs, IN_DEPTH, |buf| path.rec_packet(buf));
        }
    }

    // --- control requests ------------------------------------------------

    /// Class and vendor SETUP. IN data goes to `buf`.
    fn setup(&mut self, req: &Request, buf: &mut [u8]) -> Reply {
        if req.recipient() != RCPT_INTERFACE {
            return Reply::Stall;
        }
        let itf = req.index as u8;
        let entity = (req.index >> 8) as u8;
        match req.kind() {
            KIND_CLASS if itf == ITF_AC && entity == desc::ENTITY_CLOCK && !self.full_speed => {
                if req.dir_in() {
                    self.clock_get(req, buf)
                } else {
                    Reply::Out
                }
            }
            KIND_CLASS if itf == ITF_AC && !self.full_speed && self.fu(entity).is_some() => {
                if req.dir_in() {
                    self.fu_get(entity, req, buf)
                } else {
                    Reply::Out
                }
            }
            KIND_VENDOR if itf == ITF_VENDOR => self.vendor(req, buf),
            KIND_CLASS if itf == ITF_DFU => dfu_runtime(req, buf),
            _ => Reply::Stall,
        }
    }

    /// OUT data stage finished.
    fn setup_out(&mut self, req: &Request, data: &[u8]) -> Reply {
        let itf = req.index as u8;
        let entity = (req.index >> 8) as u8;
        match req.kind() {
            KIND_CLASS if itf == ITF_AC && entity != desc::ENTITY_CLOCK => self.fu_set(entity, req, data),
            KIND_CLASS if itf == ITF_AC => {
                let selector = (req.value >> 8) as u8;
                if req.request == REQ_CUR && selector == CS_SAM_FREQ_CONTROL && data.len() == 4 {
                    let rate = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                    if RATES.contains(&rate) && self.set_rate(rate) {
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

    fn fu(&self, entity: u8) -> Option<&Gain> {
        match entity {
            desc::ENTITY_FU_PLAY => Some(&self.path.play_gain),
            desc::ENTITY_FU_REC => Some(&self.path.rec_gain),
            _ => None,
        }
    }

    /// Feature Unit GET. Master channel (CN 0) only.
    fn fu_get(&self, entity: u8, req: &Request, buf: &mut [u8]) -> Reply {
        let (selector, channel) = ((req.value >> 8) as u8, req.value as u8);
        let Some(g) = self.fu(entity) else {
            return Reply::Stall;
        };
        if channel != 0 {
            return Reply::Stall;
        }
        match (selector, req.request) {
            (FU_MUTE_CONTROL, REQ_CUR) => {
                buf[0] = g.mute() as u8;
                Reply::In(1)
            }
            (FU_VOLUME_CONTROL, REQ_CUR) => {
                buf[..2].copy_from_slice(&g.volume().to_le_bytes());
                Reply::In(2)
            }
            (FU_VOLUME_CONTROL, REQ_RANGE) => {
                // One subrange {min, max, res}, 1/256 dB.
                buf[0..2].copy_from_slice(&1u16.to_le_bytes());
                buf[2..4].copy_from_slice(&gain::VOL_MIN.to_le_bytes());
                buf[4..6].copy_from_slice(&gain::VOL_MAX.to_le_bytes());
                buf[6..8].copy_from_slice(&gain::VOL_RES.to_le_bytes());
                Reply::In(8)
            }
            _ => Reply::Stall,
        }
    }

    /// Feature Unit SET_CUR (data stage done).
    fn fu_set(&mut self, entity: u8, req: &Request, data: &[u8]) -> Reply {
        let (selector, channel) = ((req.value >> 8) as u8, req.value as u8);
        let g = match entity {
            desc::ENTITY_FU_PLAY => &mut self.path.play_gain,
            desc::ENTITY_FU_REC => &mut self.path.rec_gain,
            _ => return Reply::Stall,
        };
        if channel != 0 || req.request != REQ_CUR {
            return Reply::Stall;
        }
        match (selector, data.len()) {
            (FU_MUTE_CONTROL, 1) => g.set_mute(data[0] != 0),
            (FU_VOLUME_CONTROL, 2) => g.set_volume(i16::from_le_bytes([data[0], data[1]])),
            _ => return Reply::Stall,
        }
        Reply::Ack
    }

    fn clock_get(&self, req: &Request, buf: &mut [u8]) -> Reply {
        let selector = (req.value >> 8) as u8;
        match (selector, req.request) {
            (CS_SAM_FREQ_CONTROL, REQ_CUR) => {
                buf[..4].copy_from_slice(&self.path.rate.to_le_bytes());
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
                let flags = (INFO_FLAG_STUB
                    | if self.full_speed { INFO_FLAG_FULL_SPEED } else { 0 })
                .to_le_bytes();
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
                self.stats_bytes(&mut buf[..STATS_LEN]);
                Reply::In(STATS_LEN)
            }
            (VREQ_CLEAR_STATS, false) if req.length == 0 => {
                self.path.stats = Stats::new();
                self.out_ring.errors = 0;
                self.in_ring.errors = 0;
                self.fb_ring.errors = 0;
                self.out_miss.missed = 0;
                self.in_miss.missed = 0;
                profile::clear();
                Reply::Ack
            }
            // Debug. Blocks the USB ISR for the I2C transfer (~0.3 ms).
            (VREQ_DBG_CODEC_WRITE, false) if req.length == 0 => {
                let ok = self
                    .codec
                    .as_mut()
                    .is_some_and(|c| c.write_reg(req.value >> 9, req.value & 0x1FF));
                if ok { Reply::Ack } else { Reply::Stall }
            }
            (VREQ_DBG_LOOPBACK, false) if req.length == 0 && req.value <= 1 => {
                self.path.loopback = req.value == 1;
                self.path.rec_restart = true;
                Reply::Ack
            }
            (VREQ_DBG_ISR_STATS, true) => {
                profile::bytes(&mut buf[..profile::BYTES]);
                Reply::In(profile::BYTES)
            }
            _ => Reply::Stall,
        }
    }

    /// vendor_stats_t (STATS_LEN bytes, docs/vendor-protocol.md).
    fn stats_bytes(&self, st: &mut [u8]) {
        let s = &self.path.stats;
        let (lead_min, lead_max) =
            if s.lead_min <= s.lead_max { (s.lead_min, s.lead_max) } else { (0, 0) };
        let words: [u32; 9] = [
            self.path.rate,
            self.feedback.value(),
            s.play_underruns,
            s.rec_overruns,
            s.rec_drops,
            s.tx_fifo_errors,
            s.rx_fifo_errors,
            lead_min as u32,
            lead_max as u32,
        ];
        for (i, w) in words.iter().enumerate() {
            st[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        let flags = (if self.codec_ok { STATS_FLAG_CODEC_OK } else { 0 })
            | (if self.alt_out != 0 { STATS_FLAG_PLAY_ACTIVE } else { 0 })
            | (if self.alt_in != 0 { STATS_FLAG_REC_ACTIVE } else { 0 })
            | (if self.path.play.primed { STATS_FLAG_PLAY_PRIMED } else { 0 })
            | (if codec::RATES.contains(&self.path.rate) { 0 } else { STATS_FLAG_CODEC_OUT_OF_SPEC })
            | (if self.path.loopback { STATS_FLAG_LOOPBACK } else { 0 })
            | (if self.path.channels == 16 { STATS_FLAG_TDM16 } else { 0 })
            | (if self.path.play_gain.unity() && self.path.rec_gain.unity() { 0 } else { STATS_FLAG_GAIN });
        st[36..38].copy_from_slice(&flags.to_le_bytes());
        st[38..40].fill(0);
        let usb = [
            self.out_ring.errors,
            self.in_ring.errors,
            self.fb_ring.errors,
            self.out_miss.missed,
            self.in_miss.missed,
        ];
        for (i, w) in usb.iter().enumerate() {
            st[40 + 4 * i..44 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
    }
}

/// DFU runtime requests (interface [`ITF_DFU`]). DETACH takes effect once
/// its status stage is done ([`Function::control_done`]).
fn dfu_runtime(req: &Request, buf: &mut [u8]) -> Reply {
    match (req.request, req.dir_in(), req.length) {
        (DFU_DETACH, false, 0) => Reply::Ack,
        (DFU_GETSTATUS, true, 6..) => {
            // bStatus OK, bwPollTimeout 0, bState appIDLE, iString 0
            buf[..6].copy_from_slice(&[0, 0, 0, 0, DFU_STATE_APP_IDLE, 0]);
            Reply::In(6)
        }
        (DFU_GETSTATE, true, 1..) => {
            buf[0] = DFU_STATE_APP_IDLE;
            Reply::In(1)
        }
        _ => Reply::Stall,
    }
}

/// At full speed only the vendor and DFU interfaces exist
/// ([`desc::FS_CONFIG`]).
fn fs_interface(itf: u8) -> bool {
    itf == ITF_VENDOR || itf == ITF_DFU
}

impl Function for Audio {
    fn control(&mut self, req: &Request, buf: &mut [u8], _qhs: &mut QhList) -> Reply {
        self.setup(req, buf)
    }

    fn control_out(&mut self, req: &Request, data: &[u8]) -> Reply {
        self.setup_out(req, data)
    }

    fn control_done(&mut self, req: &Request) {
        if req.kind() == KIND_CLASS
            && req.recipient() == RCPT_INTERFACE
            && req.index as u8 == ITF_DFU
            && req.request == DFU_DETACH
        {
            DETACH_REQUESTED.store(true, Ordering::Release);
        }
    }

    fn reset(&mut self) {
        self.streams_off();
    }

    fn get_interface(&self, itf: u8) -> Option<u8> {
        if self.full_speed && !fs_interface(itf) {
            return None;
        }
        self.alt(itf)
    }

    fn set_interface(&mut self, itf: u8, alt: u8, qhs: &mut QhList) -> bool {
        if self.full_speed && !fs_interface(itf) {
            return false;
        }
        self.set_alt(itf, alt, qhs)
    }

    /// Link speed known. At full speed the audio interfaces don't exist.
    fn set_full_speed(&mut self, full_speed: bool) {
        self.full_speed = full_speed;
    }

    fn sof(&mut self, frindex: u32) {
        self.on_sof(frindex);
    }

    fn service(&mut self, qhs: &mut QhList) {
        self.pump(qhs);
    }
}

/// Channels of a streaming alt setting (both directions): alt 1 is the
/// 2-channel codec format, alt 2 the 16-channel TDM debug format.
fn alt_channels(alt: u8) -> usize {
    if alt == 2 { 16 } else { 2 }
}

/// Iso max packet and transactions per microframe of a streaming alt.
fn alt_packets(alt: u8) -> (u16, u32) {
    if alt == 2 { (EP_SIZE_16CH as u16, EP_MULT_16CH as u32) } else { (EP_SIZE as u16, 1) }
}
