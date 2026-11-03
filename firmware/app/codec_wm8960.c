// WM8960 codec (MIMXRT1020-EVK, on LPI2C1 at 0x1A).
//
// Slave I2S, 32-bit words, SYSCLK = MCLK (no PLL), ADC/DAC = SYSCLK/256.
// The WM8960 is specified for 8..48 kHz, so only 48 kHz is offered: 96 and
// 192 kHz would need SYSCLK/128 and /64 ratios the part does not support.
//
// Register values follow the NXP SDK wm8960 driver defaults for this board
// (power, mixers, input selection), with gains changed for measurement:
// DAC 0 dB, headphone 0 dB, ADC 0 dB, input PGA 0 dB, mic boost 0 dB.
//
// Inputs (left = LINPUT1/LINPUT3 diff, right = RINPUT1/RINPUT2 diff) as in
// the NXP SDK. Measured by routing scan (2026-11-03, no schematic): the
// onboard mic P1 is on the right channel (RINPUT1/RINPUT2); RINPUT3 is dead
// (likely J11 jack detect); the left inputs carried no signal with nothing
// plugged into J11 (probably the J11 headset mic, unverified).

#include "audio_hw.h"
#include "codec.h"

#define WM8960_ADDR 0x1A

enum {
  R_LINVOL  = 0x00,
  R_RINVOL  = 0x01,
  R_LOUT1   = 0x02,
  R_ROUT1   = 0x03,
  R_CLOCK1  = 0x04,
  R_DACCTL1 = 0x05,
  R_IFACE1  = 0x07,
  R_IFACE2  = 0x09,
  R_LDAC    = 0x0A,
  R_RDAC    = 0x0B,
  R_RESET   = 0x0F,
  R_LADC    = 0x15,
  R_RADC    = 0x16,
  R_ADDCTL1 = 0x17,
  R_POWER1  = 0x19,
  R_POWER2  = 0x1A,
  R_LINPATH = 0x20,
  R_RINPATH = 0x21,
  R_LOUTMIX = 0x22,
  R_ROUTMIX = 0x25,
  R_BYPASS1 = 0x2D,
  R_BYPASS2 = 0x2E,
  R_POWER3  = 0x2F,
  R_ADDCTL4 = 0x30,
};

static uint32_t const rates[] = {48000};

size_t codec_rates(uint32_t const **r) {
  *r = rates;
  return sizeof(rates) / sizeof(rates[0]);
}

static bool wr(uint8_t reg, uint16_t val) {
  uint8_t const b[2] = {(uint8_t) ((reg << 1) | ((val >> 8) & 1u)), (uint8_t) val};
  return audio_hw_i2c_write(WM8960_ADDR, b, sizeof(b));
}

bool codec_write_reg(uint16_t reg, uint16_t val) {
  if (reg > 0x37 || val > 0x1FF) return false;
  return wr((uint8_t) reg, val);
}

bool codec_init(void) {
  static const struct { uint8_t reg; uint16_t val; } seq[] = {
    {R_RESET,   0x000},
    {R_POWER1,  0x0FE}, // VMID 50k, VREF, AINL/R, ADCL/R, MICB
    {R_POWER2,  0x1E0}, // DACL/R, LOUT1/ROUT1 (speaker amps off)
    {R_POWER3,  0x03C}, // LMIC/RMIC, LOMIX/ROMIX
    {R_CLOCK1,  0x000}, // SYSCLK = MCLK, ADC/DAC = SYSCLK/256
    {R_IFACE1,  0x00E}, // slave, 32-bit, I2S
    {R_IFACE2,  0x040}, // ADCLRC pin as GPIO: ADC uses DACLRC
    {R_LOUTMIX, 0x100}, // left DAC to left output mixer
    {R_ROUTMIX, 0x100}, // right DAC to right output mixer
    {R_ADDCTL1, 0x0C0}, // VSEL = 3.3 V
    {R_ADDCTL4, 0x040},
    {R_BYPASS1, 0x000},
    {R_BYPASS2, 0x000},
    {R_LADC,    0x1C3}, // ADC 0 dB, update
    {R_RADC,    0x1C3},
    {R_LDAC,    0x1FF}, // DAC 0 dB, update
    {R_RDAC,    0x1FF},
    {R_LOUT1,   0x179}, // headphone 0 dB, update
    {R_ROUT1,   0x179},
    {R_DACCTL1, 0x000}, // DAC unmute, no de-emphasis
    {R_LINPATH, 0x188}, // LMN1, LMP3, boost 0 dB, to PGA
    {R_RINPATH, 0x148}, // RMN1, RMP2, boost 0 dB, to PGA
    {R_LINVOL,  0x117}, // input PGA 0 dB, unmuted, update
    {R_RINVOL,  0x117},
  };

  for (size_t i = 0; i < sizeof(seq) / sizeof(seq[0]); i++) {
    if (!wr(seq[i].reg, seq[i].val)) return false;
  }
  return true;
}

bool codec_set_rate(uint32_t rate) {
  // Ratio is fixed at 256 fs; nothing to change for supported rates.
  for (size_t i = 0; i < sizeof(rates) / sizeof(rates[0]); i++) {
    if (rates[i] == rate) return true;
  }
  return false;
}
