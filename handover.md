# HANDOVER: UAC2 sound card bring-up on MIMXRT1020-EVK

## Who / what this is for

Owner: Markus (audio pro, electronics tech, Linux sysadmin, embedded dev).
This firmware is the first step toward a **workbench measurement frontend** for
**ac2**, his Rust audio measurement workspace (github.com/mkovero/ac).

Long-term target: a QuantAsylum-class USB audio analyser frontend with generator
outputs, power-amp-safe inputs, level ranges, and protection. The device **must be
a class-compliant USB Audio Class 2.0 device**, usable from any software
(REW, Audacity, DAWs), not only ac2. Range and protection control go over a
**separate vendor interface** that ac2 opens in addition to the audio.

This repo is the **digital core bring-up**: USB + clocking + SAI + codec, proven
on an eval board before any custom analog hardware exists.

## Hardware on the desk

- **NXP MIMXRT1020-EVK**: i.MX RT1021, Cortex-M7 @ 500 MHz, 256 Mbit SDRAM,
  64 Mbit QSPI flash (XIP), high-speed USB OTG with integrated PHY, onboard
  audio codec with headphone/mic jack, onboard OpenSDA/DAPLink debug probe.
  - **Verify the codec part from the EVK schematic** (believed to be a WM8960).
- Later, not now: Markus's own **Teensy 4.1 + CS4270** audio board
  (Teensy socket, CS4270 on SAI1 + I2C, 2 in / 2 out on 1/4" jacks, 4 CV inputs,
  4 pots, 4 buttons, 4 addressable RGB LEDs). Same SAI IP family as the RT1020,
  so code should port with pin and clock changes only.
- Final MCU for the custom board is **not decided** (STM32 with HS PHY vs.
  i.MX RT). Keep hardware-specific code behind a thin layer.

## Toolchain (Linux)

- `arm-none-eabi-gcc`, make or CMake.
- **TinyUSB** (has an `mimxrt1020_evk` board definition; `make get-deps`
  pulls NXP MCUX SDK drivers incl. SAI and codec drivers).
- Flash/debug via the **onboard probe, no external J-Link needed**:
  1. Default: stock DAPLink + **NXP LinkServer** (handles the external QSPI
     flash; pyOCD currently does not).
  2. Optional: reflash the onboard probe with Segger's free OpenSDA J-Link
     firmware for smoother GDB/VS Code and RTT logging. With J-Link firmware the
     board must be powered from a source other than the debug USB.
- Optional: MCUXpresso Config Tools for pin mux and clock tree (audio PLL),
  NXP SDK USB audio examples as reference for SAI/codec/feedback handling.
- C first. Rust (imxrt-rs) is fine later for non-USB-audio parts, but TinyUSB's
  UAC2 is the mature path.

## Milestones and acceptance criteria

### M1: USB-only UAC2, high speed
- Build TinyUSB `uac2_speaker_fb` (or `uac2_headset`) for `mimxrt1020_evk`
  on the **high-speed** USB port.
- Accept when: `lsusb -t` shows 480M; `/proc/asound/cardN/stream0` lists
  formats/rates and the feedback endpoint; `aplay` of a long file runs with no
  xruns; feedback values look stable in Wireshark + usbmon.

### M2: Own descriptor set (before touching audio hardware)
- 2 in / 2 out, 24-bit samples in 32-bit slots, 48/96/192 kHz on one clock
  source, **asynchronous** with explicit feedback endpoint.
- Composite: UAC2 + **vendor interface** (bulk or control-based) for control.
  Stub commands are fine for now.
- Accept when: enumerates and plays/records on **Linux, macOS and Windows**
  (Windows is strictest on composite UAC2 / IAD layout; test it early).

### M3: Real audio through SAI + onboard codec
- SAI with DMA, codec init over I2C, audio clock from the i.MX audio PLL.
- **Device is the clock master** (async mode): the feedback value must come
  from measured hardware counts (SAI frame rate vs. USB SOF), not from an
  estimate. Investigate the cleanest RT1020 mechanism for this (timer capture,
  USB frame index, SAI/DMA counters) and document the choice.
- Accept when: playback and capture through the codec jack work at all rates,
  multi-hour soak with no xruns, no sample slips, feedback stays converged.

### M4: Vendor control interface, real
- Commands for: range select, mute, generator enable, input-type/fault status,
  read/write calibration constants, firmware version, serial number.
- Define it as a small documented protocol (versioned) that ac2 will implement.
- Generic software must not need this interface to use the audio.

### M5 (later): Port to the CS4270 board / custom hardware
- Either via a Teensy 4.1 or by wiring the EVK to the board's Teensy socket.
- Keep codec drivers pluggable (onboard codec now, CS4270 next, higher-grade
  ADC/DAC later, e.g. PCM4222-class ADC, AKM/ESS-class DAC).

## Known pitfalls to handle from day one

- **D-cache vs DMA.** SAI and USB DMA buffers in cached RAM cause intermittent
  clicks or stale audio that look like clock bugs. Use a non-cacheable region
  (MPU) or explicit clean/invalidate around every transfer. Check in the RT1020
  reference manual which RAM regions each DMA master can reach (DTCM/OCRAM).
- **Clocking quality matters later.** The fractional audio PLL is fine for
  bring-up. Later hardware will likely use dedicated audio oscillators, and
  possibly lock MCLK to an external 10 MHz reference via a PLL + jitter cleaner.
  Do not hard-code assumptions that the MCU generates MCLK.
- **Full-speed fallback is a bug** here. Always confirm 480M.

## Testing (Linux host)

- `lsusb -v`, `lsusb -t`, `/proc/asound/cardN/stream0`
- `aplay` / `arecord` long runs; ALSA xrun reporting
- Wireshark + usbmon for feedback endpoint values
- Loopback measurements with ac2 once audio works (owner will run these)

## Working style

- Small, verifiable steps; report what was measured, not just what was built.
- Mark anything unverified (pin numbers, register settings, part numbers) as
  such and check against the EVK schematic / reference manual.
- Ask Markus before major architecture decisions (final MCU, protocol design).
