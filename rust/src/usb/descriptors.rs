//! diyuac2 USB descriptors. Based on the C firmware
//! (`firmware/app/usb_descriptors.{c,h}`), plus the Feature Units.
//!
//! ```text
//! IAD (itf 0..2) UAC2 function
//!   itf 0  Audio Control
//!            Clock Source 0x04 (internal, programmable)
//!            Playback: IT 0x01 (USB streaming, 2ch) -> FU 0x02 -> OT 0x03 (line out)
//!            Capture:  IT 0x11 (line in, 2ch)       -> FU 0x12 -> OT 0x13 (USB streaming)
//!   itf 1  Audio Streaming OUT: alt 0 idle, alt 1 2ch 24-in-32,
//!          iso OUT EP 0x01 (async) + explicit feedback iso IN EP 0x81
//!   itf 2  Audio Streaming IN: alt 0 idle, alt 1 2ch 24-in-32, iso IN EP 0x82 (async)
//! itf 3  Vendor control interface (class 0xFF, no endpoints), see
//!        docs/vendor-protocol.md
//! itf 4  DFU runtime (class 0xFE/1/1): DFU_DETACH restarts into the
//!        bootloader (`boot/`), which takes the update
//! ```
//!
//! The device is high speed only. On a full-speed link the configuration
//! has the vendor and DFU interfaces alone ([`FS_CONFIG`]), so no audio
//! card appears, host tools can still read GET_INFO (which flags the
//! link), and a firmware update still works.
//!
//! The Feature Units have master mute and volume, applied digitally
//! (`gain.rs`). They power up at 0 dB, unmuted, where the path is
//! bit-exact; the vendor stats flag `gain` shows any other setting, so a
//! measurement run can check that the host mixer left the path alone.

/// Development IDs. 0xCAFE is TinyUSB's test VID: bench use only.
pub const USB_VID: u16 = 0xCAFE;
pub const USB_PID: u16 = 0x0AC2;

pub const EP0_SIZE: u16 = 64;

pub const ITF_AC: u8 = 0;
pub const ITF_AS_OUT: u8 = 1;
pub const ITF_AS_IN: u8 = 2;
pub const ITF_VENDOR: u8 = 3;
pub const ITF_DFU: u8 = 4;

pub const ENTITY_CLOCK: u8 = 0x04;
pub const ENTITY_FU_PLAY: u8 = 0x02;
pub const ENTITY_FU_REC: u8 = 0x12;

/// Endpoint numbers: OUT 0x01 (playback), IN 0x81 (feedback), IN 0x82 (capture).
pub const EP_AUDIO: usize = 1;
pub const EP_CAPTURE: usize = 2;

pub const N_CHANNELS: usize = 2;
pub const BYTES_PER_SAMPLE: usize = 4;
pub const FRAME_BYTES: usize = N_CHANNELS * BYTES_PER_SAMPLE;
/// Iso packet size: (192000 / 8000 + 1) * 4 * 2 = 200 bytes per microframe.
pub const EP_SIZE: usize = (192_000 / 8000 + 1) * FRAME_BYTES;
pub const FB_SIZE: usize = 4;

pub const STR_MANUFACTURER: u8 = 1;
pub const STR_PRODUCT: u8 = 2;
pub const STR_SERIAL: u8 = 3;

/// String descriptors 1..6 except the serial (index 3), which is built
/// from the chip unique ID.
pub const STRINGS: [&str; 7] = ["", "diyuac2", "diyuac2", "", "diyuac2 Audio", "diyuac2 Control", "diyuac2 DFU"];

/// DFU functional descriptor (runtime and DFU mode alike): will detach,
/// can download; 1 s detach timeout, 4 KiB transfers, DFU 1.1.
pub const DFU_ATTRIBUTES: u8 = 0x09;
pub const DFU_TRANSFER_SIZE: u16 = 4096;

#[rustfmt::skip]
pub const DEVICE: [u8; 18] = [
    18, 0x01,
    0x00, 0x02,             // bcdUSB 2.00
    0xEF, 0x02, 0x01,       // Misc / Common / IAD
    EP0_SIZE as u8,
    USB_VID as u8, (USB_VID >> 8) as u8,
    USB_PID as u8, (USB_PID >> 8) as u8,
    0x01, 0x00,             // bcdDevice
    STR_MANUFACTURER, STR_PRODUCT, STR_SERIAL,
    1,                      // bNumConfigurations
];

#[rustfmt::skip]
pub const DEVICE_QUALIFIER: [u8; 10] = [
    10, 0x06, 0x00, 0x02, 0xEF, 0x02, 0x01, EP0_SIZE as u8, 1, 0,
];

/// Full-speed configuration: the vendor and DFU interfaces only. Their
/// interface numbers stay 3 and 4, so host tools address them the same way
/// at either speed.
#[rustfmt::skip]
pub const FS_CONFIG: [u8; 36] = [
    9, 0x02, 36, 0, 2, CONFIG_VALUE, 0, 0xC0, 0,
    9, 0x04, ITF_VENDOR, 0, 0, 0xFF, 0x00, 0x00, 5,
    9, 0x04, ITF_DFU, 0, 0, 0xFE, 0x01, 0x01, 6,
    9, 0x21, DFU_ATTRIBUTES, 0xE8, 0x03, DFU_TRANSFER_SIZE as u8, (DFU_TRANSFER_SIZE >> 8) as u8, 0x10, 0x01,
];

pub const CONFIG_VALUE: u8 = 1;

const CONFIG_LEN: u16 = 281;
const EP_SIZE_LO: u8 = EP_SIZE as u8;
const EP_SIZE_HI: u8 = (EP_SIZE >> 8) as u8;

#[rustfmt::skip]
pub const CONFIG: [u8; CONFIG_LEN as usize] = [
    // Configuration: 281 bytes, 5 interfaces, value 1, self powered, 0 mA
    9, 0x02, CONFIG_LEN as u8, (CONFIG_LEN >> 8) as u8, 5, CONFIG_VALUE, 0, 0xC0, 0,
    // IAD: interfaces 0..2, audio, IP version 2.0
    8, 0x0B, ITF_AC, 3, 0x01, 0x00, 0x20, 0,
    // Interface 0: Audio Control, string 4
    9, 0x04, ITF_AC, 0, 0, 0x01, 0x01, 0x20, 4,
    // CS AC header: UAC 2.00, I/O box, wTotalLength 111
    9, 0x24, 0x01, 0x00, 0x02, 0x08, 111, 0, 0x00,
    // Clock source 0x04: internal programmable, frequency r/w, validity r
    8, 0x24, 0x0A, ENTITY_CLOCK, 0x03, 0x07, 0x00, 0x00,
    // Input terminal 0x01: USB streaming, clock 0x04, 2ch FL FR
    17, 0x24, 0x02, 0x01, 0x01, 0x01, 0x00, ENTITY_CLOCK, 2, 0x03, 0, 0, 0, 0, 0x00, 0x00, 0,
    // Feature unit 0x02: source 0x01, master mute + volume r/w, none per channel
    18, 0x24, 0x06, ENTITY_FU_PLAY, 0x01, 0x0F, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // Output terminal 0x03: line connector, source FU 0x02, clock 0x04
    12, 0x24, 0x03, 0x03, 0x03, 0x06, 0x00, ENTITY_FU_PLAY, ENTITY_CLOCK, 0x00, 0x00, 0,
    // Input terminal 0x11: line connector, clock 0x04, 2ch FL FR
    17, 0x24, 0x02, 0x11, 0x03, 0x06, 0x00, ENTITY_CLOCK, 2, 0x03, 0, 0, 0, 0, 0x00, 0x00, 0,
    // Feature unit 0x12: source 0x11, master mute + volume r/w, none per channel
    18, 0x24, 0x06, ENTITY_FU_REC, 0x11, 0x0F, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // Output terminal 0x13: USB streaming, source FU 0x12, clock 0x04
    12, 0x24, 0x03, 0x13, 0x01, 0x01, 0x00, ENTITY_FU_REC, ENTITY_CLOCK, 0x00, 0x00, 0,

    // Interface 1 alt 0: playback, zero bandwidth
    9, 0x04, ITF_AS_OUT, 0, 0, 0x01, 0x02, 0x20, 0,
    // Interface 1 alt 1: playback, 2 endpoints
    9, 0x04, ITF_AS_OUT, 1, 2, 0x01, 0x02, 0x20, 0,
    // CS AS general: terminal 0x01, type I PCM, 2ch FL FR
    16, 0x24, 0x01, 0x01, 0x00, 0x01, 0x01, 0, 0, 0, 2, 0x03, 0, 0, 0, 0,
    // Type I format: 4-byte slots, 24 valid bits
    6, 0x24, 0x02, 0x01, BYTES_PER_SAMPLE as u8, 24,
    // EP 0x01 OUT iso async data, 200 bytes, every microframe
    7, 0x05, 0x01, 0x05, EP_SIZE_LO, EP_SIZE_HI, 1,
    // CS iso data EP: lock delay 1 ms
    8, 0x25, 0x01, 0x00, 0x00, 0x01, 0x01, 0x00,
    // EP 0x81 IN iso feedback, 4 bytes, every 2^(4-1) = 8 microframes
    7, 0x05, 0x81, 0x11, FB_SIZE as u8, 0, 4,

    // Interface 2 alt 0: capture, zero bandwidth
    9, 0x04, ITF_AS_IN, 0, 0, 0x01, 0x02, 0x20, 0,
    // Interface 2 alt 1: capture, 1 endpoint
    9, 0x04, ITF_AS_IN, 1, 1, 0x01, 0x02, 0x20, 0,
    // CS AS general: terminal 0x13, type I PCM, 2ch FL FR
    16, 0x24, 0x01, 0x13, 0x00, 0x01, 0x01, 0, 0, 0, 2, 0x03, 0, 0, 0, 0,
    // Type I format: 4-byte slots, 24 valid bits
    6, 0x24, 0x02, 0x01, BYTES_PER_SAMPLE as u8, 24,
    // EP 0x82 IN iso async data, 200 bytes, every microframe
    7, 0x05, 0x82, 0x05, EP_SIZE_LO, EP_SIZE_HI, 1,
    // CS iso data EP
    8, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,

    // Interface 3: vendor control, string 5
    9, 0x04, ITF_VENDOR, 0, 0, 0xFF, 0x00, 0x00, 5,

    // Interface 4: DFU runtime, string 6
    9, 0x04, ITF_DFU, 0, 0, 0xFE, 0x01, 0x01, 6,
    // DFU functional: will detach + can download, detach timeout 1000 ms,
    // transfer size 4096, DFU 1.1
    9, 0x21, DFU_ATTRIBUTES, 0xE8, 0x03, DFU_TRANSFER_SIZE as u8, (DFU_TRANSFER_SIZE >> 8) as u8, 0x10, 0x01,
];

const _: () = assert!(CONFIG[2] as usize | (CONFIG[3] as usize) << 8 == CONFIG.len());
const _: () = assert!(FS_CONFIG[2] as usize == FS_CONFIG.len());
