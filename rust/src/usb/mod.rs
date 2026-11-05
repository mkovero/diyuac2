//! The app's USB device: the shared device core (`diyuac2_common::usb`)
//! with the audio function ([`crate::audio::Audio`]), driven from the
//! USB_OTG1 interrupt.

pub mod descriptors;

use core::ptr::addr_of_mut;

use cortex_m::peripheral::NVIC;
use imxrt_ral as ral;

pub use diyuac2_common::usb::*;
use diyuac2_common::{clock, layout};

use crate::audio::Audio;
use descriptors as desc;

static DESCRIPTORS: Descriptors = Descriptors {
    device: &desc::DEVICE,
    qualifier: &desc::DEVICE_QUALIFIER,
    config_hs: &desc::CONFIG,
    config_fs: &desc::FS_CONFIG,
    strings: &desc::STRINGS,
    serial_index: desc::STR_SERIAL,
};

const _: () = assert!(desc::CONFIG.len() <= EP0_BUF_SIZE);

static mut USB: Usb<Audio> = Usb::new(&DESCRIPTORS, Audio::new());

/// The driver state. Only the USB ISR and `init` (before the IRQ is
/// unmasked) may call this.
#[allow(clippy::mut_from_ref)]
unsafe fn usb() -> &'static mut Usb<Audio> {
    unsafe { &mut *addr_of_mut!(USB) }
}

/// Bring up USB and attach. `serial` is the 32-character serial number;
/// `audio_setup` runs on the audio function before the USB IRQ is unmasked.
/// The USB clock gate and PLL3 must be on.
pub fn init(serial: &str, audio_setup: impl FnOnce(&mut Audio)) {
    // SAFETY: the USB IRQ is still masked.
    let usb = unsafe { usb() };
    audio_setup(&mut usb.function);
    // SOF every microframe: feedback measurement and playback ring
    // housekeeping. The `force-full-speed` test build disables the chirp.
    usb.start(serial, true, cfg!(feature = "force-full-speed"));
    unsafe {
        let mut cp = cortex_m::Peripherals::steal();
        cp.NVIC.set_priority(ral::Interrupt::USB_OTG1, 2 << 4);
        NVIC::unmask(ral::Interrupt::USB_OTG1);
    }
}

/// DFU_DETACH: leave the bus, then reset into the bootloader, which stays
/// in DFU mode because of the request flag (`layout::request_dfu`).
pub fn restart_into_bootloader() -> ! {
    NVIC::mask(ral::Interrupt::USB_OTG1);
    // SAFETY: the USB IRQ is masked, so the ISR no longer runs.
    unsafe { usb() }.stop();
    // Let the host see the disconnect before the bootloader attaches.
    let start = cortex_m::peripheral::DWT::cycle_count();
    while cortex_m::peripheral::DWT::cycle_count().wrapping_sub(start) < 50 * (clock::AHB_HZ / 1000) {}
    layout::request_dfu();
    cortex_m::peripheral::SCB::sys_reset();
}

#[imxrt_rt::interrupt]
fn USB_OTG1() {
    // SAFETY: the only user after init.
    unsafe { usb() }.poll();
}

use ral::Interrupt as interrupt;
