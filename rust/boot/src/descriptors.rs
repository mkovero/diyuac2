//! DFU-mode descriptors: one configuration with the DFU interface alone,
//! the same at high and full speed.

use diyuac2_common::layout::SECTOR;

pub const USB_VID: u16 = 0xCAFE;
/// The app is 0x0AC2.
pub const USB_PID: u16 = 0x0AC3;
const EP0_SIZE: u8 = 64;

pub const STR_SERIAL: u8 = 3;
/// Same serial as the app (chip unique ID).
pub const STRINGS: [&str; 5] = ["", "diyuac2", "diyuac2 bootloader", "", "diyuac2 DFU"];

/// Will detach + can download; not manifestation tolerant (the
/// bootloader resets into the new app), no upload.
pub const DFU_ATTRIBUTES: u8 = 0x09;
/// One flash sector per DNLOAD block.
pub const TRANSFER_SIZE: u16 = SECTOR as u16;

#[rustfmt::skip]
pub const DEVICE: [u8; 18] = [
    18, 0x01,
    0x00, 0x02,             // bcdUSB 2.00
    0x00, 0x00, 0x00,       // class per interface
    EP0_SIZE,
    USB_VID as u8, (USB_VID >> 8) as u8,
    USB_PID as u8, (USB_PID >> 8) as u8,
    0x00, 0x01,             // bcdDevice 1.00
    1, 2, STR_SERIAL,
    1,
];

#[rustfmt::skip]
pub const DEVICE_QUALIFIER: [u8; 10] = [10, 0x06, 0x00, 0x02, 0x00, 0x00, 0x00, EP0_SIZE, 1, 0];

#[rustfmt::skip]
pub const CONFIG: [u8; 27] = [
    9, 0x02, 27, 0, 1, 1, 0, 0xC0, 0,
    // Interface 0: DFU mode (0xFE/1/2), string 4
    9, 0x04, 0, 0, 0, 0xFE, 0x01, 0x02, 4,
    // DFU functional: detach timeout 1000 ms, DFU 1.1
    9, 0x21, DFU_ATTRIBUTES, 0xE8, 0x03, TRANSFER_SIZE as u8, (TRANSFER_SIZE >> 8) as u8, 0x10, 0x01,
];
