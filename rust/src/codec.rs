//! WM8960 codec (MIMXRT1020-EVK, on LPI2C1 at 0x1A). Port of
//! `firmware/app/codec_wm8960.c`.
//!
//! Slave I2S, 32-bit words, SYSCLK = MCLK (no PLL), ADC/DAC = SYSCLK/256.
//! The WM8960 is specified for 8..48 kHz, so only 48 kHz is offered: 96 and
//! 192 kHz would need SYSCLK/128 and /64 ratios the part does not support.
//!
//! Register values follow the NXP SDK wm8960 driver defaults for this board
//! (power, mixers, input selection), with gains changed for measurement:
//! DAC 0 dB, headphone 0 dB, ADC 0 dB, input PGA 0 dB, mic boost 0 dB.
//!
//! Inputs (left = LINPUT1/LINPUT3 diff, right = RINPUT1/RINPUT2 diff) as in
//! the NXP SDK. Measured on the C firmware (2026-11-03): the onboard mic P1
//! is on the right channel; RINPUT3 is dead (likely J11 jack detect).

use embedded_hal::i2c::I2c as _;

use crate::audio_hw::I2c;

const ADDR: u8 = 0x1A;

/// Sample rates this codec supports in the 256 fs clocking scheme, ascending.
/// Rates within the WM8960 specification (8..48 kHz) in this clocking
/// scheme (MCLK = 256 fs). The device also runs 96 and 192 kHz with the
/// codec out of spec ([`crate::audio::RATES`]).
pub const RATES: &[u32] = &[48_000];

const R_LINVOL: u8 = 0x00;
const R_RINVOL: u8 = 0x01;
const R_LOUT1: u8 = 0x02;
const R_ROUT1: u8 = 0x03;
const R_CLOCK1: u8 = 0x04;
const R_DACCTL1: u8 = 0x05;
const R_IFACE1: u8 = 0x07;
const R_IFACE2: u8 = 0x09;
const R_LDAC: u8 = 0x0A;
const R_RDAC: u8 = 0x0B;
const R_RESET: u8 = 0x0F;
const R_LADC: u8 = 0x15;
const R_RADC: u8 = 0x16;
const R_ADDCTL1: u8 = 0x17;
const R_POWER1: u8 = 0x19;
const R_POWER2: u8 = 0x1A;
const R_LINPATH: u8 = 0x20;
const R_RINPATH: u8 = 0x21;
const R_LOUTMIX: u8 = 0x22;
const R_ROUTMIX: u8 = 0x25;
const R_BYPASS1: u8 = 0x2D;
const R_BYPASS2: u8 = 0x2E;
const R_POWER3: u8 = 0x2F;
const R_ADDCTL4: u8 = 0x30;

const INIT: &[(u8, u16)] = &[
    (R_RESET, 0x000),
    (R_POWER1, 0x0FE),  // VMID 50k, VREF, AINL/R, ADCL/R, MICB
    (R_POWER2, 0x1E0),  // DACL/R, LOUT1/ROUT1 (speaker amps off)
    (R_POWER3, 0x03C),  // LMIC/RMIC, LOMIX/ROMIX
    (R_CLOCK1, 0x000),  // SYSCLK = MCLK, ADC/DAC = SYSCLK/256
    (R_IFACE1, 0x00E),  // slave, 32-bit, I2S
    (R_IFACE2, 0x040),  // ADCLRC pin as GPIO: ADC uses DACLRC
    (R_LOUTMIX, 0x100), // left DAC to left output mixer
    (R_ROUTMIX, 0x100), // right DAC to right output mixer
    (R_ADDCTL1, 0x0C0), // VSEL = 3.3 V
    (R_ADDCTL4, 0x040),
    (R_BYPASS1, 0x000),
    (R_BYPASS2, 0x000),
    (R_LADC, 0x1C3),    // ADC 0 dB, update
    (R_RADC, 0x1C3),
    (R_LDAC, 0x1FF),    // DAC 0 dB, update
    (R_RDAC, 0x1FF),
    (R_LOUT1, 0x179),   // headphone 0 dB, update
    (R_ROUT1, 0x179),
    (R_DACCTL1, 0x000), // DAC unmute, no de-emphasis
    (R_LINPATH, 0x188), // LMN1, LMP3, boost 0 dB, to PGA
    (R_RINPATH, 0x148), // RMN1, RMP2, boost 0 dB, to PGA
    (R_LINVOL, 0x117),  // input PGA 0 dB, unmuted, update
    (R_RINVOL, 0x117),
];

pub struct Codec {
    i2c: I2c,
}

impl Codec {
    pub fn new(i2c: I2c) -> Self {
        Codec { i2c }
    }

    /// Registers are write-only: 7-bit address, 9-bit value.
    fn write(&mut self, reg: u8, val: u16) -> bool {
        let b = [(reg << 1) | ((val >> 8) as u8 & 1), val as u8];
        self.i2c.write(ADDR, &b).is_ok()
    }

    /// Power up and configure. MCLK must be running.
    pub fn init(&mut self) -> bool {
        INIT.iter().all(|&(reg, val)| self.write(reg, val))
    }

    /// Debug: raw register write.
    pub fn write_reg(&mut self, reg: u16, val: u16) -> bool {
        reg <= 0x37 && val <= 0x1FF && self.write(reg as u8, val)
    }

    /// Called after the audio interface restarted at a new rate. The ratio
    /// is fixed at 256 fs: nothing to change for supported rates.
    pub fn set_rate(&mut self, rate: u32) -> bool {
        RATES.contains(&rate)
    }
}
