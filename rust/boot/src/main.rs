//! diyuac2 DFU bootloader.
//!
//! The boot ROM starts this image from the flash beginning. Without a DFU
//! request from the app (`layout::request_dfu`, kept across the reset) and
//! with a valid app (info sector and image CRC), it jumps to the app at
//! once, before touching any clock or peripheral. Otherwise it stays in USB
//! DFU 1.1 mode (VID:PID cafe:0ac3, stock `dfu-util` works) and takes a new
//! app image; see [`dfu`].
//!
//! LED (GPIO_AD_B0_05, active low): short flash every 500 ms in DFU mode.

#![no_std]
#![no_main]

mod descriptors;
mod dfu;
mod fcb;

use core::ptr::{addr_of_mut, read_volatile};

use cortex_m::peripheral::{DWT, SCB};
use diyuac2_common::layout::{self, APP_OFFSET, AppInfo, FLASH_BASE};
use diyuac2_common::usb::{Descriptors, Usb};
use diyuac2_common::{clock, flash::Flash, serial};
use imxrt_hal as hal;
use imxrt_ral as ral;
use panic_halt as _;

use dfu::Dfu;

static DESCRIPTORS: Descriptors = Descriptors {
    device: &descriptors::DEVICE,
    qualifier: &descriptors::DEVICE_QUALIFIER,
    config_hs: &descriptors::CONFIG,
    config_fs: &descriptors::CONFIG,
    strings: &descriptors::STRINGS,
    serial_index: descriptors::STR_SERIAL,
};

static mut USB: Usb<Dfu> = Usb::new(&DESCRIPTORS, Dfu::new());

const MS: u32 = clock::AHB_HZ / 1000;

#[imxrt_rt::entry]
fn main() -> ! {
    let requested = layout::take_dfu_request();
    if !requested
        && let Some(info) = AppInfo::read()
        && info.app_valid()
    {
        // SAFETY: the image passed its CRC; nothing is set up yet.
        unsafe { start_app() }
    }

    clock::init();
    let mut cp = cortex_m::Peripherals::take().unwrap();
    cp.DCB.enable_trace();
    cp.DWT.enable_cycle_counter();

    let pads = unsafe { hal::iomuxc::into_pads(ral::iomuxc::IOMUXC::instance()) };
    let mut gpio1 = hal::gpio::Port::new(unsafe { ral::gpio::GPIO1::instance() });
    let Ok(led) = gpio1.output(pads.gpio_ad_b0.p05) else {
        panic!()
    };

    // SAFETY: the only user; the USB interrupt stays masked (polled).
    let usb = unsafe { &mut *addr_of_mut!(USB) };
    usb.function.set_flash(Flash::new(&fcb::rom_config()));
    usb.start(serial::get(), false, false);

    let start = DWT::cycle_count();
    loop {
        usb.poll();
        if usb.function.reset_due(DWT::cycle_count()) {
            usb.stop();
            let t = DWT::cycle_count();
            while DWT::cycle_count().wrapping_sub(t) < 50 * MS {}
            SCB::sys_reset();
        }
        let phase = DWT::cycle_count().wrapping_sub(start) / MS % 500;
        if (phase < 50) == led.is_set() {
            // Active low: on during the first 50 ms of each 500 ms.
            led.toggle();
        }
    }
}

/// Jump to the app: its vector table is the first thing in its partition.
unsafe fn start_app() -> ! {
    let base = FLASH_BASE + APP_OFFSET;
    unsafe {
        let sp = read_volatile(base as *const u32);
        let pc = read_volatile((base + 4) as *const u32);
        (*SCB::PTR).vtor.write(base);
        core::arch::asm!(
            "msr msp, {sp}",
            "bx {pc}",
            sp = in(reg) sp,
            pc = in(reg) pc,
            options(noreturn),
        );
    }
}
