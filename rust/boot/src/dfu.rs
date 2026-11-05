//! USB DFU 1.1, DFU mode: download into the app partition.
//!
//! Each DNLOAD block is one 4 KiB flash sector: erased, programmed and read
//! back in the OUT data stage, before the status stage, so GETSTATUS never
//! asks the host to wait. The first block erases the app info sector, so
//! from then on no valid app exists until the download completes. The
//! zero-length DNLOAD ends the download; the following GETSTATUS checks the
//! image (vector table, CRC of the flash contents against the CRC of the
//! data received) and writes the info sector. The device is not
//! manifestation tolerant: it reports dfuMANIFEST, then
//! dfuMANIFEST-WAIT-RESET, and resets into the new app on the next bus
//! reset or after [`RESET_TIMEOUT_MS`].
//!
//! UPLOAD is not supported. DETACH resets (into the app, if valid).

use core::ptr::read_volatile;

use diyuac2_common::crc32;
use diyuac2_common::flash::{self, Flash};
use diyuac2_common::layout::{self, APP_MAX, APP_OFFSET, AppInfo, FLASH_BASE, INFO_OFFSET, PAGE, SECTOR};
use diyuac2_common::usb::hw::QhList;
use diyuac2_common::usb::{Function, KIND_CLASS, RCPT_INTERFACE, Reply, Request};

use crate::MS;

// Requests
const DETACH: u8 = 0;
const DNLOAD: u8 = 1;
const GETSTATUS: u8 = 3;
const CLRSTATUS: u8 = 4;
const GETSTATE: u8 = 5;
const ABORT: u8 = 6;

// States
const DFU_IDLE: u8 = 2;
const DNLOAD_SYNC: u8 = 3;
const DNLOAD_IDLE: u8 = 5;
const MANIFEST_SYNC: u8 = 6;
const MANIFEST: u8 = 7;
const MANIFEST_WAIT_RESET: u8 = 8;
const DFU_ERROR: u8 = 10;

// Status codes
const OK: u8 = 0x00;
const ERR_FILE: u8 = 0x02;
const ERR_WRITE: u8 = 0x03;
const ERR_ERASE: u8 = 0x04;
const ERR_VERIFY: u8 = 0x07;
const ERR_ADDRESS: u8 = 0x08;
const ERR_NOTDONE: u8 = 0x09;
const ERR_STALLEDPKT: u8 = 0x0F;

/// Reset this long after the manifestation even without a bus reset.
const RESET_TIMEOUT_MS: u32 = 3000;

/// RAM ranges the app's initial stack pointer may be in: DTCM, OCRAM.
const SP_RANGES: [(u32, u32); 2] = [(0x2000_0000, 0x2002_0000), (0x2020_0000, 0x2024_0000)];

pub struct Dfu {
    flash: Option<Result<Flash, flash::Error>>,
    state: u8,
    status: u8,
    /// wValue of the first block of the download.
    first_block: u16,
    /// Bytes received so far.
    length: u32,
    /// CRC-32 of the bytes received.
    crc: u32,
    /// A short block was received: it must be the last.
    short: bool,
    /// DWT cycle count when the manifestation finished.
    manifested_at: Option<u32>,
    reset_now: bool,
}

impl Dfu {
    pub const fn new() -> Self {
        Dfu {
            flash: None,
            state: DFU_IDLE,
            status: OK,
            first_block: 0,
            length: 0,
            crc: 0,
            short: false,
            manifested_at: None,
            reset_now: false,
        }
    }

    pub fn set_flash(&mut self, flash: Result<Flash, flash::Error>) {
        self.flash = Some(flash);
    }

    /// Time to reset (DETACH, or the new app is in place).
    pub fn reset_due(&self, now: u32) -> bool {
        self.reset_now || self.manifested_at.is_some_and(|t| now.wrapping_sub(t) >= RESET_TIMEOUT_MS * MS)
    }

    fn fail(&mut self, status: u8) -> Reply {
        self.state = DFU_ERROR;
        self.status = status;
        Reply::Stall
    }

    fn flash(&mut self) -> Option<&mut Flash> {
        match self.flash.as_mut() {
            Some(Ok(f)) => Some(f),
            _ => None,
        }
    }

    /// Erase, program and verify one block at app offset `offset`.
    fn write_block(&mut self, offset: u32, data: &[u8]) -> Result<(), u8> {
        let flash = self.flash().ok_or(ERR_WRITE)?;
        let base = APP_OFFSET + offset;
        flash.erase(base, SECTOR).map_err(|_| ERR_ERASE)?;
        for (i, chunk) in data.chunks(PAGE as usize).enumerate() {
            let mut page = [u32::MAX; (PAGE / 4) as usize];
            for (j, b) in chunk.iter().enumerate() {
                let w = &mut page[j / 4];
                let shift = 8 * (j % 4);
                *w = (*w & !(0xFF << shift)) | (*b as u32) << shift;
            }
            flash.program_page(base + i as u32 * PAGE, &page).map_err(|_| ERR_WRITE)?;
        }
        if flash_bytes(base, data.len()) != data {
            return Err(ERR_VERIFY);
        }
        Ok(())
    }

    fn dnload(&mut self, block: u16, data: &[u8]) -> Reply {
        if self.state == DFU_IDLE {
            // A new download: invalidate the current app first.
            let Some(flash) = self.flash() else {
                return self.fail(ERR_WRITE);
            };
            if flash.erase(INFO_OFFSET, SECTOR).is_err() {
                return self.fail(ERR_ERASE);
            }
            self.first_block = block;
            self.length = 0;
            self.crc = 0;
            self.short = false;
        }
        let index = block.wrapping_sub(self.first_block) as u32;
        if self.short || index * SECTOR != self.length {
            return self.fail(ERR_ADDRESS);
        }
        if self.length + data.len() as u32 > APP_MAX {
            return self.fail(ERR_ADDRESS);
        }
        if let Err(e) = self.write_block(self.length, data) {
            return self.fail(e);
        }
        self.crc = crc32::update(self.crc, data);
        self.length += data.len() as u32;
        self.short = data.len() < SECTOR as usize;
        self.state = DNLOAD_SYNC;
        Reply::Ack
    }

    /// Check the image and write the info sector.
    fn manifest(&mut self) -> Result<(), u8> {
        if self.length < 8 {
            return Err(ERR_NOTDONE);
        }
        let base = FLASH_BASE + APP_OFFSET;
        let sp = unsafe { read_volatile(base as *const u32) };
        let pc = unsafe { read_volatile((base + 4) as *const u32) };
        let sp_ok = SP_RANGES.iter().any(|&(lo, hi)| sp > lo && sp <= hi);
        let pc_ok = pc & 1 == 1 && (base..base + self.length).contains(&(pc & !1));
        if !sp_ok || !pc_ok {
            return Err(ERR_FILE);
        }
        let info = AppInfo { length: self.length, crc: self.crc };
        if !info.app_valid() {
            return Err(ERR_VERIFY);
        }
        let mut page = [u32::MAX; (PAGE / 4) as usize];
        page[..AppInfo::WORDS].copy_from_slice(&info.to_words());
        let flash = self.flash().ok_or(ERR_WRITE)?;
        flash.program_page(INFO_OFFSET, &page).map_err(|_| ERR_WRITE)?;
        match AppInfo::read() {
            Some(r) if r.length == info.length && r.crc == info.crc => Ok(()),
            _ => Err(ERR_VERIFY),
        }
    }

    fn get_status(&mut self, buf: &mut [u8]) -> Reply {
        match self.state {
            DNLOAD_SYNC => self.state = DNLOAD_IDLE,
            MANIFEST_SYNC => match self.manifest() {
                Ok(()) => {
                    self.state = MANIFEST;
                    self.manifested_at = Some(cortex_m::peripheral::DWT::cycle_count());
                }
                Err(e) => {
                    self.state = DFU_ERROR;
                    self.status = e;
                }
            },
            MANIFEST => self.state = MANIFEST_WAIT_RESET,
            _ => {}
        }
        // bStatus, bwPollTimeout (0 ms), bState, iString
        buf[..6].copy_from_slice(&[self.status, 0, 0, 0, self.state, 0]);
        Reply::In(6)
    }
}

/// `len` bytes of flash at flash offset `offset`.
fn flash_bytes(offset: u32, len: usize) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts((layout::FLASH_BASE + offset) as *const u8, len) }
}

impl Function for Dfu {
    fn control(&mut self, req: &Request, buf: &mut [u8], _qhs: &mut QhList) -> Reply {
        if req.kind() != KIND_CLASS || req.recipient() != RCPT_INTERFACE || req.index != 0 {
            return Reply::Stall;
        }
        match (req.request, req.dir_in()) {
            (GETSTATUS, true) if req.length >= 6 => self.get_status(buf),
            (GETSTATE, true) if req.length >= 1 => {
                buf[0] = self.state;
                Reply::In(1)
            }
            (DNLOAD, false) if self.state == DFU_IDLE || self.state == DNLOAD_IDLE => {
                if req.length > 0 {
                    Reply::Out
                } else if self.state == DNLOAD_IDLE {
                    self.state = MANIFEST_SYNC;
                    Reply::Ack
                } else {
                    self.fail(ERR_NOTDONE)
                }
            }
            (CLRSTATUS, false) if self.state == DFU_ERROR => {
                self.state = DFU_IDLE;
                self.status = OK;
                Reply::Ack
            }
            (ABORT, false) if self.state == DFU_IDLE || self.state == DNLOAD_IDLE => {
                self.state = DFU_IDLE;
                Reply::Ack
            }
            (DETACH, false) => Reply::Ack,
            _ => self.fail(ERR_STALLEDPKT),
        }
    }

    fn control_out(&mut self, req: &Request, data: &[u8]) -> Reply {
        if req.request == DNLOAD {
            self.dnload(req.value, data)
        } else {
            self.fail(ERR_STALLEDPKT)
        }
    }

    fn control_done(&mut self, req: &Request) {
        if req.request == DETACH && req.recipient() == RCPT_INTERFACE {
            self.reset_now = true;
        }
    }

    fn reset(&mut self) {
        // The host resets the bus to leave dfuMANIFEST-WAIT-RESET.
        if self.state == MANIFEST_WAIT_RESET {
            self.reset_now = true;
        }
    }

    fn get_interface(&self, itf: u8) -> Option<u8> {
        (itf == 0).then_some(0)
    }

    fn set_interface(&mut self, itf: u8, alt: u8, _qhs: &mut QhList) -> bool {
        itf == 0 && alt == 0
    }
}
