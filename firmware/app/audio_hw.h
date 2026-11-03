// diyuac2 audio hardware layer: serial audio interface, DMA rings, codec bus.
//
// The MCU is I2S clock master. The audio interface runs continuously once
// started; playback and capture move data through two DMA rings of
// AUDIO_HW_RING_FRAMES stereo 32-bit frames each. All positions are free
// running 32-bit counters (modulo 2^32), the ring index is counter % ring.
//
// Implementation for i.MX RT1020: audio_hw_imxrt.c (SAI1, eDMA ch 0/1,
// audio PLL, LPI2C1).

#ifndef AUDIO_HW_H_
#define AUDIO_HW_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define AUDIO_HW_CHANNELS    2
#define AUDIO_HW_RING_FRAMES 2048u // power of two

// Pins, clocks, codec bus. Does not start audio.
void audio_hw_init(void);

// (Re)start the audio interface at the given rate. MCLK = 256 * rate,
// BCLK = 64 * rate, I2S, 32-bit slots. Rings are zeroed and counters reset.
bool audio_hw_start(uint32_t rate);
void audio_hw_stop(void);

// Ring buffers, interleaved L/R, AUDIO_HW_RING_FRAMES frames.
int32_t *audio_hw_tx_ring(void);
int32_t const *audio_hw_rx_ring(void);

// Playback: frames fetched by DMA from the TX ring (ring read position).
uint32_t audio_hw_tx_frames_fetched(void);
// Playback: 32-bit words actually taken from the TX FIFO by the serial
// shifter (DMA position minus FIFO fill). The timing reference for
// feedback. Safe to call from any context.
uint32_t audio_hw_tx_words_played(void);
// Capture: frames written by DMA into the RX ring (ring write position).
uint32_t audio_hw_rx_frames_written(void);

// Clears and returns hardware FIFO error counts (polled).
void audio_hw_poll(uint32_t *tx_fifo_errors, uint32_t *rx_fifo_errors);

// Codec control bus: write len bytes to 7-bit address addr.
bool audio_hw_i2c_write(uint8_t addr, uint8_t const *data, size_t len);

#endif
