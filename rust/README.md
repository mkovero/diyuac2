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
| R3 | SAI1 + eDMA + WM8960, measured feedback; parity with M3 | done |

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

## R3 results (2026-11-03, Linux host)

R3 is a port of the C M3 audio path. SAI1 is the I2S master, clocked from the
audio PLL (PLL4 at 786.432 MHz, then ÷4, ÷16 → MCLK 12.288 MHz). eDMA
channels 0 (TX) and 1 (RX) loop over 2048-frame rings. The WM8960 is set up
over LPI2C1 with the same register sequence as the C firmware. SAI, CCM and
DMA register values match a dump from the running C firmware.

Everything runs in the USB ISR, with no task loop:
- each playback packet is written straight into the TX ring, about 2 ms
  ahead of the DMA. Frames the DMA has fetched are zeroed every microframe,
  so a starved ring plays silence;
- capture packets are filled straight from the RX ring;
- feedback is measured at SOF from the words the SAI has shifted out (DMA
  position minus FIFO fill). It is a port of `feedback.c`.

Results:
- Playback and capture together, 30 s: 0 underruns, 0 capture overruns,
  0 SAI FIFO errors. The playback lead stays at 117-119 frames. The
  capture has noise from the onboard mic on the right channel only
  (-67 dB RMS, against -88 dB on the left), the same as the C firmware.
- Feedback from usbmon over 20 s (20,000 packets): mean 48001.286 Hz,
  standard deviation 0.235 Hz, range 48000.977-48001.465. The C firmware
  measured 48001.3 Hz with a standard deviation of 0.27.
- 10 start/stop cycles of 1 s each: 0 underruns.
- DBG_CODEC_WRITE works. Registers above 0x37 stall.
- GET_INFO reports firmware 0.3.2. The stats flags show codec_ok, play,
  rec and primed.
- 5-minute playback and capture run: 0 underruns, 0 overruns, 0 FIFO
  errors, lead 117-119 frames, no ALSA errors.
- 2 h soak (`/tmp/soak.sh` on the host, stats every 60 s): 0 underruns,
  0 overruns, 0 FIFO errors in all 120 samples, lead 116-119 frames.
  Feedback samples read 48000.977 or 48001.465 Hz, except the first, which
  was still warming up. Capture ran the full 2 h. Playback stopped
  cleanly after 93 minutes: the test pipes a WAV through aplay, and
  aplay stops at the 2 GiB WAV size limit (5592 s at 384 kB/s). That is
  a limit of the test, not the firmware.

LED: blinks every 100 ms if the codec did not answer at boot.

A bug found along the way: `IsoRing` holds its dTDs inline. Moving a
started ring (`mem::replace` around the service call) handed the
controller stack addresses. Capture then sent only zero-length packets.
The data path state now lives in its own struct, so the service closures
borrow disjoint fields instead.
