//! diyuac2 firmware, Rust port.
//!
//! R2: own ChipIdea device stack (src/usb), UAC2 + vendor interface,
//! isochronous playback/capture/feedback with playback looped back into
//! capture. No audio hardware yet.
//!
//! LED (GPIO_AD_B0_05, active low): 250 ms blink until configured, 1 s once
//! configured, 50 ms if the link came up at full speed (a bug: this device
//! is high speed only).

#![no_std]
#![no_main]

mod audio;
mod clock;
mod fcb;
mod serial;
mod usb;

use core::sync::atomic::Ordering;

use imxrt_hal as hal;
use imxrt_ral as ral;
use panic_halt as _;

#[imxrt_rt::entry]
fn main() -> ! {
    clock::init();

    let mut cp = cortex_m::Peripherals::take().unwrap();
    cp.DCB.enable_trace();
    cp.DWT.enable_cycle_counter();

    // USER LED D5: GPIO_AD_B0_05 = GPIO1_IO05, active low.
    let pads = unsafe { hal::iomuxc::into_pads(ral::iomuxc::IOMUXC::instance()) };
    let mut gpio1 = hal::gpio::Port::new(unsafe { ral::gpio::GPIO1::instance() });
    let Ok(led) = gpio1.output(pads.gpio_ad_b0.p05) else {
        panic!()
    };

    usb::init(serial::get());

    let mut last_toggle = cortex_m::peripheral::DWT::cycle_count();
    loop {
        let period_ms = match usb::LINK.load(Ordering::Relaxed) {
            usb::LINK_CONFIGURED => 1000,
            usb::LINK_FULL_SPEED => 50,
            _ => 250,
        };
        let now = cortex_m::peripheral::DWT::cycle_count();
        if now.wrapping_sub(last_toggle) >= period_ms * (clock::AHB_HZ / 1000) {
            last_toggle = now;
            led.toggle();
        }
    }
}
