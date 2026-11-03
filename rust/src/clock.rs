//! Clock tree setup for the i.MX RT1021 (MIMXRT1020-EVK).
//!
//! Core/AHB at 500 MHz from PLL6 (ENET PLL), IPG at 125 MHz, same as the
//! C firmware (MCUXpresso `BOARD_BootClockRUN`). Adapted from the imxrt-hal
//! board crate (`imxrt10xx/clock_tree/pll6_ahb.rs`), which targets the 1010
//! and documents the 1020 as using the same scheme.

use imxrt_hal as hal;
use imxrt_ral as ral;

use hal::ccm;

/// Core / AHB clock, Hz.
pub const AHB_HZ: u32 = ccm::analog::pll6::FREQUENCY;
const _: () = assert!(AHB_HZ == 500_000_000);

/// IPG divider: 500 MHz / 4 = 125 MHz.
const IPG_DIVIDER: u32 = 4;

/// VDD_SOC needed for 500 MHz operation (RT1020 datasheet, overdrive run mode).
const VDD_SOC_MV: u32 = 1250;

/// Configure power and clocks. Call once, early.
pub fn init() {
    let mut ccm = unsafe { ral::ccm::CCM::instance() };
    let mut ccm_analog = unsafe { ral::ccm_analog::CCM_ANALOG::instance() };
    let mut dcdc = unsafe { ral::dcdc::DCDC::instance() };

    ccm::set_low_power_mode(&mut ccm, ccm::LowPowerMode::RemainInRun);
    hal::dcdc::set_target_vdd_soc(&mut dcdc, VDD_SOC_MV);

    ccm::clock_gate::IPG_CLOCK_GATES
        .iter()
        .for_each(|locator| locator.set(&mut ccm, ccm::clock_gate::OFF));

    if ccm::ahb_clk::selection(&ccm) == ccm::ahb_clk::Selection::PeriphClk2Sel {
        // Switch to the pre-peripheral clock before changing peripheral clock 2.
        ccm::ahb_clk::set_selection(&mut ccm, ccm::ahb_clk::Selection::PrePeriphClkSel);
    }

    // Run from the 24 MHz crystal while PLL6 restarts.
    ccm::periph_clk2::set_selection(&mut ccm, ccm::periph_clk2::Selection::Osc);
    ccm::ahb_clk::set_selection(&mut ccm, ccm::ahb_clk::Selection::PeriphClk2Sel);

    ccm::analog::pll6::restart(&mut ccm_analog);
    ccm::ahb_clk::set_divider(&mut ccm, 1);
    ccm::pre_periph_clk::set_selection(&mut ccm, ccm::pre_periph_clk::Selection::Pll6);
    ccm::ahb_clk::set_selection(&mut ccm, ccm::ahb_clk::Selection::PrePeriphClkSel);

    ccm::ipg_clk::set_divider(&mut ccm, IPG_DIVIDER);

    // USB1 PLL (PLL3, 480 MHz) feeds the USB PHY.
    ccm::analog::pll3::restart(&mut ccm_analog);

    for gate in [
        ccm::clock_gate::gpio::<1>(),
        ccm::clock_gate::usb(),
    ] {
        gate.set(&mut ccm, ccm::clock_gate::ON);
    }
    // OCOTP controller (CCGR2 CG6), for the unique ID. imxrt-hal has no locator for it.
    ral::modify_reg!(ral::ccm, ccm, CCGR2, CG6: 0b11);
}
