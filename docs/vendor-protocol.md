# diyuac2 vendor control protocol

Status: **draft v0.1, transport only.** The command set (range, mute,
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

## Commands (v0.1)

| bRequest | Name | Dir | wLength | Data |
|---|---|---|---|---|
| 0x00 | GET_INFO | IN | 8 | `vendor_info_t` |
| 0x01 | GET_SCRATCH | IN | 32 | scratch buffer |
| 0x02 | SET_SCRATCH | OUT | 1..32 | new scratch contents (rest zero-filled) |

`vendor_info_t` (8 bytes):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | protocol major |
| 1 | 1 | protocol minor |
| 2 | 1 | firmware major |
| 3 | 1 | firmware minor |
| 4 | 1 | firmware patch |
| 5 | 1 | reserved (0) |
| 6 | 2 | flags; bit 0 = stub firmware |

SCRATCH is a transport test (round trip of an OUT and IN data stage), not
part of the final protocol.

## Versioning

Hosts read GET_INFO first. A protocol major change means incompatible;
minor additions are backward compatible (new commands only).

## Reference client

`tools/vendorctl.py` (no dependencies, Linux usbfs): `info`, `scratch-read`,
`scratch-write <hex>`, `selftest`.
