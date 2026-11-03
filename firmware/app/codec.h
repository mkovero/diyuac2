// diyuac2 codec interface. One implementation is linked per board
// (codec_wm8960.c for the MIMXRT1020-EVK, CS4270 planned for M5).
//
// The codec is always I2S slave with SYSCLK = MCLK = 256 fs, 32-bit slots.

#ifndef CODEC_H_
#define CODEC_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Sample rates the codec supports in this clocking scheme, ascending.
size_t codec_rates(uint32_t const **rates);

// Power up and configure. MCLK must be running.
bool codec_init(void);

// Debug: raw register write (codec specific address/value width).
bool codec_write_reg(uint16_t reg, uint16_t val);

// Called after the audio interface was restarted at a new rate.
bool codec_set_rate(uint32_t rate);

#endif
