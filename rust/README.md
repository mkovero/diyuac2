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

## Interrupt budget and fault injection (2026-11-03)

`vendorctl.py isr` reads the USB ISR cycle profile (debug request 0x11,
DWT cycle counter at 500 MHz). Numbers at 48 kHz:

| Case | ISR mean | ISR max | Load |
|---|---|---|---|
| idle (SOF only) | 0.95 us | 2.34 us | 0.65 % |
| playback + capture | 1.77 us | 4.34 us (completions 3.97) | 2.47 % |
| ... + 200 codec writes | | 285 us (setup 282) | 4.5 % |

The microframe is 125 us, so streaming uses a few percent. A codec write
blocks the ISR for about 0.28 ms; during 200 writes the playback lead fell
from 117 to 106 frames, with 0 underruns. 192 kHz would copy 4 times the
data per packet. That is an estimate, not measured.

`tools/faults.sh` (host). After each case, the device must come back at
480M and run a clean 5 s playback + capture with zero counters:

- reset: usbreset in the middle of a stream. ok
- port: hub port disabled for 1 s, then enabled again, mid-stream. ok
- suspend: runtime autosuspend while idle (the device reached
  `suspended`), then resume by opening the stream. ok
- openclose: 100 short open/close cycles in mixed directions. ok
- order: capture first, then playback first, then overlapping stops. ok,
  0 counters

Not covered: SET_CUR rate changes and endpoint halt while streaming.
usbfs refuses class and endpoint requests to interfaces that snd-usb-audio
holds. snd-usb-audio does not send halt requests to iso endpoints.

Full speed, forced with the `force-full-speed` feature (PFSC). The device
is high speed only. At full speed the configuration holds the vendor
interface alone (still interface 3), so no audio card appears. GET_INFO
sets flag bit 1, which `vendorctl.py info` prints as "FULL SPEED: no
audio". The LED blinks every 50 ms. Linux logs "not running at top speed"
and "config 1 has no interface number 0". At high speed, the other-speed
descriptor returns this vendor-only configuration.

Before this change, the high-speed configuration was also sent at full
speed. snd-usb-audio created the card, playback ran with 999 underruns in
3 s, and capture failed with EIO. The C firmware sends one configuration
at both speeds, so it probably behaves the same way. Not checked.

The fault suite passes again on the high-speed build after this change.

## 48/96/192 kHz and the SAI loopback (2026-11-03)

The clock source offers 48, 96 and 192 kHz. The SAI root clock is
256 fs (49.152 MHz at 192 kHz, from PLL4 / 4 / (768000 / rate)), with
BCLK = 64 fs. The WM8960 is specified only up to 48 kHz. At 96 and
192 kHz it runs out of spec, and the analog I/O is not valid. The stats
flag `codec_out_of_spec` (bit 4) marks this.

Debug request 0x12 (`vendorctl.py loopback on`) makes capture return the
playback data as the SAI sent it. Capture reads the TX ring behind the
DMA, paced by the same SAI frame clock as the RX path. This tests USB in
both directions, the SAI timing and the feedback, bit-exact, without
depending on the codec. Played frames are zeroed 512 frames behind the
DMA, not at once, so the loopback can still read them. A starved ring
still plays silence (see `ZERO_LAG` in `src/audio.rs`).

- `tools/loopback.py 3 10 <rate>`: 10 s bit-exact at 48, 96 and 192 kHz.
  0 counters. The playback lead is 117/219/424 frames, about 2.2 ms.
- 192 kHz, playback and capture: ISR mean 2.15 us, max 3.75 us, load
  4.4 %.
- Feedback at 192 kHz (usbmon, 20,000 packets): mean 192005.06 Hz,
  standard deviation 0.236 Hz. That is +26 ppm, the same offset as at
  48 kHz (48001.3).
- The fault suite passes at 192 kHz (`RATE=192000 tools/faults.sh`).
- `tools/lbsoak.py`: a streaming bit-exact checker for long runs. It
  repeats a 1 s random block and checks every frame. It caught a glitch
  injected by switching the loopback off for a moment.
- Capture packet sizes at 192 kHz vary between 22 and 25 frames per
  microframe (nominal 24). The average follows the SAI clock, with no
  overruns.

### 2 h soak at 192 kHz (2026-11-03)

`tools/lbsoak.py 3 7200 192000` with the loopback on, playback and
capture running together, and stats polled every minute:

- PASS: 1,382,400,000 frames (7200.0 s) bit-exact, 0 mismatches.
- 0 underruns, 0 capture overruns or drops, 0 SAI FIFO errors. The
  playback lead stayed at 422..426 frames for the whole run.
- Feedback 192004.883 Hz (0x00180028).
- USB ISR over 180 million entries: mean 2.15 us, max 8.89 us, load
  5.37 %. The max came from a setup packet (8.20 us, the stats
  polling); SOF max was 2.28 us and transfer max 4.20 us.

### usbfs host abuse tests at 192 kHz (2026-11-03)

`tools/usbhost.py` detaches snd-usb-audio and runs its own iso host:
playback paced by the device feedback, a counter pattern through the
debug loopback, and capture checked frame by frame. All tests pass at
192 kHz on ketunkolo (`sudo RATE=192000 python3 tools/usbhost.py`):

- stream: 10 s, 1.92 M frames, 0 discontinuities, 0 counters.
- ratechange: SET_CUR 192→48, 48→192 and 192→96 kHz mid-stream. GET_CUR
  follows, the rings restart (2–3 discontinuities at the switch), and a
  clean stream follows.
- halt: SET_FEATURE and CLEAR_FEATURE(ENDPOINT_HALT) on 0x01, 0x81 and
  0x82 mid-stream. 0 discontinuities.
- oddsize: OUT packets with 3 or 7 junk bytes (not a whole frame), and a
  zero-length packet every 101st. 0 corrupt frames; the zero-length
  packets count as underruns (26), as they should.
- altstorm: 1000 SET_INTERFACE in 0.87 s plus 50 short streams, then a
  clean stream.
- control: 21 malformed requests (wLength short and long, SET_CUR with
  bad lengths or rates, unknown entities, selectors, descriptors and
  vendor requests, wrong interface). All give the expected length or
  stall, and a clean stream follows.

The same tests through QEMU usb-host passthrough to a VM fail. The iso
IN data arrives with missing frames, stale frames from earlier
transfers and zero-filled gaps, while the firmware is clean on bare
metal. Raising QEMU's usb-host iso buffering from the default
isobufs=4, isobsize=32 to 32/64 (added from the monitor with a passed
fd) removed the stale frames and cut the discontinuities from 437 to 50
in 10 s. The rest is one zeroed frame about every 0.2 s, which matches
the rate of 25-frame (200-byte, max size) capture packets (not verified
per packet). Don't use VM passthrough for iso data checks.
