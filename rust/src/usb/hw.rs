//! ChipIdea HS device controller (i.MX RT USB_OTG1): queue heads, transfer
//! descriptors, priming, and isochronous dTD rings.
//!
//! References: i.MX RT1020 reference manual, USB chapter ("Device Data
//! Structures", "Managing Queue Heads", "Executing a Transfer Descriptor").
//!
//! All structures live in DTCM (imxrt-rt default for .bss), which the USB
//! DMA can reach and which is not cached, so no cache maintenance is needed.

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use imxrt_ral as ral;

// --- register bits (USB_OTG1, same layout as RT1011 in imxrt-ral) ----------

pub const USBCMD_RS: u32 = 1 << 0;
pub const USBCMD_RST: u32 = 1 << 1;
pub const USBCMD_SUTW: u32 = 1 << 13;
pub const USBCMD_ATDTW: u32 = 1 << 14;
pub const USBCMD_ITC_MASK: u32 = 0xFF << 16;

pub const USBSTS_UI: u32 = 1 << 0;
pub const USBSTS_UEI: u32 = 1 << 1;
pub const USBSTS_PCI: u32 = 1 << 2;
pub const USBSTS_SEI: u32 = 1 << 4;
pub const USBSTS_URI: u32 = 1 << 6;
pub const USBSTS_SRI: u32 = 1 << 7;
pub const USBSTS_SLI: u32 = 1 << 8;

pub const DEVICEADDR_USBADRA: u32 = 1 << 24;
pub const DEVICEADDR_USBADR_SHIFT: u32 = 25;

pub const PORTSC1_PSPD_SHIFT: u32 = 26;
pub const PORTSC1_PSPD_MASK: u32 = 0b11 << PORTSC1_PSPD_SHIFT;
pub const PSPD_HIGH: u32 = 2;

pub const USBMODE_CM_DEVICE: u32 = 0b10;
pub const USBMODE_SLOM: u32 = 1 << 3;

pub const EPCTRL_RXS: u32 = 1 << 0;
pub const EPCTRL_RXT_SHIFT: u32 = 2;
pub const EPCTRL_RXR: u32 = 1 << 6;
pub const EPCTRL_RXE: u32 = 1 << 7;
pub const EPCTRL_TXS: u32 = 1 << 16;
pub const EPCTRL_TXT_SHIFT: u32 = 18;
pub const EPCTRL_TXR: u32 = 1 << 22;
pub const EPCTRL_TXE: u32 = 1 << 23;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EpType {
    Control = 0,
    Isochronous = 1,
    Bulk = 2,
}

// --- dQH / dTD bits -------------------------------------------------------

const QH_CAP_IOS: u32 = 1 << 15;
const QH_CAP_MAXPKT_SHIFT: u32 = 16;
const QH_CAP_ZLT_DISABLE: u32 = 1 << 29;
const QH_CAP_MULT_SHIFT: u32 = 30;

const TD_TERMINATE: u32 = 1;
const TD_TOTAL_SHIFT: u32 = 16;
const TD_TOTAL_MASK: u32 = 0x7FFF;
pub const TD_IOC: u32 = 1 << 15;
pub const TD_ACTIVE: u32 = 1 << 7;
pub const TD_HALTED: u32 = 1 << 6;
pub const TD_DATA_BUFFER_ERROR: u32 = 1 << 5;
pub const TD_TRANSACTION_ERROR: u32 = 1 << 3;
const TD_ERRORS: u32 = TD_HALTED | TD_DATA_BUFFER_ERROR | TD_TRANSACTION_ERROR;

/// Number of endpoints the controller supports (DCCPARAMS.DEN = 8).
pub const NUM_EP: usize = 8;

pub fn regs() -> ral::usb::USB {
    // SAFETY: only the USB driver (USB ISR and init) touches these registers.
    unsafe { ral::usb::USB::instance() }
}

/// ENDPTPRIME/STAT/COMPLETE/FLUSH bit for an endpoint.
pub const fn ep_bit(num: usize, dir_in: bool) -> u32 {
    if dir_in { 1 << (16 + num) } else { 1 << num }
}

/// Endpoint queue head (dQH), 64 bytes.
#[repr(C, align(64))]
pub struct Qh {
    cap: u32,
    current: u32,
    next: u32,
    token: u32,
    buffer: [u32; 5],
    _reserved: u32,
    setup: [u32; 2],
    _pad: [u32; 4],
}

impl Qh {
    pub const fn new() -> Self {
        Qh {
            cap: 0,
            current: 0,
            next: TD_TERMINATE,
            token: 0,
            buffer: [0; 5],
            _reserved: 0,
            setup: [0; 2],
            _pad: [0; 4],
        }
    }

    /// Set up the queue head. `mult` is the number of transactions per
    /// microframe for isochronous endpoints (1..3), 0 otherwise.
    pub fn init(&mut self, max_packet: u16, mult: u32, ios: bool) {
        let cap = ((max_packet as u32) << QH_CAP_MAXPKT_SHIFT)
            | (mult << QH_CAP_MULT_SHIFT)
            | QH_CAP_ZLT_DISABLE
            | if ios { QH_CAP_IOS } else { 0 };
        unsafe {
            write_volatile(addr_of_mut!(self.cap), cap);
            write_volatile(addr_of_mut!(self.current), 0);
            write_volatile(addr_of_mut!(self.next), TD_TERMINATE);
            write_volatile(addr_of_mut!(self.token), 0);
        }
    }

    pub fn setup_packet(&self) -> [u8; 8] {
        let a = unsafe { read_volatile(addr_of!(self.setup[0])) };
        let b = unsafe { read_volatile(addr_of!(self.setup[1])) };
        let mut out = [0; 8];
        out[..4].copy_from_slice(&a.to_le_bytes());
        out[4..].copy_from_slice(&b.to_le_bytes());
        out
    }
}

/// All queue heads: index 2 * ep + dir (0 = OUT, 1 = IN). 2 KiB aligned for
/// ENDPTLISTADDR.
#[repr(C, align(2048))]
pub struct QhList(pub [Qh; 2 * NUM_EP]);

impl QhList {
    pub const fn new() -> Self {
        QhList([const { Qh::new() }; 2 * NUM_EP])
    }

    pub fn qh(&mut self, num: usize, dir_in: bool) -> &mut Qh {
        &mut self.0[2 * num + dir_in as usize]
    }
}

/// Transfer descriptor (dTD), 32 bytes.
#[repr(C, align(32))]
pub struct Td {
    next: u32,
    token: u32,
    buffer: [u32; 5],
    _reserved: u32,
}

impl Td {
    pub const fn new() -> Self {
        Td { next: TD_TERMINATE, token: 0, buffer: [0; 5], _reserved: 0 }
    }

    /// Arm for a transfer of `len` bytes at `buf` (DMA address), with
    /// interrupt on completion. The dTD is the end of its list.
    pub fn arm(&mut self, buf: *const u8, len: usize) {
        let addr = buf as u32;
        unsafe {
            write_volatile(addr_of_mut!(self.next), TD_TERMINATE);
            write_volatile(addr_of_mut!(self.buffer[0]), addr);
            for i in 1..5 {
                write_volatile(addr_of_mut!(self.buffer[i]), (addr & !0xFFF) + 0x1000 * i as u32);
            }
            write_volatile(
                addr_of_mut!(self.token),
                ((len as u32 & TD_TOTAL_MASK) << TD_TOTAL_SHIFT) | TD_IOC | TD_ACTIVE,
            );
        }
    }

    fn token(&self) -> u32 {
        unsafe { read_volatile(addr_of!(self.token)) }
    }

    pub fn active(&self) -> bool {
        self.token() & TD_ACTIVE != 0
    }

    pub fn errors(&self) -> u32 {
        self.token() & TD_ERRORS
    }

    /// Bytes not transferred.
    pub fn remaining(&self) -> usize {
        ((self.token() >> TD_TOTAL_SHIFT) & TD_TOTAL_MASK) as usize
    }

    /// Chain `next` behind this dTD.
    pub fn link(&mut self, next: *const Td) {
        unsafe { write_volatile(addr_of_mut!(self.next), next as u32) };
    }
}

/// Prime an idle endpoint with a dTD list starting at `td`.
pub fn prime(qh: &mut Qh, td: *const Td, bit: u32) {
    unsafe {
        write_volatile(addr_of_mut!(qh.next), td as u32);
        let token = read_volatile(addr_of!(qh.token));
        write_volatile(addr_of_mut!(qh.token), token & !(TD_ACTIVE | TD_HALTED));
    }
    fence(Ordering::SeqCst);
    let usb = regs();
    ral::write_reg!(ral::usb, usb, ENDPTPRIME, bit);
}

/// Add `td` (already armed, terminated) behind `last` on a possibly running
/// endpoint, re-priming if the controller already went idle. Reference
/// manual "Executing a Transfer Descriptor", case 2 (list not empty).
pub fn append(qh: &mut Qh, last: &mut Td, td: *const Td, bit: u32) {
    last.link(td);
    fence(Ordering::SeqCst);
    let usb = regs();
    if ral::read_reg!(ral::usb, usb, ENDPTPRIME) & bit != 0 {
        return;
    }
    let status = loop {
        ral::modify_reg!(ral::usb, usb, USBCMD, |v| v | USBCMD_ATDTW);
        let status = ral::read_reg!(ral::usb, usb, ENDPTSTAT) & bit;
        if ral::read_reg!(ral::usb, usb, USBCMD) & USBCMD_ATDTW != 0 {
            break status;
        }
    };
    ral::modify_reg!(ral::usb, usb, USBCMD, |v| v & !USBCMD_ATDTW);
    if status == 0 {
        prime(qh, td, bit);
    }
}

/// Flush endpoints (ENDPTFLUSH bits) and wait until done.
pub fn flush(bits: u32) {
    let usb = regs();
    loop {
        ral::write_reg!(ral::usb, usb, ENDPTFLUSH, bits);
        while ral::read_reg!(ral::usb, usb, ENDPTFLUSH) & bits != 0 {}
        if ral::read_reg!(ral::usb, usb, ENDPTSTAT) & bits == 0 {
            break;
        }
    }
}

fn endptctrl(num: usize) -> u32 {
    let usb = regs();
    if num == 0 {
        ral::read_reg!(ral::usb, usb, ENDPTCTRL0)
    } else {
        ral::read_reg!(ral::usb, usb, ENDPTCTRL[num - 1])
    }
}

fn set_endptctrl(num: usize, v: u32) {
    let usb = regs();
    if num == 0 {
        ral::write_reg!(ral::usb, usb, ENDPTCTRL0, v)
    } else {
        ral::write_reg!(ral::usb, usb, ENDPTCTRL[num - 1], v)
    }
}

/// Enable one direction of endpoint `num` (1..7) with data toggle reset. The
/// unused direction of an endpoint must not be left as type control.
pub fn ep_enable(num: usize, dir_in: bool, ty: EpType) {
    let mut v = endptctrl(num);
    if dir_in {
        v &= !(0b11 << EPCTRL_TXT_SHIFT);
        v |= ((ty as u32) << EPCTRL_TXT_SHIFT) | EPCTRL_TXR | EPCTRL_TXE;
        if v & EPCTRL_RXE == 0 {
            v = (v & !(0b11 << EPCTRL_RXT_SHIFT)) | ((EpType::Bulk as u32) << EPCTRL_RXT_SHIFT);
        }
    } else {
        v &= !(0b11 << EPCTRL_RXT_SHIFT);
        v |= ((ty as u32) << EPCTRL_RXT_SHIFT) | EPCTRL_RXR | EPCTRL_RXE;
        if v & EPCTRL_TXE == 0 {
            v = (v & !(0b11 << EPCTRL_TXT_SHIFT)) | ((EpType::Bulk as u32) << EPCTRL_TXT_SHIFT);
        }
    }
    set_endptctrl(num, v);
}

/// Disable one direction of endpoint `num` (1..7) and flush it.
pub fn ep_disable(num: usize, dir_in: bool) {
    let v = endptctrl(num) & !if dir_in { EPCTRL_TXE } else { EPCTRL_RXE };
    set_endptctrl(num, v);
    flush(ep_bit(num, dir_in));
}

/// Stall both directions of EP0 (cleared by hardware on the next SETUP).
pub fn ep0_stall() {
    set_endptctrl(0, endptctrl(0) | EPCTRL_TXS | EPCTRL_RXS);
}

/// Ring of `N` dTDs with one buffer each, for an isochronous endpoint. Each
/// dTD carries one microframe's packet.
pub struct IsoRing<const N: usize, const SIZE: usize> {
    tds: [Td; N],
    bufs: [Buf<SIZE>; N],
    /// Oldest in-flight dTD.
    head: usize,
    /// dTDs in flight.
    count: usize,
    num: usize,
    dir_in: bool,
    /// dTDs that completed with an error status.
    pub errors: u32,
}

#[repr(C, align(32))]
struct Buf<const SIZE: usize>([u8; SIZE]);

impl<const N: usize, const SIZE: usize> IsoRing<N, SIZE> {
    pub const fn new(num: usize, dir_in: bool) -> Self {
        IsoRing {
            tds: [const { Td::new() }; N],
            bufs: [const { Buf([0; SIZE]) }; N],
            head: 0,
            count: 0,
            num,
            dir_in,
            errors: 0,
        }
    }

    fn bit(&self) -> u32 {
        ep_bit(self.num, self.dir_in)
    }

    /// Queue dTD `idx` (armed for `len` bytes) at the end of the list.
    fn push(&mut self, qhs: &mut QhList, idx: usize, len: usize) {
        let bit = self.bit();
        let buf = self.bufs[idx].0.as_ptr();
        self.tds[idx].arm(buf, len);
        let td: *const Td = &self.tds[idx];
        let qh = qhs.qh(self.num, self.dir_in);
        if self.count == 0 {
            prime(qh, td, bit);
        } else {
            let last = (idx + N - 1) % N;
            append(qh, &mut self.tds[last], td, bit);
        }
        self.count += 1;
    }

    /// Stop the endpoint direction and drop all dTDs.
    pub fn stop(&mut self) {
        ep_disable(self.num, self.dir_in);
        self.head = 0;
        self.count = 0;
    }

    /// OUT: enable and queue all dTDs for packets of up to SIZE bytes.
    pub fn start_out(&mut self, qhs: &mut QhList) {
        self.stop();
        qhs.qh(self.num, false).init(SIZE as u16, 1, false);
        ep_enable(self.num, false, EpType::Isochronous);
        for i in 0..N {
            self.push(qhs, i, SIZE);
        }
    }

    /// OUT: hand each received packet to `f`, oldest first, and requeue its dTD.
    pub fn service_out(&mut self, qhs: &mut QhList, mut f: impl FnMut(&[u8])) {
        while self.count > 0 && !self.tds[self.head].active() {
            let idx = self.head;
            if self.tds[idx].errors() != 0 {
                self.errors = self.errors.wrapping_add(1);
            } else {
                let len = SIZE - self.tds[idx].remaining().min(SIZE);
                f(&self.bufs[idx].0[..len]);
            }
            self.head = (self.head + 1) % N;
            self.count -= 1;
            self.push(qhs, idx, SIZE);
        }
    }

    /// IN: enable with max packet `max_packet`, nothing queued yet.
    pub fn start_in(&mut self, qhs: &mut QhList, max_packet: u16) {
        self.stop();
        qhs.qh(self.num, true).init(max_packet, 1, false);
        ep_enable(self.num, true, EpType::Isochronous);
    }

    /// IN: retire sent dTDs, then keep `depth` dTDs queued, each filled by
    /// `fill` (returns the packet length).
    pub fn service_in(&mut self, qhs: &mut QhList, depth: usize, mut fill: impl FnMut(&mut [u8]) -> usize) {
        while self.count > 0 && !self.tds[self.head].active() {
            if self.tds[self.head].errors() != 0 {
                self.errors = self.errors.wrapping_add(1);
            }
            self.head = (self.head + 1) % N;
            self.count -= 1;
        }
        while self.count < depth.min(N) {
            let idx = (self.head + self.count) % N;
            let len = fill(&mut self.bufs[idx].0).min(SIZE);
            self.push(qhs, idx, len);
        }
    }
}
