# diyuac2 firmware, Rust port

Full-Rust firmware for the MIMXRT1020-EVK, built on imxrt-rt, imxrt-ral and
imxrt-hal. The USB device stack is our own (`src/usb/`): a ChipIdea HS
driver with isochronous dTD rings, EP0 control transfers and standard
requests, all run from the USB_OTG1 interrupt. usb-device and imxrt-usbd were
used for R1 and then dropped, because they cannot do high-speed iso (no QH
Mult, a single dTD per endpoint, no SOF hook). The C firmware in `../firmware` stays
the verified reference; this port has to reach parity with it.

| Step | Scope | Status |
|---|---|---|
| R1 | Boot, 500 MHz clocks, USB HS enumeration with the C descriptors, clock source and vendor requests | done |
| R2 | Own USB stack: iso OUT/IN at high speed + feedback EP, bit-exact loopback | done |
| R3 | SAI1 + eDMA + WM8960, measured feedback, soak; parity with M3 | |

## Build and flash

```sh
rustup target add thumbv7em-none-eabihf
cargo build --release
arm-none-eabi-objcopy -O ihex target/thumbv7em-none-eabihf/release/diyuac2 target/diyuac2.hex
pyocd load -t mimxrt1020 -f 500000 -M under-reset target/diyuac2.hex
pyocd reset -t mimxrt1020 -f 500000 -m hw    # hardware reset; no power cycle needed
```

`.data` and `.bss` are placed in DTCM (`build.rs`), so the USB queue heads,
dTDs and buffers are not cached and the USB DMA can reach them.

The FlexSPI configuration block (`src/fcb.rs`) is a copy of the one the C
firmware boots with.

## R1 results (2026-11-03, Linux host)

- Enumerates at 480M. The configuration descriptor is byte-identical to the
  C firmware's (227 bytes), and so is the device descriptor.
- The serial number is the chip unique ID, in the same format as the C
  firmware.
- snd-usb-audio binds and reads the rate range from the clock source
  (48 kHz).
- Vendor protocol 0.2: GET_INFO reports firmware 0.3.0 with the stub flag.
  The scratch round trip works. GET_STATS returns the rate and the nominal
  feedback with zero counters. CLEAR_STATS is accepted. DBG_CODEC_WRITE
  stalls, because there is no codec driver yet.
- aplay can open the device and switch alt settings. arecord failed with
  EIO, because R1 sends no iso data.

## R2 results (2026-11-03, Linux host, `tools/loopback.py`)

R2 has no audio hardware. Playback (EP 0x01) is looped back into capture
(EP 0x82). The device clock is the host's microframe clock: capture sends
exactly rate/8000 frames per microframe, and the feedback is the nominal
value (6.0 at 48 kHz). Loopback starts once 2 ms of playback data is
buffered.

- Random 32-bit stereo data played and recorded at the same time comes back
  bit-exact:
  - 5 s run;
  - 10 runs of 1 s each (start/stop cycles);
  - 60 s run, 2,880,000 frames.
- The first playback packet is not lost. The C/TinyUSB firmware drops the
  first 1-4 microframes.
- usbmon:
  - capture packets are 48 bytes every microframe;
  - playback packets are 48 bytes;
  - feedback is `00 00 06 00` every 8 microframes.
- Vendor stats over 60 s: 0 underruns, 0 overruns, 0 iso dTD errors; FIFO
  lead 90 frames while running. The minimum of 0 is the drain at stream end.
- Playback only and capture only both work.
- A bus reset (`usbreset`) re-enumerates at 480M, and loopback passes again.
- GET_INFO reports firmware 0.3.1.

Not handled or not tested yet:
- suspend/resume (the SLI interrupt is only acknowledged);
- a full-speed link (the LED blinks every 50 ms; not provoked);
- endpoint halt features (accepted, no-op).

LED: blinks every 250 ms until the host configures the device, then every
1 s; every 50 ms if the link is full speed.
