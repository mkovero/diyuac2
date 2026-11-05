# diyuac2 firmware, Rust port

Full-Rust firmware for the MIMXRT1020-EVK, built on imxrt-rt, imxrt-ral and
imxrt-hal. The USB device stack is our own (`common/src/usb/`): a ChipIdea HS
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

Crates (one cargo workspace):

| Crate | What |
|---|---|
| `.` (`diyuac2`) | the app: UAC2 + vendor + DFU runtime interfaces, audio path |
| `boot/` (`diyuac2-boot`) | USB DFU bootloader, at the flash start |
| `common/` (`diyuac2-common`) | USB device core, clocks, serial number, flash (ROM API), flash layout, CRC-32, ISR profile |

## Build and flash

```sh
rustup target add thumbv7em-none-eabihf
cargo build --release --workspace
R=target/thumbv7em-none-eabihf/release
arm-none-eabi-objcopy -O binary $R/diyuac2 target/app.bin
arm-none-eabi-objcopy -O binary $R/diyuac2-boot target/boot.bin

# Over USB (the app or the bootloader must be running):
sudo ../tools/dfu.py update target/app.bin

# With a debug probe (bootloader + app + app info sector):
../tools/mkimage.py target/app.bin target/full.hex target/boot.bin
pyocd load -t mimxrt1020 -f 500000 -M under-reset target/full.hex
pyocd reset -t mimxrt1020 -f 500000 -m hw    # hardware reset; no power cycle needed
```

The app is linked for flash offset 0x40000 and cannot boot from the boot
ROM by itself; flashing only the app with a probe also needs the info
sector (`mkimage.py app.bin out.hex` without `boot.bin`).

`.data` and `.bss` are placed in DTCM (`build.rs`), so the USB queue heads,
dTDs and buffers are not cached and the USB DMA can reach them.

The FlexSPI configuration block (`boot/src/fcb.rs`) is a copy of the one
the C firmware boots with.

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

## 44.1 kHz family (2026-11-05)

The clock source now offers 44.1, 48, 88.2, 96, 176.4 and 192 kHz. The
48 kHz family runs PLL4 at 786.432 MHz, the 44.1 kHz family at
722.5344 MHz (24 MHz x (30 + 1056/10000) = 4 x 4096 x 44.1 kHz). The SAI
root dividers are the same in both families (16/8/4), and the PLL is
reprogrammed only when a rate change crosses families. 44.1 kHz is within
the WM8960 specification; 88.2 and 176.4 kHz are not (stats flag 4).
At 44.1 and 176.4 kHz the packets alternate between two sizes
(5/6 and 22/23 frames per microframe), so capture packets may carry the
nominal count rounded up plus one.

- `tools/loopback.py` bit-exact at all six rates, 0 counters. Feedback
  44101.07 Hz at 44.1 kHz, +24 ppm like the 48 kHz family.
- `tools/usbhost.py` passes at 192 kHz, with rate changes across the
  families mid-stream (192 -> 176.4 -> 44.1 -> 192 kHz).
- Rate switch cost in the USB ISR: 41 us within a family, about 0.49 ms
  across families (PLL relock).

Fixed on the way: `audio_hw::start()` waited for the DMA to prefill the
TX FIFO before enabling the transmitter. The DMA does not fill it while
the transmitter is disabled, so the wait always ran into its timeout:
6.4 ms inside the USB ISR on every rate set. It is gone; the TX FIFO
error counter stays 0 at stream start.

Also fixed in `tools/usbhost.py`: at exit it gave the interfaces back
one by one, so snd-usb-audio probed interface 0 while the streaming
interfaces were still claimed, and the card came back without PCM
devices.

## Volume and mute (Feature Units, 2026-11-05)

Feature Unit 0x02 sits between the playback input terminal and the line
out, Feature Unit 0x12 between the line in and the capture output
terminal. Each has a master mute and a master volume from -100 dB to 0 dB
in 0.5 dB steps (UAC2 1/256 dB units). The gain is applied to the samples
in the USB ISR (`src/gain.rs`, Q2.30 factor from a table built at compile
time). At 0 dB, unmuted, the samples are not touched, so the path is
bit-exact. Values out of range clamp, others round to the nearest step.
Linux shows them as `PCM Playback Volume/Switch` and `Line Capture
Volume/Switch`.

The C firmware left the Feature Unit out so that host volume could not
alter a measurement path. That risk is real: on ketunkolo the host mixer
set the playback volume to -23.5 dB as soon as the card appeared. So
any setting other than 0 dB unmuted raises stats flag bit 6 (`gain` in
`vendorctl.py stats`), and `tools/loopback.py` and `tools/lbsoak.py` set
both units to 0 dB before they run. The volumes power up at 0 dB and are
not stored.

- `tools/usbhost.py gain` at 192 kHz: playback and capture at -6 dB and
  -60 dB, every captured frame equals the played counter frame scaled by
  the firmware's factor (about 384,000 frames each, 0 mismatches). Mute
  gives all-zero capture. The flag is set in every non-unity case and
  clears at 0 dB, with a bit-exact stream after.
- `tools/usbhost.py control` adds Feature Unit cases: RANGE (8 bytes),
  wrong sizes, channel 1, unknown selectors and RANGE on mute all stall.

No interrupt endpoint: UAC2 uses it to tell the host about changes the
device makes itself, and the device makes none (rate, volume and mute
only change on host requests).

The configuration descriptor grew to 263 bytes, past the 256-byte EP0
buffer, which would have truncated it; the buffer is 512 bytes now, with
a compile-time check. (With the DFU interface it is 281 bytes, and the
shared core's EP0 buffer is 4 KiB, the DFU block size.)

## Firmware update over USB (DFU, 0.4.0, 2026-11-05)

A small bootloader (`boot/`, 26 KiB) sits at the flash start and is never
written over USB. The app moved to flash offset 0x40000.

```text
0x6000_0000  bootloader: FCB, IVT, code         (boot ROM starts it)
0x6003_F000  app info sector: "D2AI", length, CRC-32, CRC of those
0x6004_0000  app image, vector table first, up to 1 MiB
```

(`common/src/layout.rs`.) At reset the bootloader checks for a DFU request
from the app (a magic in SRC_GPR10, which survives SYSRESETREQ and is
cleared at power-on) and for a valid app (info sector, CRC-32 of the
image). With no request and a valid app it jumps to the app before
touching any clock or peripheral, so the app starts from the same state
as from the boot ROM. Otherwise it stays in DFU mode: USB DFU 1.1, VID:PID
cafe:0ac3, product "diyuac2 bootloader", same serial as the app, high
speed, polled (no interrupts). LED: short flash every 500 ms.

The app has a DFU runtime interface (interface 4, at both speeds, so a
full-speed link can still be updated). DFU_DETACH makes the app leave the
bus and reset into the bootloader.

Download: each 4 KiB DNLOAD block is erased, programmed and read back in
the OUT data stage, through the boot ROM's FlexSPI NOR driver (the ROM API
tree, as MCUXpresso `fsl_romapi`). The first block erases the info
sector, so an interrupted update leaves no valid app and the bootloader
stays in DFU mode. After the final zero-length DNLOAD, GETSTATUS checks
the vector table (stack pointer in DTCM/OCRAM, reset vector inside the
image), compares the CRC-32 of the flash contents with the CRC-32 of the
data received, and writes the info sector. The device is not
manifestation tolerant: it reports dfuMANIFEST, then
dfuMANIFEST-WAIT-RESET, and resets into the new app on the host's bus
reset or after 3 s. Out-of-order blocks, a block after a short one,
images over 1 MiB, UPLOAD and unknown requests end in dfuERROR with the
matching DFU status; CLRSTATUS recovers.

The boot FCB has only the read sequence in its LUT. The ROM driver needs
read status, write enable, sector erase and page program too (with the
boot FCB it waited forever for the status register), so the bootloader
hands it a copy with those sequences added (`fcb::rom_config`,
IS25LP064A opcodes, single pad).

`tools/dfu.py` (Python only, usbfs) does detach, download, manifest and
waits for the app to come back. Stock dfu-util 0.11 works too (as root,
or with udev rules for both IDs):

```sh
dfu-util -d cafe:0ac2,cafe:0ac3 -D app.bin -R   # update from the running app
dfu-util -d cafe:0ac2 -e                        # app -> bootloader only
```

On ketunkolo: detach, 9 blocks, dfuMANIFEST, dfuMANIFEST-WAIT-RESET, USB
reset, app back at high speed, exit 0, 7.3 s (dfu-util waits 1 s at
dfuMANIFEST and polls for the re-enumeration). Without `-R` dfu-util
still resets the bus at dfuMANIFEST-WAIT-RESET. The file has no DFU
suffix, so dfu-util warns about it. usbguard there has an id+serial+port
rule for cafe:0ac3 as for cafe:0ac2.

Results on ketunkolo (bare metal):

- `dfu.py update`: 34,816-byte app in 9 blocks, 0.7 s download, 1.7 to
  2.2 s from the app's DETACH to the new app enumerating again at high
  speed. Repeated several times, including while `aplay` was playing at
  192 kHz (aplay gets ENODEV, as expected).
- Full-speed config: a `force-full-speed` build installed by DFU came up
  at 12 Mbit/s and was replaced by DFU through its full-speed DFU
  interface; the normal build came back at 480.
- Fault cases (scratch script): zero-length DNLOAD in dfuIDLE (errNOTDONE),
  UPLOAD (errSTALLEDPKT), block 2 after block 0 (errADDRESS), random data
  as image (errFILE at manifest), a block after a short block (stall),
  ABORT mid-download then DETACH: comes back as the bootloader, no app.
  All as expected, CLRSTATUS recovers each.
- Probe path: `mkimage.py` hex (bootloader + info + app) boots the app;
  erasing the info sector leaves the bootloader in DFU mode; hardware
  reset with a valid app boots the app.
- Regression at 192 kHz after the restructure: `usbhost.py all` passes,
  `faults.sh all` passes, `loopback.py` bit-exact at all six rates.

`usbhost.py` now sets both Feature Units to 0 dB at start: after any
re-enumeration (a DFU update is one) the host mixer sets its own volume
again, and the counter-frame checks failed on the scaled data.

Not done: entering DFU mode by button (SW4) when the app is broken but
its CRC is fine; until then the probe is the fallback for an app that
boots but hangs before USB is up.
