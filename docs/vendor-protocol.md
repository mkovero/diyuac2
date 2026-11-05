# diyuac2 vendor control protocol

Status: **draft v0.3, transport + diagnostics.** The command set (range, mute,
generator, input type/fault status, calibration, serial) is designed in M4.
Only the transport and a version query exist now.

## Transport

- Interface: `bInterfaceClass = 0xFF`, interface number 3, no endpoints,
  string "diyuac2 Control". It is outside the audio IAD, so class-compliant
  audio software never sees it.
- All commands are **EP0 control transfers**:
  - `bmRequestType`: vendor, recipient interface
    (`0xC1` device-to-host, `0x41` host-to-device)
  - `wIndex`: `3` (vendor interface number)
  - `bRequest`: command code
  - `wValue`: command argument (0 unless stated)
- Unknown commands, wrong direction, wrong recipient or oversize data:
  **STALL** (libusb `LIBUSB_ERROR_PIPE`, usbfs `EPIPE`).
- Multi-byte fields are little-endian.
- The interface carries no kernel driver on Linux. To access it as a normal
  user, add a udev rule, e.g. `/etc/udev/rules.d/70-diyuac2.rules`:

  ```
  SUBSYSTEM=="usb", ATTRS{idVendor}=="cafe", ATTRS{idProduct}=="0ac2", MODE="0660", TAG+="uaccess"
  ```

## Commands (v0.3)

| bRequest | Name | Dir | wLength | Data |
|---|---|---|---|---|
| 0x00 | GET_INFO | IN | 8 | `vendor_info_t` |
| 0x01 | GET_SCRATCH | IN | 32 | scratch buffer |
| 0x02 | SET_SCRATCH | OUT | 1..32 | new scratch contents (rest zero-filled) |
| 0x03 | GET_STATS | IN | 60 | `vendor_stats_t` (v0.2: 40 bytes; v0.3: 60, USB error counters added) |
| 0x04 | CLEAR_STATS | OUT | 0 | zero counters, reset lead min/max (v0.2) |

`vendor_info_t` (8 bytes):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | protocol major |
| 1 | 1 | protocol minor |
| 2 | 1 | firmware major |
| 3 | 1 | firmware minor |
| 4 | 1 | firmware patch |
| 5 | 1 | reserved (0) |
| 6 | 2 | flags; bit 0 = stub firmware, bit 1 = full-speed link (the device is high speed only; at full speed it offers only the vendor interface, no audio) |

SCRATCH is a transport test (round trip of an OUT and IN data stage), not
part of the final protocol.

Debug requests (0x10 and up) are not part of the protocol and may change:

| bRequest | Name | Dir | wLength | Data |
|---|---|---|---|---|
| 0x10 | DBG_CODEC_WRITE | OUT | 0 | `wValue = reg << 9 \| value` (WM8960: 7-bit register, 9-bit value) |
| 0x11 | DBG_ISR_STATS | IN | 40 | USB ISR cycle profile, Rust firmware only (below); CLEAR_STATS resets it |
| 0x12 | DBG_LOOPBACK | OUT | 0 | Rust firmware only: `wValue` 1 = capture returns the playback data as the SAI sent it (TX ring, SAI-clocked), 0 = capture from the codec |

DBG_ISR_STATS data (little endian, cycles of the core clock):

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | ISR entries |
| 4 | 4 | longest ISR |
| 8 | 4 | longest SOF section (feedback, TX ring zeroing) |
| 12 | 4 | longest transfer-completion section (EP0 and iso rings) |
| 16 | 4 | longest SETUP section (includes codec I2C writes) |
| 20 | 8 | cycles spent in the ISR |
| 28 | 8 | cycles elapsed since the profile was cleared |
| 36 | 4 | core clock, Hz |

`vendor_stats_t` (60 bytes, diagnostics for soak tests; 40 bytes, up to
offset 38, before Rust firmware 0.4.1. The device returns at most wLength
bytes, so a 40-byte request still works):

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | current sample rate, Hz |
| 4 | 4 | last feedback value, 16.16 samples per microframe (Hz = value * 8000 / 65536) |
| 8 | 4 | playback underruns (ring ran dry inside a stream) |
| 12 | 4 | capture overruns (RX ring overrun before data was queued) |
| 16 | 4 | capture frames dropped (USB IN FIFO full) |
| 20 | 4 | SAI TX FIFO errors |
| 24 | 4 | SAI RX FIFO errors |
| 28 | 4 | playback lead minimum, frames (int32) |
| 32 | 4 | playback lead maximum, frames (int32) |
| 36 | 2 | flags: bit 0 codec OK, 1 playback active, 2 capture active, 3 playback primed, 4 rate outside the codec's specification (Rust firmware: 88.2 to 192 kHz on the WM8960), 5 debug loopback on, 6 a Feature Unit is muted or not at 0 dB (the path is not bit-exact; Rust firmware 0.3.3+), 7 streaming in the 16-channel TDM format (alt setting 2; Rust firmware 0.4.1+) |
| 38 | 2 | reserved (0) |
| 40 | 4 | USB OUT data dTDs completed with an error status (halted, data buffer or transaction error) |
| 44 | 4 | USB IN capture dTDs completed with an error status |
| 48 | 4 | USB IN feedback dTDs completed with an error status |
| 52 | 4 | microframes with no OUT data packet from the host (missed iso slots: data the host never sent) |
| 56 | 4 | microframes in which the host did not poll the capture endpoint |

Playback lead is how far the firmware has written into the TX ring ahead of
the DMA. A stable range during a long run means the feedback matches the
audio clock. The range is reset at each playback stream start.

The missed-microframe counters (52, 56) compare, per stream, the
microframes elapsed (FRINDEX) with the dTDs retired on the endpoint
(bInterval 1). The difference moves within a small jitter band; each step
above the highest value seen so far counts as a missed microframe.
Counting starts at the stream's first completed packet. CLEAR_STATS
resets all counters.

## Versioning

Hosts read GET_INFO first. A protocol major change means incompatible;
minor additions are backward compatible (new commands only).

## Reference client

`tools/vendorctl.py` (no dependencies, Linux usbfs): `info`, `scratch-read`,
`scratch-write <hex>`, `selftest`, `stats [interval_s]`, `clear-stats`.
