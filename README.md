# diyuac2

Class-compliant USB Audio Class 2.0 frontend firmware, bring-up on the NXP
MIMXRT1020-EVK. Digital core for the ac2 measurement frontend. See
[handover.md](handover.md) for goals and milestones.

## Status

M1 (TinyUSB `uac2_speaker_fb`, high speed): enumerates at 480M, async with
explicit feedback EP, 120 s `aplay` at 48 kHz without xruns. Feedback is
FIFO-count based in the stock example (no real audio clock yet), so the
long soak is deferred to M3 when SAI provides real timing.

## Toolchain

- Arm GNU Toolchain 14.3.rel1 (`arm-none-eabi-gcc`)
- pyOCD >= 0.46 (built-in `mimxrt1020` target with QSPI flash algorithm;
  LinkServer not required)

```sh
export PATH="$HOME/.local/opt/arm-gnu-toolchain-14.3.rel1-x86_64-arm-none-eabi/bin:$PATH"
```

## Build

```sh
git submodule update --init            # TinyUSB, pinned at 0.21.0
cd lib/tinyusb/examples/device/uac2_speaker_fb
make BOARD=mimxrt1020_evk get-deps     # NXP MCUX SDK, CMSIS, etc.
make BOARD=mimxrt1020_evk -j
```

Output: `_build/mimxrt1020_evk/uac2_speaker_fb.{elf,hex,bin}`

## Board setup (MIMXRT1020-EVK)

| Item | Setting | Notes |
|---|---|---|
| Power | J23 (debug USB), J1 on 5-6 | J2 DC jack: J1 on 1-2 |
| Boot | SW8 = OFF OFF ON OFF | QSPI boot. Serial downloader: SW8-3 OFF, SW8-4 ON |
| Debug/flash | J23 (DAPLink, `0d28:0204`) | |
| Target USB (HS) | J9 (USB OTG1) | Repaired on this unit: D+/D- wired to external connector |
| Codec | WM8960 on LPI2C1, headphone J11 | Per EVK user guide and NXP SDK |

## Flash

```sh
pyocd load -t mimxrt1020 _build/mimxrt1020_evk/uac2_speaker_fb.hex
```

Alternatively copy the `.hex` to the DAPLink `RT1020-EVK` drive.

**After flashing, power-cycle the board** (unplug J23). A soft or debug reset
leaves the QSPI flash in the mode the flash algorithm put it in; the boot ROM
then fails to read the boot header and falls back to the serial downloader
(enumerates on J9 as `1fc9:0130 SE Blank RT Family`).

Debug-port memory reads of 0x60000000 while the ROM is in serial downloader
mode return data shifted by one byte per 1 KB block (stale FlexSPI config);
do not use them to judge flash contents.

## Test (Linux host)

```sh
lsusb -t                                   # J9 device must be 480M
cat /proc/asound/cardN/stream0             # formats, ASYNC, sync EP 0x81
aplay -D hw:N,0 -f S16_LE -c 2 -r 48000 -d 120 /dev/zero   # watch for "underrun"
sudo modprobe usbmon
sudo cat /sys/kernel/debug/usb/usbmon/<bus>u | grep ' C Zi:<bus>:<dev>:1 '
```

Feedback at high speed is 16.16 samples per microframe, little-endian:
`09000600` = 0x00060009 = 6.000137 samples/uframe = 48001.1 Hz.
