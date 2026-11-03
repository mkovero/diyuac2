//! diyuac2 USB descriptors. Byte-identical to the C firmware
//! (`firmware/app/usb_descriptors.{c,h}`); verified against its built image.
//!
//! ```text
//! IAD (itf 0..2) UAC2 function
//!   itf 0  Audio Control
//!            Clock Source 0x04 (internal, programmable)
//!            Playback: IT 0x01 (USB streaming, 2ch) -> OT 0x03 (line out)
//!            Capture:  IT 0x11 (line in, 2ch)       -> OT 0x13 (USB streaming)
//!   itf 1  Audio Streaming OUT: alt 0 idle, alt 1 2ch 24-in-32,
//!          iso OUT EP 0x01 (async) + explicit feedback iso IN EP 0x81
//!   itf 2  Audio Streaming IN: alt 0 idle, alt 1 2ch 24-in-32, iso IN EP 0x82 (async)
//! itf 3  Vendor control interface (class 0xFF, no endpoints), see
//!        docs/vendor-protocol.md
//! ```
//!
//! The device is high speed only. On a full-speed link the configuration
//! has the vendor interface alone ([`FS_CONFIG`]), so no audio card appears
//! and host tools can still read GET_INFO, which flags the link.
//!
//! No Feature Unit on purpose: host-side volume/mute must not alter the
//! signal path of a measurement device.

/// Development IDs. 0xCAFE is TinyUSB's test VID: bench use only.
pub const USB_VID: u16 = 0xCAFE;
pub const USB_PID: u16 = 0x0AC2;

pub const EP0_SIZE: u16 = 64;

pub const ITF_AC: u8 = 0;
pub const ITF_AS_OUT: u8 = 1;
pub const ITF_AS_IN: u8 = 2;
pub const ITF_VENDOR: u8 = 3;

pub const ENTITY_CLOCK: u8 = 0x04;

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

/// String descriptors 1..5 except the serial (index 3), which is built
/// from the chip unique ID.
pub const STRINGS: [&str; 6] = ["", "diyuac2", "diyuac2", "", "diyuac2 Audio", "diyuac2 Control"];

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

/// Full-speed configuration: the vendor interface only. Its interface
/// number stays 3, so host tools address it the same way at either speed.
#[rustfmt::skip]
pub const FS_CONFIG: [u8; 18] = [
    9, 0x02, 18, 0, 1, CONFIG_VALUE, 0, 0xC0, 0,
    9, 0x04, ITF_VENDOR, 0, 0, 0xFF, 0x00, 0x00, 5,
];

pub const CONFIG_VALUE: u8 = 1;

const EP_SIZE_LO: u8 = EP_SIZE as u8;
const EP_SIZE_HI: u8 = (EP_SIZE >> 8) as u8;

#[rustfmt::skip]
pub const CONFIG: [u8; 227] = [
    // Configuration: 227 bytes, 4 interfaces, value 1, self powered, 0 mA
    9, 0x02, 227, 0, 4, CONFIG_VALUE, 0, 0xC0, 0,
    // IAD: interfaces 0..2, audio, IP version 2.0
    8, 0x0B, ITF_AC, 3, 0x01, 0x00, 0x20, 0,
    // Interface 0: Audio Control, string 4
    9, 0x04, ITF_AC, 0, 0, 0x01, 0x01, 0x20, 4,
    // CS AC header: UAC 2.00, I/O box, wTotalLength 75
    9, 0x24, 0x01, 0x00, 0x02, 0x08, 75, 0, 0x00,
    // Clock source 0x04: internal programmable, frequency r/w, validity r
    8, 0x24, 0x0A, ENTITY_CLOCK, 0x03, 0x07, 0x00, 0x00,
    // Input terminal 0x01: USB streaming, clock 0x04, 2ch FL FR
    17, 0x24, 0x02, 0x01, 0x01, 0x01, 0x00, ENTITY_CLOCK, 2, 0x03, 0, 0, 0, 0, 0x00, 0x00, 0,
    // Output terminal 0x03: line connector, source 0x01, clock 0x04
    12, 0x24, 0x03, 0x03, 0x03, 0x06, 0x00, 0x01, ENTITY_CLOCK, 0x00, 0x00, 0,
    // Input terminal 0x11: line connector, clock 0x04, 2ch FL FR
    17, 0x24, 0x02, 0x11, 0x03, 0x06, 0x00, ENTITY_CLOCK, 2, 0x03, 0, 0, 0, 0, 0x00, 0x00, 0,
    // Output terminal 0x13: USB streaming, source 0x11, clock 0x04
    12, 0x24, 0x03, 0x13, 0x01, 0x01, 0x00, 0x11, ENTITY_CLOCK, 0x00, 0x00, 0,

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
];
