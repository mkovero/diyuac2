# diyuac2

Class-compliant USB Audio Class 2.0 frontend firmware, bring-up on the NXP
MIMXRT1020-EVK. Digital core for the ac2 measurement frontend. See
[handover.md](handover.md) for goals and milestones.

## Status

- M1 (TinyUSB `uac2_speaker_fb`): enumerates at 480M, async with explicit
  feedback EP, 120 s `aplay` at 48 kHz without xruns.
- M2 (`firmware/`, own descriptors, `cafe:0ac2`): UAC2 2 in / 2 out, S32_LE
  with 24 valid bits, 48/96/192 kHz, async + explicit feedback, vendor
  control interface ([protocol](docs/vendor-protocol.md)). No audio hardware
  yet: playback is looped back to capture, paced by SysTick. Tested on Linux
  only (Windows/macOS not tested, not a current target):
  - bit-exact loopback at all rates, no xruns, no slips
    (`tools/loopback_test.py`), except: the first 1-4 microframes of a
    playback stream are lost at stream start (iso OUT endpoint is primed
    only after SET_INTERFACE is processed in task context)
  - feedback mean within +1.7 / +1.2 / +8.1 Hz of 48 / 96 / 192 kHz
    (FIFO-count placeholder; replaced by measured SAI timing in M3)
  - vendor control selftest passes (`tools/vendorctl.py selftest`)

- M3 (SAI1 + eDMA + WM8960, MCU is I2S master, MCLK 12.288 MHz from the
  audio PLL): playback to the J11 headphone output and capture from the
  codec inputs, measured feedback ([docs/feedback.md](docs/feedback.md)).
  - **48 kHz only**: the WM8960 is specified up to 48 kHz. The device takes
    its rate list from the codec driver, so 96 and 192 kHz return with the
    CS4270 (M5).
  - 60 s of playback and capture: 0 underruns, overruns or SAI FIFO errors.
    Feedback mean 48001.30 Hz, stdev 0.27 Hz. Playback lead stable at
    103..113 frames.
  - Capture: onboard mic P1 found on the **right** channel (RINPUT1/2
    differential) by a routing scan with music vs quiet (+29 dB boost:
    >300 Hz level -68 -> -57 dBFS). Left inputs showed no signal (probably
    the J11 headset mic). Default mic boost is 0 dB, so P1 is quiet. A real
    analog loopback through J11 is still untested.
  - Do not connect J12/J13 (class-D BTL speaker outputs) to an audio
    interface input; use J11 for loopback.
  - Multi-hour soak: in progress.
  - Debug: `tools/vendorctl.py codec-write <reg> <val>` writes WM8960
    registers live (vendor request 0x10, not part of the stable protocol).
  - The headphone path is at 0 dB (DAC and HP amp). Test signals are loud.

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
cd firmware
make BOARD=mimxrt1020_evk get-deps     # one-time: NXP MCUX SDK, CMSIS, etc.
make BOARD=mimxrt1020_evk -j
```

Output: `firmware/_build/mimxrt1020_evk/firmware.{elf,hex,bin}`

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
pyocd load -t mimxrt1020 firmware/_build/mimxrt1020_evk/firmware.hex
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

Audio path diagnostics (see [vendor protocol](docs/vendor-protocol.md)):

```sh
sudo tools/vendorctl.py clear-stats
sudo tools/vendorctl.py stats 60     # underruns, FIFO errors, feedback, playback lead
```

Feedback at high speed is 16.16 samples per microframe, little-endian:
`09000600` = 0x00060009 = 6.000137 samples/uframe = 48001.1 Hz.
