//! Flash layout and the hand-over between app and bootloader.
//!
//! ```text
//! 0x6000_0000  bootloader: FCB, IVT, code (boot ROM starts it)
//! 0x6003_F000  app info sector: magic, length, CRC-32 of the app image
//! 0x6004_0000  app image (vector table first), up to APP_MAX bytes
//! ```
//!
//! The bootloader writes the info sector last, after the whole image is in
//! flash and its CRC checks out, so an interrupted update leaves no valid
//! app and the bootloader stays in DFU mode.

use core::ptr::{read_volatile, write_volatile};

pub const FLASH_BASE: u32 = 0x6000_0000;
pub const SECTOR: u32 = 4096;
pub const PAGE: u32 = 256;

/// Bootloader image: FCB, IVT and code, up to the info sector.
pub const BOOT_SIZE: u32 = 0x3_F000;
pub const INFO_OFFSET: u32 = 0x3_F000;
pub const APP_OFFSET: u32 = 0x4_0000;
pub const APP_MAX: u32 = 0x10_0000;

pub const INFO_MAGIC: u32 = 0x4941_3244; // "D2AI"

/// App info sector contents (little endian words).
#[derive(Clone, Copy)]
pub struct AppInfo {
    pub length: u32,
    pub crc: u32,
}

impl AppInfo {
    pub const WORDS: usize = 4;

    pub fn to_words(self) -> [u32; Self::WORDS] {
        let mut w = [INFO_MAGIC, self.length, self.crc, 0];
        w[3] = crate::crc32::update(0, &words_bytes(&w[..3]));
        w
    }

    /// Read the info sector; `None` if erased or corrupt.
    pub fn read() -> Option<Self> {
        let p = (FLASH_BASE + INFO_OFFSET) as *const u32;
        let mut w = [0u32; Self::WORDS];
        for (i, x) in w.iter_mut().enumerate() {
            *x = unsafe { read_volatile(p.add(i)) };
        }
        let ok = w[0] == INFO_MAGIC
            && w[3] == crate::crc32::update(0, &words_bytes(&w[..3]))
            && w[1] != 0
            && w[1] <= APP_MAX;
        ok.then_some(AppInfo { length: w[1], crc: w[2] })
    }

    /// The app image in flash matches this info.
    pub fn app_valid(&self) -> bool {
        let app = unsafe { core::slice::from_raw_parts((FLASH_BASE + APP_OFFSET) as *const u8, self.length as usize) };
        crate::crc32::update(0, app) == self.crc
    }
}

fn words_bytes(w: &[u32]) -> [u8; 12] {
    let mut b = [0u8; 12];
    for (i, x) in w.iter().take(3).enumerate() {
        b[4 * i..4 * i + 4].copy_from_slice(&x.to_le_bytes());
    }
    b
}

/// SRC_GPR10: kept across SYSRESETREQ, cleared at power-on.
const SRC_GPR10: *mut u32 = 0x400F_8044 as *mut u32;
const DFU_REQUEST: u32 = 0xDF00_B007;

/// App side: ask the bootloader to stay in DFU mode after the next reset.
pub fn request_dfu() {
    unsafe { write_volatile(SRC_GPR10, DFU_REQUEST) };
}

/// Bootloader side: was DFU mode requested? Clears the request.
pub fn take_dfu_request() -> bool {
    let r = unsafe { read_volatile(SRC_GPR10) } == DFU_REQUEST;
    unsafe { write_volatile(SRC_GPR10, 0) };
    r
}
