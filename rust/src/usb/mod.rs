//! USB device stack for the ChipIdea HS controller: bus events, EP0 control
//! transfers and standard requests. Everything runs in the USB_OTG1 ISR;
//! class, vendor and streaming work is delegated to [`crate::audio`].

pub mod descriptors;
pub mod hw;

use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicU8, Ordering};

use cortex_m::peripheral::NVIC;
use imxrt_ral as ral;

use crate::audio::Audio;
use crate::profile::{self, Section};
use descriptors as desc;
use hw::{QhList, Td, ep_bit};

/// Control request (SETUP packet).
#[derive(Clone, Copy, Default)]
pub struct Request {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

pub const KIND_STANDARD: u8 = 0;
pub const KIND_CLASS: u8 = 1;
pub const KIND_VENDOR: u8 = 2;
pub const RCPT_DEVICE: u8 = 0;
pub const RCPT_INTERFACE: u8 = 1;
pub const RCPT_ENDPOINT: u8 = 2;

impl Request {
    fn parse(b: [u8; 8]) -> Self {
        Request {
            request_type: b[0],
            request: b[1],
            value: u16::from_le_bytes([b[2], b[3]]),
            index: u16::from_le_bytes([b[4], b[5]]),
            length: u16::from_le_bytes([b[6], b[7]]),
        }
    }
    pub fn dir_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }
    pub fn kind(&self) -> u8 {
        (self.request_type >> 5) & 0b11
    }
    pub fn recipient(&self) -> u8 {
        self.request_type & 0x1F
    }
}

/// Handler verdict for a control request.
pub enum Reply {
    /// Protocol stall.
    Stall,
    /// No data stage (or OUT data consumed): send the status ZLP.
    Ack,
    /// IN data stage: the first `n` bytes of the EP0 buffer.
    In(usize),
    /// OUT data stage expected: receive `wLength` bytes, then call the OUT handler.
    Out,
}

pub const EP0_BUF_SIZE: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ep0State {
    Idle,
    DataIn,
    StatusOut,
    DataOut,
    StatusIn,
}

#[repr(C, align(32))]
struct Ep0Buf([u8; EP0_BUF_SIZE]);

/// Link state for the main loop (LED).
pub const LINK_DETACHED: u8 = 0;
pub const LINK_DEFAULT: u8 = 1;
pub const LINK_CONFIGURED: u8 = 2;
/// Connected at full speed: a bug for this device (high speed only).
pub const LINK_FULL_SPEED: u8 = 3;
pub static LINK: AtomicU8 = AtomicU8::new(LINK_DETACHED);

const STANDARD_GET_STATUS: u8 = 0x00;
const STANDARD_CLEAR_FEATURE: u8 = 0x01;
const STANDARD_SET_FEATURE: u8 = 0x03;
const STANDARD_SET_ADDRESS: u8 = 0x05;
const STANDARD_GET_DESCRIPTOR: u8 = 0x06;
const STANDARD_GET_CONFIGURATION: u8 = 0x08;
const STANDARD_SET_CONFIGURATION: u8 = 0x09;
const STANDARD_GET_INTERFACE: u8 = 0x0A;
const STANDARD_SET_INTERFACE: u8 = 0x0B;

pub struct Usb {
    qhs: QhList,
    ep0_in: Td,
    ep0_zlp: Td,
    ep0_out: Td,
    ep0_buf: Ep0Buf,
    ep0_state: Ep0State,
    /// The ZLP dTD is linked behind the IN data dTD.
    ep0_zlp_used: bool,
    req: Request,
    configured: bool,
    high_speed: bool,
    serial: [u8; 32],
    pub audio: Audio,
}

static mut USB: Usb = Usb::new();

/// The driver state. Only the USB ISR and `init` (before the IRQ is
/// unmasked) may call this.
#[allow(clippy::mut_from_ref)]
unsafe fn usb() -> &'static mut Usb {
    unsafe { &mut *addr_of_mut!(USB) }
}

/// Bring up the PHY and controller and attach. `serial` is the 32-character
/// serial number; `audio_setup` runs on the audio function before the USB
/// IRQ is unmasked. The USB clock gate and PLL3 must be on.
pub fn init(serial: &str, audio_setup: impl FnOnce(&mut Audio)) {
    // SAFETY: the USB IRQ is still masked.
    let usb = unsafe { usb() };
    usb.serial.copy_from_slice(&serial.as_bytes()[..32]);
    audio_setup(&mut usb.audio);

    let phy = unsafe { ral::usbphy::USBPHY::instance() };
    ral::write_reg!(ral::usbphy, phy, CTRL_SET, SFTRST: 1);
    ral::write_reg!(ral::usbphy, phy, CTRL_CLR, SFTRST: 1);
    ral::write_reg!(ral::usbphy, phy, CTRL_CLR, CLKGATE: 1);
    ral::write_reg!(ral::usbphy, phy, PWD, 0);

    let regs = hw::regs();
    ral::write_reg!(ral::usb, regs, USBCMD, hw::USBCMD_RST);
    while ral::read_reg!(ral::usb, regs, USBCMD) & hw::USBCMD_RST != 0 {}
    // Interrupt threshold: immediate (default is 8 microframes).
    ral::write_reg!(ral::usb, regs, USBCMD, 0);
    ral::write_reg!(ral::usb, regs, USBMODE, hw::USBMODE_CM_DEVICE | hw::USBMODE_SLOM);
    // PFSC = 0: allow high-speed chirp. The `force-full-speed` test build
    // sets it, to exercise the full-speed path.
    if cfg!(feature = "force-full-speed") {
        ral::modify_reg!(ral::usb, regs, PORTSC1, |v| v | (1 << 24));
    } else {
        ral::modify_reg!(ral::usb, regs, PORTSC1, |v| v & !(1 << 24));
    }
    usb.ep0_init();
    ral::write_reg!(ral::usb, regs, ASYNCLISTADDR, &usb.qhs as *const QhList as u32);
    let status = ral::read_reg!(ral::usb, regs, USBSTS);
    ral::write_reg!(ral::usb, regs, USBSTS, status);
    ral::write_reg!(
        ral::usb,
        regs,
        USBINTR,
        hw::USBSTS_UI
            | hw::USBSTS_UEI
            | hw::USBSTS_PCI
            | hw::USBSTS_URI
            | hw::USBSTS_SEI
            | hw::USBSTS_SLI
            | hw::USBSTS_SRI
    );

    unsafe {
        let mut cp = cortex_m::Peripherals::steal();
        cp.NVIC.set_priority(ral::Interrupt::USB_OTG1, 2 << 4);
        NVIC::unmask(ral::Interrupt::USB_OTG1);
    }
    ral::modify_reg!(ral::usb, regs, USBCMD, |v| v | hw::USBCMD_RS);
}

#[imxrt_rt::interrupt]
fn USB_OTG1() {
    // SAFETY: the only user after init.
    unsafe { usb() }.isr();
}

use ral::Interrupt as interrupt;

impl Usb {
    const fn new() -> Self {
        Usb {
            qhs: QhList::new(),
            ep0_in: Td::new(),
            ep0_zlp: Td::new(),
            ep0_out: Td::new(),
            ep0_buf: Ep0Buf([0; EP0_BUF_SIZE]),
            ep0_state: Ep0State::Idle,
            ep0_zlp_used: false,
            req: Request { request_type: 0, request: 0, value: 0, index: 0, length: 0 },
            configured: false,
            high_speed: false,
            serial: [0; 32],
            audio: Audio::new(),
        }
    }

    fn ep0_init(&mut self) {
        self.qhs.qh(0, false).init(desc::EP0_SIZE, 0, true);
        self.qhs.qh(0, true).init(desc::EP0_SIZE, 0, false);
        self.ep0_state = Ep0State::Idle;
    }

    fn isr(&mut self) {
        let entry = profile::enter();
        let regs = hw::regs();
        let status = ral::read_reg!(ral::usb, regs, USBSTS) & ral::read_reg!(ral::usb, regs, USBINTR);
        ral::write_reg!(ral::usb, regs, USBSTS, status);

        if status & hw::USBSTS_URI != 0 {
            self.bus_reset();
        }
        if status & hw::USBSTS_PCI != 0 {
            let portsc = ral::read_reg!(ral::usb, regs, PORTSC1);
            // Port change also fires on suspend/resume; only judge speed
            // when the port is enabled and not in reset.
            if portsc & (1 << 8) == 0 {
                self.high_speed =
                    (portsc & hw::PORTSC1_PSPD_MASK) >> hw::PORTSC1_PSPD_SHIFT == hw::PSPD_HIGH;
                self.update_link();
            }
        }
        if status & hw::USBSTS_SRI != 0 {
            // Every microframe at high speed: feedback measurement and
            // playback ring housekeeping.
            let t = profile::now();
            self.audio.sof(ral::read_reg!(ral::usb, regs, FRINDEX) & 0x3FFF);
            profile::section(Section::Sof, t);
        }
        if status & (hw::USBSTS_UI | hw::USBSTS_UEI) != 0 {
            let t = profile::now();
            let complete = ral::read_reg!(ral::usb, regs, ENDPTCOMPLETE);
            ral::write_reg!(ral::usb, regs, ENDPTCOMPLETE, complete);
            if complete & (ep_bit(0, false) | ep_bit(0, true)) != 0 {
                self.ep0_complete(complete);
            }
            if self.configured {
                self.audio.service(&mut self.qhs);
            }
            profile::section(Section::Xfer, t);
            if ral::read_reg!(ral::usb, regs, ENDPTSETUPSTAT) & 1 != 0 {
                let t = profile::now();
                self.setup();
                profile::section(Section::Setup, t);
            }
        }
        profile::exit(entry);
    }

    fn update_link(&self) {
        let link = if !self.high_speed {
            LINK_FULL_SPEED
        } else if self.configured {
            LINK_CONFIGURED
        } else {
            LINK_DEFAULT
        };
        LINK.store(link, Ordering::Relaxed);
    }

    fn bus_reset(&mut self) {
        let regs = hw::regs();
        let v = ral::read_reg!(ral::usb, regs, ENDPTSETUPSTAT);
        ral::write_reg!(ral::usb, regs, ENDPTSETUPSTAT, v);
        let v = ral::read_reg!(ral::usb, regs, ENDPTCOMPLETE);
        ral::write_reg!(ral::usb, regs, ENDPTCOMPLETE, v);
        while ral::read_reg!(ral::usb, regs, ENDPTPRIME) != 0 {}
        hw::flush(0xFFFF_FFFF);
        ral::write_reg!(ral::usb, regs, DEVICEADDR, 0);
        for n in 0..hw::NUM_EP - 1 {
            ral::write_reg!(ral::usb, regs, ENDPTCTRL[n], 0);
        }
        self.audio.reset();
        self.configured = false;
        self.ep0_init();
        self.update_link();
    }

    // --- EP0 -------------------------------------------------------------

    fn setup(&mut self) {
        let regs = hw::regs();
        ral::write_reg!(ral::usb, regs, ENDPTSETUPSTAT, 1);
        // Setup tripwire: retry the copy if a new SETUP arrived meanwhile.
        let packet = loop {
            ral::modify_reg!(ral::usb, regs, USBCMD, |v| v | hw::USBCMD_SUTW);
            let p = self.qhs.qh(0, false).setup_packet();
            if ral::read_reg!(ral::usb, regs, USBCMD) & hw::USBCMD_SUTW != 0 {
                break p;
            }
        };
        ral::modify_reg!(ral::usb, regs, USBCMD, |v| v & !hw::USBCMD_SUTW);
        // Drop whatever is left of an aborted previous transfer.
        hw::flush(ep_bit(0, false) | ep_bit(0, true));

        let req = Request::parse(packet);
        self.req = req;
        let reply = if req.kind() == KIND_STANDARD {
            self.standard(&req)
        } else {
            self.audio.control(&req, &mut self.ep0_buf.0, &mut self.qhs)
        };
        self.reply(reply);
    }

    fn reply(&mut self, reply: Reply) {
        let req = self.req;
        match reply {
            Reply::Stall => {
                hw::ep0_stall();
                self.ep0_state = Ep0State::Idle;
            }
            Reply::Ack => self.status_in(),
            Reply::In(n) => {
                let len = n.min(req.length as usize).min(EP0_BUF_SIZE);
                let buf = self.ep0_buf.0.as_ptr();
                self.ep0_in.arm(buf, len);
                self.ep0_zlp_used = false;
                // A short reply that is a multiple of the packet size needs a
                // ZLP to end the data stage.
                if len < req.length as usize && len > 0 && len % desc::EP0_SIZE as usize == 0 {
                    self.ep0_zlp.arm(buf, 0);
                    self.ep0_in.link(&self.ep0_zlp);
                    self.ep0_zlp_used = true;
                }
                self.ep0_state = Ep0State::DataIn;
                let td: *const Td = &self.ep0_in;
                hw::prime(self.qhs.qh(0, true), td, ep_bit(0, true));
            }
            Reply::Out => {
                let len = req.length as usize;
                if len == 0 || len > EP0_BUF_SIZE {
                    hw::ep0_stall();
                    self.ep0_state = Ep0State::Idle;
                    return;
                }
                self.ep0_out.arm(self.ep0_buf.0.as_ptr(), len);
                self.ep0_state = Ep0State::DataOut;
                let td: *const Td = &self.ep0_out;
                hw::prime(self.qhs.qh(0, false), td, ep_bit(0, false));
            }
        }
    }

    fn status_in(&mut self) {
        self.ep0_in.arm(self.ep0_buf.0.as_ptr(), 0);
        self.ep0_state = Ep0State::StatusIn;
        let td: *const Td = &self.ep0_in;
        hw::prime(self.qhs.qh(0, true), td, ep_bit(0, true));
    }

    fn ep0_complete(&mut self, complete: u32) {
        if complete & ep_bit(0, true) != 0 {
            match self.ep0_state {
                Ep0State::DataIn if !self.ep0_in.active() && !self.ep0_zlp_pending() => {
                    self.ep0_out.arm(self.ep0_buf.0.as_ptr(), 0);
                    self.ep0_state = Ep0State::StatusOut;
                    let td: *const Td = &self.ep0_out;
                    hw::prime(self.qhs.qh(0, false), td, ep_bit(0, false));
                }
                // SET_ADDRESS: hardware applies the address (USBADRA) after this stage.
                Ep0State::StatusIn if !self.ep0_in.active() => self.ep0_state = Ep0State::Idle,
                _ => {}
            }
        }
        if complete & ep_bit(0, false) != 0 {
            match self.ep0_state {
                Ep0State::DataOut if !self.ep0_out.active() => {
                    let req = self.req;
                    let len = (req.length as usize).saturating_sub(self.ep0_out.remaining());
                    let reply = if self.ep0_out.errors() != 0 {
                        Reply::Stall
                    } else {
                        self.audio.control_out(&req, &self.ep0_buf.0[..len])
                    };
                    match reply {
                        Reply::Ack => self.status_in(),
                        _ => {
                            hw::ep0_stall();
                            self.ep0_state = Ep0State::Idle;
                        }
                    }
                }
                Ep0State::StatusOut if !self.ep0_out.active() => {
                    self.ep0_state = Ep0State::Idle;
                }
                _ => {}
            }
        }
    }

    fn ep0_zlp_pending(&self) -> bool {
        self.ep0_zlp_used && self.ep0_zlp.active()
    }

    // --- standard requests ---------------------------------------------

    fn standard(&mut self, req: &Request) -> Reply {
        let buf = &mut self.ep0_buf.0;
        match (req.recipient(), req.request) {
            (_, STANDARD_GET_STATUS) if req.dir_in() => {
                // Device: self powered. Interface/endpoint: 0.
                buf[0] = (req.recipient() == RCPT_DEVICE) as u8;
                buf[1] = 0;
                Reply::In(2)
            }
            (_, STANDARD_CLEAR_FEATURE | STANDARD_SET_FEATURE) => Reply::Ack,
            (RCPT_DEVICE, STANDARD_SET_ADDRESS) => {
                let regs = hw::regs();
                ral::write_reg!(
                    ral::usb,
                    regs,
                    DEVICEADDR,
                    ((req.value as u32 & 0x7F) << hw::DEVICEADDR_USBADR_SHIFT) | hw::DEVICEADDR_USBADRA
                );
                Reply::Ack
            }
            (RCPT_DEVICE, STANDARD_GET_DESCRIPTOR) => self.descriptor(req),
            (RCPT_DEVICE, STANDARD_GET_CONFIGURATION) => {
                buf[0] = if self.configured { desc::CONFIG_VALUE } else { 0 };
                Reply::In(1)
            }
            (RCPT_DEVICE, STANDARD_SET_CONFIGURATION) => match req.value as u8 {
                0 => {
                    self.audio.reset();
                    self.configured = false;
                    self.update_link();
                    Reply::Ack
                }
                desc::CONFIG_VALUE => {
                    self.audio.reset();
                    self.configured = true;
                    self.update_link();
                    Reply::Ack
                }
                _ => Reply::Stall,
            },
            (RCPT_INTERFACE, STANDARD_GET_INTERFACE) if self.configured => {
                match self.audio.get_interface(req.index as u8) {
                    Some(alt) => {
                        buf[0] = alt;
                        Reply::In(1)
                    }
                    None => Reply::Stall,
                }
            }
            (RCPT_INTERFACE, STANDARD_SET_INTERFACE) if self.configured => {
                if self.audio.set_interface(req.index as u8, req.value as u8, &mut self.qhs) {
                    Reply::Ack
                } else {
                    Reply::Stall
                }
            }
            _ => Reply::Stall,
        }
    }

    fn descriptor(&mut self, req: &Request) -> Reply {
        let index = req.value as u8;
        let buf = &mut self.ep0_buf.0;
        let src: &[u8] = match (req.value >> 8) as u8 {
            0x01 => &desc::DEVICE,
            0x02 => &desc::CONFIG,
            0x06 => &desc::DEVICE_QUALIFIER,
            0x07 => &desc::OTHER_SPEED_CONFIG,
            0x03 => {
                let n = match index {
                    0 => {
                        buf[..4].copy_from_slice(&[4, 0x03, 0x09, 0x04]); // en-US
                        return Reply::In(4);
                    }
                    desc::STR_SERIAL => string_descriptor(buf, &self.serial),
                    i if (i as usize) < desc::STRINGS.len() => {
                        string_descriptor(buf, desc::STRINGS[i as usize].as_bytes())
                    }
                    _ => return Reply::Stall,
                };
                return Reply::In(n);
            }
            _ => return Reply::Stall,
        };
        buf[..src.len()].copy_from_slice(src);
        Reply::In(src.len())
    }
}

/// UTF-16LE string descriptor from ASCII.
fn string_descriptor(buf: &mut [u8], s: &[u8]) -> usize {
    let n = s.len().min(126);
    buf[0] = (2 + 2 * n) as u8;
    buf[1] = 0x03;
    for (i, &c) in s[..n].iter().enumerate() {
        buf[2 + 2 * i] = c;
        buf[3 + 2 * i] = 0;
    }
    2 + 2 * n
}
