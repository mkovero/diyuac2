//! diyuac2 firmware, Rust port.
//!
//! R3: own ChipIdea device stack (common/src/usb), UAC2 + vendor + DFU
//! runtime interfaces, SAI1 + eDMA + WM8960 audio path, feedback measured
//! from the SAI clock.
//!
//! The image runs at flash offset 0x40000, started by the DFU bootloader
//! (`boot/`), which the boot ROM starts from the flash beginning.
//!
//! LED (GPIO_AD_B0_05, active low): 250 ms blink until configured, 1 s once
//! configured, 50 ms if the link came up at full speed (a bug: this device
//! is high speed only), 100 ms if the codec did not answer.

#![no_std]
#![no_main]

mod audio;
mod audio_hw;
mod codec;
mod feedback;
mod gain;
mod usb;

use core::sync::atomic::Ordering;

use diyuac2_common::{clock, serial};
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

    // Audio interface first: the SAI must be clocking (MCLK) before the
    // codec is configured, and before the USB stack measures feedback.
    let i2c = audio_hw::init(audio_hw::Pins {
        mclk: pads.gpio_ad_b1.p00,
        tx_bclk: pads.gpio_ad_b1.p01,
        tx_sync: pads.gpio_ad_b1.p02,
        tx_data: pads.gpio_ad_b1.p03,
        rx_data: pads.gpio_ad_b1.p05,
        scl: pads.gpio_ad_b1.p14,
        sda: pads.gpio_ad_b1.p15,
    });
    let rate = audio::DEFAULT_RATE;
    if !audio_hw::start(rate) {
        panic!();
    }
    let mut codec = codec::Codec::new(i2c);
    let codec_ok = codec.init() && codec.set_rate(rate);

    usb::init(serial::get(), |audio| audio.attach(codec, codec_ok, rate));

    let mut last_toggle = cortex_m::peripheral::DWT::cycle_count();
    loop {
        if audio::DETACH_REQUESTED.load(Ordering::Acquire) {
            usb::restart_into_bootloader();
        }
        let period_ms = match usb::LINK.load(Ordering::Relaxed) {
            usb::LINK_FULL_SPEED => 50,
            _ if !codec_ok => 100,
            usb::LINK_CONFIGURED => 1000,
            _ => 250,
        };
        let now = cortex_m::peripheral::DWT::cycle_count();
        if now.wrapping_sub(last_toggle) >= period_ms * (clock::AHB_HZ / 1000) {
            last_toggle = now;
            led.toggle();
        }
    }
}
