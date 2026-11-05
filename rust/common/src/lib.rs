//! Code shared by the diyuac2 app and its DFU bootloader.

#![no_std]

pub mod clock;
pub mod crc32;
pub mod flash;
pub mod layout;
pub mod profile;
pub mod serial;
pub mod usb;
