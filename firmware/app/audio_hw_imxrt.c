// diyuac2 audio hardware layer for i.MX RT1020 (MIMXRT1020-EVK).
//
// Clocking: audio PLL (PLL4) 786.432 MHz -> SAI1 clock root /4 /N = 256*fs,
// output on SAI1_MCLK (codec SYSCLK). SAI1 TX is I2S master (BCLK = 64 fs,
// FS = fs); RX is synchronous to TX. The audio clock derives from the 24 MHz
// crystal and is independent of the USB host clock.
//
// Data: eDMA channel 0 loops over the TX ring into TDR0, channel 1 loops from
// RDR0 into the RX ring, both forever (SLAST / DLAST rewind the address at
// major loop end). A major-loop interrupt counts ring wraps; position within
// the ring comes from CITER. Rings are in DTCM (not cached, DMA accessible),
// so no cache maintenance is needed.

#include <string.h>

#include "audio_hw.h"

#ifdef __GNUC__
  #pragma GCC diagnostic push
  #pragma GCC diagnostic ignored "-Wunused-parameter"
#endif
#include "fsl_device_registers.h"
#include "fsl_clock.h"
#include "fsl_iomuxc.h"
#include "fsl_lpi2c.h"
#include "fsl_sai.h"
#ifdef __GNUC__
  #pragma GCC diagnostic pop
#endif

#define SAI_BASE      SAI1
#define TX_CH         0u
#define RX_CH         1u
#define RING_WORDS    (AUDIO_HW_RING_FRAMES * AUDIO_HW_CHANNELS)
#define SAI_FIFO_MASK 0x3Fu // FIFO pointers: 5 bits index + wrap bit
#define I2C_BASE      LPI2C1
#define I2C_CLK_HZ    10000000u // pll3_60m / 6

// Pad settings from the NXP EVK reference: SAI 0x10B0 (keeper, R0/6, 100 MHz),
// I2C 0xD8B0 (open drain, 22k pull-up).
#define PAD_SAI 0x10B0u
#define PAD_I2C 0xD8B0u

// TCSR/RCSR write-1-to-clear flags (WSF, SEF, FEF) must not be written back.
#define SAI_CSR_W1C (I2S_TCSR_WSF_MASK | I2S_TCSR_SEF_MASK | I2S_TCSR_FEF_MASK)

static int32_t tx_ring[RING_WORDS] __attribute__((aligned(32)));
static int32_t rx_ring[RING_WORDS] __attribute__((aligned(32)));

static volatile uint32_t tx_wraps;
static volatile uint32_t rx_wraps;

//--------------------------------------------------------------------+
// Interrupts: count major loop completions (ring wraps)
//--------------------------------------------------------------------+

void DMA0_DMA16_IRQHandler(void) {
  if (DMA0->INT & (1u << TX_CH)) {
    tx_wraps++; // increment before clearing the flag, see dma_words()
    DMA0->CINT = TX_CH;
  }
  __DSB();
}

void DMA1_DMA17_IRQHandler(void) {
  if (DMA0->INT & (1u << RX_CH)) {
    rx_wraps++;
    DMA0->CINT = RX_CH;
  }
  __DSB();
}

// Total words moved by a ring channel. Race-free against the wrap interrupt
// (which has higher priority than any caller): retry if the wrap count
// changed; if the major loop completed but its interrupt is still pending
// (caller runs with it masked or before entry), account for it here.
static uint32_t dma_words(uint32_t ch, volatile uint32_t *wraps) {
  uint32_t w, pos, pend;
  do {
    w    = *wraps;
    pos  = RING_WORDS - DMA0->TCD[ch].CITER_ELINKNO;
    pend = DMA0->INT & (1u << ch);
  } while (w != *wraps);
  if (pend && pos < RING_WORDS / 2) w++;
  return w * RING_WORDS + pos;
}

//--------------------------------------------------------------------+
// Init
//--------------------------------------------------------------------+

static void pins_init(void) {
  CLOCK_EnableClock(kCLOCK_Iomuxc);

  IOMUXC_SetPinMux(IOMUXC_GPIO_AD_B1_00_SAI1_MCLK, 0U);
  IOMUXC_SetPinMux(IOMUXC_GPIO_AD_B1_01_SAI1_TX_BCLK, 0U);
  IOMUXC_SetPinMux(IOMUXC_GPIO_AD_B1_02_SAI1_TX_SYNC, 0U);
  IOMUXC_SetPinMux(IOMUXC_GPIO_AD_B1_03_SAI1_TX_DATA00, 0U);
  IOMUXC_SetPinMux(IOMUXC_GPIO_AD_B1_05_SAI1_RX_DATA00, 0U);
  IOMUXC_SetPinConfig(IOMUXC_GPIO_AD_B1_00_SAI1_MCLK, PAD_SAI);
  IOMUXC_SetPinConfig(IOMUXC_GPIO_AD_B1_01_SAI1_TX_BCLK, PAD_SAI);
  IOMUXC_SetPinConfig(IOMUXC_GPIO_AD_B1_02_SAI1_TX_SYNC, PAD_SAI);
  IOMUXC_SetPinConfig(IOMUXC_GPIO_AD_B1_03_SAI1_TX_DATA00, PAD_SAI);
  IOMUXC_SetPinConfig(IOMUXC_GPIO_AD_B1_05_SAI1_RX_DATA00, PAD_SAI);

  IOMUXC_SetPinMux(IOMUXC_GPIO_AD_B1_14_LPI2C1_SCL, 1U);
  IOMUXC_SetPinMux(IOMUXC_GPIO_AD_B1_15_LPI2C1_SDA, 1U);
  IOMUXC_SetPinConfig(IOMUXC_GPIO_AD_B1_14_LPI2C1_SCL, PAD_I2C);
  IOMUXC_SetPinConfig(IOMUXC_GPIO_AD_B1_15_LPI2C1_SDA, PAD_I2C);

  // SAI1_MCLK pad is an output driven by SAI1 MCLK1 = SAI1 clock root.
  IOMUXC_SetSaiMClkClockSource(IOMUXC_GPR, kIOMUXC_GPR_SAI1MClk1Sel, 0);
  IOMUXC_GPR->GPR1 |= IOMUXC_GPR_GPR1_SAI1_MCLK_DIR_MASK;
}

static void clocks_init(void) {
  // 24 MHz * (32 + 768/1000) = 786.432 MHz
  clock_audio_pll_config_t const pll = {
    .loopDivider = 32, .postDivider = 1, .numerator = 768, .denominator = 1000,
  };
  CLOCK_InitAudioPll(&pll);

  CLOCK_SetMux(kCLOCK_Sai1Mux, 2);    // PLL4
  CLOCK_SetDiv(kCLOCK_Sai1PreDiv, 3); // /4 = 196.608 MHz
  CLOCK_SetDiv(kCLOCK_Sai1Div, 15);   // /16 = 12.288 MHz, changed per rate

  CLOCK_SetMux(kCLOCK_Lpi2cMux, 0);   // pll3_60m
  CLOCK_SetDiv(kCLOCK_Lpi2cDiv, 5);   // /6 = 10 MHz
}

static void i2c_init(void) {
  lpi2c_master_config_t cfg;
  LPI2C_MasterGetDefaultConfig(&cfg);
  cfg.baudRate_Hz = 100000;
  LPI2C_MasterInit(I2C_BASE, &cfg, I2C_CLK_HZ);
}

void audio_hw_init(void) {
  clocks_init();
  pins_init();
  i2c_init();

  CLOCK_EnableClock(kCLOCK_Dma); // eDMA + DMAMUX
  NVIC_SetPriority(DMA0_DMA16_IRQn, 0);
  NVIC_SetPriority(DMA1_DMA17_IRQn, 0);
  NVIC_EnableIRQ(DMA0_DMA16_IRQn);
  NVIC_EnableIRQ(DMA1_DMA17_IRQn);
}

//--------------------------------------------------------------------+
// Start / stop
//--------------------------------------------------------------------+

static void dma_ring_setup(uint32_t ch, uint32_t src, int16_t soff, int32_t slast,
                           uint32_t dst, int16_t doff, int32_t dlast, uint8_t mux_src) {
  DMA0->CERQ = (uint8_t) ch;
  DMAMUX->CHCFG[ch] = 0;

  DMA0->TCD[ch].CSR           = 0;
  DMA0->TCD[ch].SADDR         = src;
  DMA0->TCD[ch].SOFF          = soff;
  DMA0->TCD[ch].ATTR          = DMA_ATTR_SSIZE(2) | DMA_ATTR_DSIZE(2);
  DMA0->TCD[ch].NBYTES_MLNO   = 4;
  DMA0->TCD[ch].SLAST         = slast;
  DMA0->TCD[ch].DADDR         = dst;
  DMA0->TCD[ch].DOFF          = doff;
  DMA0->TCD[ch].CITER_ELINKNO = RING_WORDS;
  DMA0->TCD[ch].BITER_ELINKNO = RING_WORDS;
  DMA0->TCD[ch].DLAST_SGA     = dlast;
  DMA0->TCD[ch].CSR           = DMA_CSR_INTMAJOR_MASK;
  DMA0->CINT = (uint8_t) ch;
  DMA0->CERR = (uint8_t) ch;

  DMAMUX->CHCFG[ch] = DMAMUX_CHCFG_SOURCE(mux_src & 0x7Fu) | DMAMUX_CHCFG_ENBL_MASK;
}

static inline uint32_t tx_fifo_fill(uint32_t tfr) {
  return ((tfr >> I2S_TFR_WFP_SHIFT) - tfr) & SAI_FIFO_MASK;
}

static bool rate_divider(uint32_t rate, uint32_t *div) {
  // SAI1 root = 196.608 MHz / div = 256 * rate
  uint32_t const d = 768000u / rate;
  if (d == 0 || d > 64 || d * rate != 768000u) return false;
  *div = d;
  return true;
}

void audio_hw_stop(void) {
  I2S_Type *sai = SAI_BASE;

  // RX is synchronous to TX: stop RX first, then TX. Enables clear at the
  // end of the current frame.
  sai->RCSR = (sai->RCSR & ~SAI_CSR_W1C) & ~(I2S_RCSR_RE_MASK | I2S_RCSR_FRDE_MASK);
  sai->TCSR = (sai->TCSR & ~SAI_CSR_W1C) & ~(I2S_TCSR_TE_MASK | I2S_TCSR_FRDE_MASK);
  for (uint32_t i = 0; i < 1000000u && ((sai->RCSR & I2S_RCSR_RE_MASK) || (sai->TCSR & I2S_TCSR_TE_MASK)); i++) {
  }
  sai->TCSR = (sai->TCSR & ~SAI_CSR_W1C) & ~I2S_TCSR_BCE_MASK;
  sai->RCSR = (sai->RCSR & ~SAI_CSR_W1C) & ~I2S_RCSR_BCE_MASK;

  DMA0->CERQ = TX_CH;
  DMA0->CERQ = RX_CH;
}

bool audio_hw_start(uint32_t rate) {
  uint32_t div;
  if (!rate_divider(rate, &div)) return false;

  audio_hw_stop();

  CLOCK_SetDiv(kCLOCK_Sai1Div, div - 1);
  uint32_t const mclk = 256u * rate;

  SAI_Init(SAI_BASE); // clock gate on, software reset, FIFO reset
  sai_transceiver_t cfg;
  SAI_GetClassicI2SConfig(&cfg, kSAI_WordWidth32bits, kSAI_Stereo, kSAI_Channel0Mask);
  cfg.masterSlave = kSAI_Master;
  cfg.syncMode    = kSAI_ModeAsync;
  SAI_TxSetConfig(SAI_BASE, &cfg);
  cfg.syncMode = kSAI_ModeSync;
  SAI_RxSetConfig(SAI_BASE, &cfg);
  SAI_TxSetBitClockRate(SAI_BASE, mclk, rate, 32, AUDIO_HW_CHANNELS);
  SAI_RxSetBitClockRate(SAI_BASE, mclk, rate, 32, AUDIO_HW_CHANNELS);

  memset(tx_ring, 0, sizeof(tx_ring));
  memset(rx_ring, 0, sizeof(rx_ring));
  tx_wraps = 0;
  rx_wraps = 0;

  dma_ring_setup(TX_CH, (uint32_t) tx_ring, 4, -(int32_t) sizeof(tx_ring),
                 (uint32_t) &SAI_BASE->TDR[0], 0, 0, (uint8_t) kDmaRequestMuxSai1Tx);
  dma_ring_setup(RX_CH, (uint32_t) &SAI_BASE->RDR[0], 0, 0,
                 (uint32_t) rx_ring, 4, -(int32_t) sizeof(rx_ring), (uint8_t) kDmaRequestMuxSai1Rx);

  DMA0->SERQ = TX_CH;
  DMA0->SERQ = RX_CH;
  SAI_TxEnableDMA(SAI_BASE, kSAI_FIFORequestDMAEnable, true);
  SAI_RxEnableDMA(SAI_BASE, kSAI_FIFORequestDMAEnable, true);

  // Let the DMA prefill the TX FIFO before the transmitter starts.
  for (uint32_t i = 0; i < 100000u && tx_fifo_fill(SAI_BASE->TFR[0]) <= 16u; i++) {
  }
  SAI_RxEnable(SAI_BASE, true); // also enables TX (RX synchronous to TX)
  SAI_TxEnable(SAI_BASE, true);
  return true;
}

//--------------------------------------------------------------------+
// Positions
//--------------------------------------------------------------------+

int32_t *audio_hw_tx_ring(void) { return tx_ring; }
int32_t const *audio_hw_rx_ring(void) { return rx_ring; }

uint32_t audio_hw_tx_frames_fetched(void) {
  return dma_words(TX_CH, &tx_wraps) / AUDIO_HW_CHANNELS;
}

uint32_t audio_hw_rx_frames_written(void) {
  return dma_words(RX_CH, &rx_wraps) / AUDIO_HW_CHANNELS;
}

uint32_t audio_hw_tx_words_played(void) {
  uint32_t f1, f2, d;
  // A DMA write between the reads changes the FIFO write pointer: retry.
  do {
    f1 = SAI_BASE->TFR[0];
    d  = dma_words(TX_CH, &tx_wraps);
    f2 = SAI_BASE->TFR[0];
  } while ((f1 ^ f2) & I2S_TFR_WFP_MASK);
  return d - tx_fifo_fill(f2);
}

void audio_hw_poll(uint32_t *tx_fifo_errors, uint32_t *rx_fifo_errors) {
  I2S_Type *sai = SAI_BASE;
  if (sai->TCSR & I2S_TCSR_FEF_MASK) {
    sai->TCSR = (sai->TCSR & ~SAI_CSR_W1C) | I2S_TCSR_FEF_MASK;
    (*tx_fifo_errors)++;
  }
  if (sai->RCSR & I2S_RCSR_FEF_MASK) {
    sai->RCSR = (sai->RCSR & ~SAI_CSR_W1C) | I2S_RCSR_FEF_MASK;
    (*rx_fifo_errors)++;
  }
}

//--------------------------------------------------------------------+
// Codec bus
//--------------------------------------------------------------------+

bool audio_hw_i2c_write(uint8_t addr, uint8_t const *data, size_t len) {
  lpi2c_master_transfer_t xfer = {
    .flags          = kLPI2C_TransferDefaultFlag,
    .slaveAddress   = addr,
    .direction      = kLPI2C_Write,
    .subaddress     = 0,
    .subaddressSize = 0,
    .data           = (void *) (uintptr_t) data,
    .dataSize       = len,
  };
  return LPI2C_MasterTransferBlocking(I2C_BASE, &xfer) == kStatus_Success;
}
