# Playback rate feedback

The device is asynchronous. The audio clock comes from the audio PLL
(PLL4), which is derived from the 24 MHz crystal. The MCU is the I2S master,
and the codec runs as a slave on MCLK. The host clock (USB SOF) and the
audio clock are independent, so the host has to be told how many samples
per microframe to send. It is told through the explicit feedback endpoint
0x81, as 16.16 samples per microframe.

## Mechanism: measured audio clock, sampled at SOF

In the USB SOF interrupt, the firmware reads the number of 32-bit words the
SAI transmitter has actually shifted out. It reads this as:

    words_played = eDMA words fetched from the TX ring - words still in the SAI TX FIFO

(`audio_hw_tx_words_played()`). The firmware stores this count together
with the microframe number (FRINDEX). Every 2 ms it computes:

    fb = (words_now - words_then) / 2 * 65536 / (uframe_now - uframe_then)

over a sliding window of 512 samples, which is 1024 ms. The value is
clamped to nominal ±0.1 % and handed to TinyUSB with `tud_audio_n_fb_set()`.
After a stream start or a rate change, the window grows from 16 ms up to
1024 ms. Until 16 ms of history exist, the nominal value is sent.

The code is in `firmware/app/feedback.c`, with the counters in
`firmware/app/audio_hw_imxrt.c`.

Details:

- The DMA position is the ring wrap count, which a DMA interrupt counts,
  plus `BITER - CITER`. The read retries if a wrap happens during it, and
  a wrap whose interrupt is still pending is accounted for. The DMA
  interrupt has priority 0. USB has priority 2.
- Subtracting the FIFO fill removes the jitter of DMA bursts. The DMA moves
  up to 16 words at a time, which over a 1 s window would be about ±8 Hz at
  48 kHz. What is left is about ±1 word.
- The window slides, so each value is the exact consumption over the last
  second. The host's running total of samples requested therefore follows
  the device's consumed total to within one window, and quantisation does
  not accumulate. No FIFO-level trim term is needed. The soak test checks
  this, because the playback lead must not drift.
- TinyUSB only calls `tud_audio_feedback_interval_isr()` and enables SOF for
  the `FREQUENCY_*` methods. The firmware registers `FREQUENCY_FIXED` with
  `mclk_freq = SystemCoreClock`, only to pass TinyUSB's interval check, and
  overrides the ISR hook. `tud_audio_feedback_update()` is never called.

## Why this and not the alternatives

| Option | Verdict |
|---|---|
| FIFO fill level (TinyUSB `FIFO_COUNT`, M2 placeholder) | Simple, but it is a control loop on buffer level. The value hunts, it depends on the host's packet pattern, and it is ±1-8 Hz biased (M2 measurements). |
| Count MCLK cycles per SOF with a timer (GPT capture of MCLK, triggered by SOF) | The classic hardware approach. It needs MCLK routed to a timer input, which is a board change, and an SOF-to-timer trigger. On CS4270 hardware MCLK may not come from the MCU. |
| Count audio frames at SOF (chosen) | Measures exactly what matters: samples consumed per host microframe. Needs no extra pins or timers and works whoever generates MCLK. The SAI frame counter is reconstructed from DMA and FIFO pointers. It works the same when the codec is the clock master, because only the SAI's consumption is observed. |

RME devices (for example the Babyface) also sample at the interrupt. The
approach here is the same in spirit.

## Measured (M3, WM8960, 48 kHz, Linux, 2026-11-03)

- 60 s of simultaneous playback and capture, usbmon on the feedback EP,
  first 10 % discarded: mean 48001.30 Hz, stdev 0.27 Hz, range
  48000.49-48002.44 Hz. The crystal offset is about +27 ppm.
- Playback lead (ring frames ahead of DMA): 103..113 frames for the whole
  run, so there is no drift. The lead is primed to about 2 ms
  (96 frames + 16 guard).
- 0 underruns, 0 capture overruns or drops, 0 SAI FIFO errors.
