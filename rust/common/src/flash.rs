//! FlexSPI NOR flash writes through the boot ROM's flash driver (the RT1020
//! ROM API, as MCUXpresso `fsl_romapi`). The ROM programs the flash with
//! the same configuration block the boot ROM booted from (the FCB).
//!
//! The caller must not execute from or read flash while an operation runs,
//! and nothing else may (no interrupts): the code, read-only data and
//! vectors of both images are in RAM (imxrt-rt copies them at startup).

use core::ffi::c_void;

/// FlexSPI instance used by the ROM API.
const INSTANCE: u32 = 0;

/// `flexspi_nor_config_t`: the 512-byte FCB.
#[repr(C, align(4))]
pub struct Config(pub [u8; 512]);

#[repr(C)]
struct NorDriver {
    version: u32,
    init: unsafe extern "C" fn(u32, *mut Config) -> i32,
    program: unsafe extern "C" fn(u32, *mut Config, u32, *const u32) -> i32,
    _reserved0: u32,
    erase: unsafe extern "C" fn(u32, *mut Config, u32, u32) -> i32,
    _reserved1: u32,
    clear_cache: unsafe extern "C" fn(u32),
}

#[repr(C)]
struct ApiTree {
    run_bootloader: unsafe extern "C" fn(*mut c_void),
    version: u32,
    copyright: *const u8,
    _reserved0: u32,
    nor: *const NorDriver,
}

fn nor() -> &'static NorDriver {
    // SAFETY: the ROM API tree pointer lives at a fixed ROM address.
    unsafe {
        let tree = *(0x0020_001C as *const *const ApiTree);
        &*(*tree).nor
    }
}

pub struct Flash {
    config: Config,
}

/// ROM status codes: 0 is success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub i32);

fn check(status: i32) -> Result<(), Error> {
    if status == 0 { Ok(()) } else { Err(Error(status)) }
}

impl Flash {
    /// Set up the ROM driver with the FCB the chip booted from.
    pub fn new(fcb: &[u8; 512]) -> Result<Self, Error> {
        let mut f = Flash { config: Config(*fcb) };
        check(unsafe { (nor().init)(INSTANCE, &mut f.config) })?;
        Ok(f)
    }

    /// Erase `len` bytes (sector multiples) at flash offset `offset`.
    pub fn erase(&mut self, offset: u32, len: u32) -> Result<(), Error> {
        let r = check(unsafe { (nor().erase)(INSTANCE, &mut self.config, offset, len) });
        unsafe { (nor().clear_cache)(INSTANCE) };
        r
    }

    /// Program one page at flash offset `offset` (page aligned).
    pub fn program_page(&mut self, offset: u32, page: &[u32; 64]) -> Result<(), Error> {
        let r = check(unsafe { (nor().program)(INSTANCE, &mut self.config, offset, page.as_ptr()) });
        unsafe { (nor().clear_cache)(INSTANCE) };
        r
    }
}
