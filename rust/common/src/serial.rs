//! USB serial number from the chip unique ID, same format as the C firmware
//! (TinyUSB `board_usb_get_serial`): OCOTP shadow words 1..4, little-endian
//! bytes, each byte as two uppercase hex digits, 32 characters.

use imxrt_ral as ral;

static mut SERIAL: [u8; 32] = [0; 32];

/// Build the serial string. Call once; the OCOTP clock gate must be on.
pub fn get() -> &'static str {
    let ocotp = unsafe { ral::ocotp::OCOTP::instance() };
    let words = [
        ral::read_reg!(ral::ocotp, ocotp, CFG0),
        ral::read_reg!(ral::ocotp, ocotp, CFG1),
        ral::read_reg!(ral::ocotp, ocotp, CFG2),
        ral::read_reg!(ral::ocotp, ocotp, CFG3),
    ];
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    // SAFETY: called once from main before anything else reads SERIAL.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(SERIAL) };
    for (i, b) in words.iter().flat_map(|w| w.to_le_bytes()).enumerate() {
        buf[2 * i] = HEX[(b >> 4) as usize];
        buf[2 * i + 1] = HEX[(b & 0xF) as usize];
    }
    // SAFETY: only ASCII hex digits were written.
    unsafe { core::str::from_utf8_unchecked(buf) }
}
