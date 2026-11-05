//! USB ISR cycle profile (DWT cycle counter, core clock).
//!
//! Only the USB ISR touches this: it records, and the vendor request that
//! reads or clears it runs inside the same ISR. Higher-priority ISRs (eDMA
//! ring wraps) that preempt the USB ISR are counted in its time.

use core::ptr::addr_of_mut;

use cortex_m::peripheral::DWT;

use crate::clock;

/// One section of the ISR.
#[derive(Clone, Copy)]
pub enum Section {
    /// SOF: feedback, TX ring zeroing, starvation and SAI error checks.
    Sof = 0,
    /// Endpoint completions: EP0 and the iso rings.
    Xfer = 1,
    /// SETUP handling, including vendor requests (codec I2C writes).
    Setup = 2,
}

struct Profile {
    /// ISR entries.
    count: u32,
    /// Longest ISR.
    max: u32,
    /// Longest run of each section.
    section_max: [u32; 3],
    /// Cycles spent inside the ISR.
    busy: u64,
    /// Cycles since the profile was cleared, summed between ISR entries.
    elapsed: u64,
    last_entry: u32,
}

static mut PROFILE: Profile = Profile {
    count: 0,
    max: 0,
    section_max: [0; 3],
    busy: 0,
    elapsed: 0,
    last_entry: 0,
};

fn profile() -> &'static mut Profile {
    // SAFETY: only the USB ISR calls in here (module doc).
    unsafe { &mut *addr_of_mut!(PROFILE) }
}

pub fn now() -> u32 {
    DWT::cycle_count()
}

/// ISR entry. Returns the entry time.
pub fn enter() -> u32 {
    let p = profile();
    let t = now();
    // ISRs come every 125 us, far below the 8.6 s counter wrap.
    if p.count != 0 {
        p.elapsed += t.wrapping_sub(p.last_entry) as u64;
    }
    p.last_entry = t;
    t
}

pub fn section(s: Section, start: u32) {
    let p = profile();
    let d = now().wrapping_sub(start);
    let m = &mut p.section_max[s as usize];
    *m = (*m).max(d);
}

pub fn exit(start: u32) {
    let p = profile();
    let d = now().wrapping_sub(start);
    p.count = p.count.wrapping_add(1);
    p.max = p.max.max(d);
    p.busy += d as u64;
}

pub fn clear() {
    let p = profile();
    *p = Profile { count: 0, max: 0, section_max: [0; 3], busy: 0, elapsed: 0, last_entry: 0 };
}

/// Debug request DBG_ISR_STATS (docs/vendor-protocol.md), 40 bytes.
pub const BYTES: usize = 40;

pub fn bytes(out: &mut [u8]) {
    let p = profile();
    let w32 = [p.count, p.max, p.section_max[0], p.section_max[1], p.section_max[2]];
    for (i, w) in w32.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    out[20..28].copy_from_slice(&p.busy.to_le_bytes());
    out[28..36].copy_from_slice(&p.elapsed.to_le_bytes());
    out[36..40].copy_from_slice(&clock::AHB_HZ.to_le_bytes());
}
