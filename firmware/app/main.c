// diyuac2 firmware, milestone M3: UAC2 audio through SAI1 + WM8960.
//
// The SAI frame clock (audio PLL, MCU is I2S master) is the device clock.
// Playback: USB OUT FIFO -> TX DMA ring, kept a little ahead of the DMA.
// Capture: RX DMA ring -> USB IN FIFO. Explicit feedback is measured from
// the SAI transmitter in the SOF interrupt (feedback.c).

#include <string.h>

#include "bsp/board_api.h"
#include "tusb.h"

#include "audio_hw.h"
#include "codec.h"
#include "feedback.h"
#include "usb_descriptors.h"
#include "vendor_ctrl.h"

#ifdef __GNUC__
  #pragma GCC diagnostic push
  #pragma GCC diagnostic ignored "-Wunused-parameter"
#endif
#include "fsl_device_registers.h"
#ifdef __GNUC__
  #pragma GCC diagnostic pop
#endif

//--------------------------------------------------------------------+
// State
//--------------------------------------------------------------------+

#define FRAME_BYTES (AUDIO_N_CHANNELS * AUDIO_BYTES_PER_SAMPLE)
#define RING_FRAMES AUDIO_HW_RING_FRAMES
#define RING_MASK   (RING_FRAMES - 1u)

// Playback: frames written ahead of the DMA fetch position never come
// closer than GUARD (the DMA moves in bursts of up to half the SAI FIFO)
// and never further than MAX_LEAD.
#define PLAY_GUARD    16u
#define PLAY_MAX_LEAD (RING_FRAMES / 2u)
// Playback starts when this many milliseconds of data are buffered.
#define PLAY_PRIME_MS 2u

static uint32_t const *sample_rates;
static size_t          n_sample_rates;

static volatile uint32_t current_sample_rate;
static volatile bool     play_active;
static volatile bool     rec_active;
static volatile bool     rec_restart;
static bool              codec_ok;

static uint32_t play_wr;     // next frame position to write in the TX ring
static uint32_t play_zeroed; // TX ring cleared up to here (behind DMA)
static bool     play_primed;
static bool     play_starved; // ring ran dry; counted when data resumes
static uint32_t rec_rd;      // next frame position to read from the RX ring

static struct {
  uint32_t play_underruns;
  uint32_t rec_overruns;
  uint32_t rec_drops;
  uint32_t tx_fifo_errors;
  uint32_t rx_fifo_errors;
  int32_t  lead_min;
  int32_t  lead_max;
} stats;

static void audio_task(void);
static void led_task(void);

static void lead_stats_reset(void) {
  stats.lead_min = INT32_MAX;
  stats.lead_max = INT32_MIN;
}

static bool audio_set_rate(uint32_t rate) {
  if (!audio_hw_start(rate)) return false;
  if (codec_ok) codec_set_rate(rate);
  current_sample_rate = rate;
  play_wr = play_zeroed = 0;
  play_primed = false;
  rec_rd = 0;
  feedback_reset(rate);
  return true;
}

//--------------------------------------------------------------------+
// Main
//--------------------------------------------------------------------+

int main(void) {
  board_init();

  n_sample_rates = codec_rates(&sample_rates);
  audio_hw_init();
  lead_stats_reset();
  audio_set_rate(sample_rates[0]); // MCLK runs from here on
  codec_ok = codec_init();

  tusb_rhport_init_t dev_init = {
    .role  = TUSB_ROLE_DEVICE,
    .speed = TUSB_SPEED_HIGH,
  };
  tusb_init(BOARD_TUD_RHPORT, &dev_init);
  // Below the DMA wrap interrupt, which must preempt position reads.
  NVIC_SetPriority(USB_OTG1_IRQn, 2);

  board_init_after_tusb();

  while (1) {
    tud_task();
    audio_task();
    led_task();
  }
}

//--------------------------------------------------------------------+
// Audio data path
//--------------------------------------------------------------------+

static void play_task(void) {
  int32_t *ring    = audio_hw_tx_ring();
  uint32_t const rd = audio_hw_tx_frames_fetched();

  // Clear what the DMA has fetched, so a starved ring plays silence.
  uint32_t n = rd - play_zeroed;
  if (n > RING_FRAMES) n = RING_FRAMES;
  for (uint32_t pos = rd - n; pos != rd;) {
    uint32_t const i     = pos & RING_MASK;
    uint32_t const chunk = TU_MIN(rd - pos, RING_FRAMES - i);
    memset(&ring[i * AUDIO_HW_CHANNELS], 0, chunk * FRAME_BYTES);
    pos += chunk;
  }
  play_zeroed = rd;

  if (!play_active) {
    play_primed  = false;
    play_starved = false;
    return;
  }

  uint32_t avail = tud_audio_available() / FRAME_BYTES;

  if (!play_primed) {
    // A gap inside a stream is an underrun. The host stopping before it
    // selects alt 0 also empties the ring, but is not counted.
    if (play_starved && avail > 0) {
      stats.play_underruns++;
      play_starved = false;
    }
    if (avail < current_sample_rate / 1000u * PLAY_PRIME_MS) return;
    play_primed = true;
    play_wr     = rd + PLAY_GUARD;
  }

  int32_t lead = (int32_t) (play_wr - rd);
  if (lead < 0) {
    // DMA overtook the writer: silence is being played.
    play_primed  = false;
    play_starved = true;
    return;
  }

  while (avail > 0 && (uint32_t) lead < PLAY_MAX_LEAD) {
    uint32_t const i     = play_wr & RING_MASK;
    uint32_t       chunk = TU_MIN(avail, PLAY_MAX_LEAD - (uint32_t) lead);
    chunk                = TU_MIN(chunk, RING_FRAMES - i);
    uint32_t const got   = tud_audio_read(&ring[i * AUDIO_HW_CHANNELS], (uint16_t) (chunk * FRAME_BYTES)) / FRAME_BYTES;
    if (got == 0) break;
    play_wr += got;
    lead += (int32_t) got;
    avail -= TU_MIN(avail, got);
  }

  if (lead < stats.lead_min) stats.lead_min = lead;
  if (lead > stats.lead_max) stats.lead_max = lead;
}

static void rec_task(void) {
  uint32_t const wr = audio_hw_rx_frames_written();

  if (!rec_active || rec_restart) {
    rec_restart = false;
    rec_rd      = wr;
    return;
  }

  uint32_t n = wr - rec_rd;
  if (n > RING_FRAMES - PLAY_GUARD) {
    stats.rec_overruns++;
    rec_rd = wr;
    return;
  }

  int32_t const *ring = audio_hw_rx_ring();
  while (n > 0) {
    uint32_t const i     = rec_rd & RING_MASK;
    uint32_t const chunk = TU_MIN(n, RING_FRAMES - i);
    uint32_t const want  = chunk * FRAME_BYTES;
    uint32_t const put   = tud_audio_write(&ring[i * AUDIO_HW_CHANNELS], (uint16_t) want);
    if (put < want) stats.rec_drops += (want - put) / FRAME_BYTES;
    rec_rd += chunk;
    n -= chunk;
  }
}

static void audio_task(void) {
  play_task();
  rec_task();
  audio_hw_poll(&stats.tx_fifo_errors, &stats.rx_fifo_errors);
}

void audio_get_stats(vendor_stats_t *st) {
  st->rate           = current_sample_rate;
  st->feedback       = feedback_last();
  st->play_underruns = stats.play_underruns;
  st->rec_overruns   = stats.rec_overruns;
  st->rec_drops      = stats.rec_drops;
  st->tx_fifo_errors = stats.tx_fifo_errors;
  st->rx_fifo_errors = stats.rx_fifo_errors;
  st->play_lead_min  = stats.lead_min == INT32_MAX ? 0 : stats.lead_min;
  st->play_lead_max  = stats.lead_max == INT32_MIN ? 0 : stats.lead_max;
  st->flags          = (uint16_t) ((codec_ok ? STATS_FLAG_CODEC_OK : 0) |
                                   (play_active ? STATS_FLAG_PLAY_ACTIVE : 0) |
                                   (rec_active ? STATS_FLAG_REC_ACTIVE : 0) |
                                   (play_primed ? STATS_FLAG_PLAY_PRIMED : 0));
  st->reserved       = 0;
}

void audio_clear_stats(void) {
  memset(&stats, 0, sizeof(stats));
  lead_stats_reset();
}

//--------------------------------------------------------------------+
// Clock source requests
//--------------------------------------------------------------------+

static bool rate_supported(uint32_t rate) {
  for (size_t i = 0; i < n_sample_rates; i++) {
    if (sample_rates[i] == rate) return true;
  }
  return false;
}

#define MAX_RATES 8

static bool clock_get_request(uint8_t rhport, tusb_control_request_t const *req) {
  uint8_t const ctrl_sel = TU_U16_HIGH(req->wValue);

  if (ctrl_sel == AUDIO20_CS_CTRL_SAM_FREQ) {
    if (req->bRequest == AUDIO20_CS_REQ_CUR) {
      audio20_control_cur_4_t cur = {.bCur = (int32_t) tu_htole32(current_sample_rate)};
      return tud_audio_buffer_and_schedule_control_xfer(rhport, req, &cur, sizeof(cur));
    }
    if (req->bRequest == AUDIO20_CS_REQ_RANGE) {
      size_t const n = TU_MIN(n_sample_rates, MAX_RATES);
      audio20_control_range_4_n_t(MAX_RATES) range = {.wNumSubRanges = tu_htole16((uint16_t) n)};
      for (size_t i = 0; i < n; i++) {
        range.subrange[i].bMin = (int32_t) sample_rates[i];
        range.subrange[i].bMax = (int32_t) sample_rates[i];
        range.subrange[i].bRes = 0;
      }
      uint16_t const len = (uint16_t) (2 + n * 12);
      return tud_audio_buffer_and_schedule_control_xfer(rhport, req, &range, len);
    }
  } else if (ctrl_sel == AUDIO20_CS_CTRL_CLK_VALID && req->bRequest == AUDIO20_CS_REQ_CUR) {
    audio20_control_cur_1_t valid = {.bCur = 1};
    return tud_audio_buffer_and_schedule_control_xfer(rhport, req, &valid, sizeof(valid));
  }
  return false;
}

static bool clock_set_request(tusb_control_request_t const *req, uint8_t const *buf) {
  TU_VERIFY(req->bRequest == AUDIO20_CS_REQ_CUR);
  TU_VERIFY(TU_U16_HIGH(req->wValue) == AUDIO20_CS_CTRL_SAM_FREQ);
  TU_VERIFY(req->wLength == sizeof(audio20_control_cur_4_t));

  uint32_t const rate = (uint32_t) ((audio20_control_cur_4_t const *) buf)->bCur;
  TU_VERIFY(rate_supported(rate));
  if (rate == current_sample_rate) return true;
  return audio_set_rate(rate);
}

bool tud_audio_get_req_entity_cb(uint8_t rhport, tusb_control_request_t const *req) {
  if (TU_U16_HIGH(req->wIndex) == UAC2_ENTITY_CLOCK) return clock_get_request(rhport, req);
  return false;
}

bool tud_audio_set_req_entity_cb(uint8_t rhport, tusb_control_request_t const *req, uint8_t *buf) {
  (void) rhport;
  if (TU_U16_HIGH(req->wIndex) == UAC2_ENTITY_CLOCK) return clock_set_request(req, buf);
  return false;
}

//--------------------------------------------------------------------+
// Interface / stream state
//--------------------------------------------------------------------+

bool tud_audio_set_itf_cb(uint8_t rhport, tusb_control_request_t const *req) {
  (void) rhport;
  uint8_t const itf = tu_u16_low(req->wIndex);
  uint8_t const alt = tu_u16_low(req->wValue);

  if (itf == ITF_NUM_AUDIO_STREAMING_OUT) {
    if (alt != 0) {
      feedback_reset(current_sample_rate);
      tud_audio_n_fb_set(0, feedback_nominal(current_sample_rate));
      lead_stats_reset();
    }
    play_active = (alt != 0);
  } else if (itf == ITF_NUM_AUDIO_STREAMING_IN) {
    if (alt != 0) {
      tud_audio_clear_ep_in_ff(); // start capture with minimal latency
      rec_restart = true;
    }
    rec_active = (alt != 0);
  }
  return true;
}

bool tud_audio_set_itf_close_ep_cb(uint8_t rhport, tusb_control_request_t const *req) {
  (void) rhport;
  uint8_t const itf = tu_u16_low(req->wIndex);

  if (itf == ITF_NUM_AUDIO_STREAMING_OUT) play_active = false;
  if (itf == ITF_NUM_AUDIO_STREAMING_IN)  rec_active  = false;
  return true;
}

void tud_audio_feedback_params_cb(uint8_t func_id, uint8_t alt_itf, audio_feedback_params_t *feedback_param) {
  (void) func_id;
  (void) alt_itf;
  // FREQUENCY_FIXED only to make TinyUSB enable the SOF interrupt and call
  // tud_audio_feedback_interval_isr(); the value is computed in feedback.c
  // from SAI timing, tud_audio_feedback_update() is never used. mclk_freq
  // just has to pass TinyUSB's interval sanity check.
  feedback_param->method               = AUDIO_FEEDBACK_METHOD_FREQUENCY_FIXED;
  feedback_param->sample_freq          = current_sample_rate;
  feedback_param->frequency.mclk_freq  = SystemCoreClock;
}

//--------------------------------------------------------------------+
// Device state / LED
//--------------------------------------------------------------------+

static uint32_t blink_ms = 250;

void tud_mount_cb(void)   { blink_ms = 1000; }
void tud_umount_cb(void)  { blink_ms = 250; }
void tud_suspend_cb(bool remote_wakeup_en) { (void) remote_wakeup_en; blink_ms = 2500; }
void tud_resume_cb(void)  { blink_ms = tud_mounted() ? 1000 : 250; }

static void led_task(void) {
  static uint32_t start_ms;
  static bool state;
  uint32_t interval = blink_ms;
  if (!codec_ok) interval = 100;
  else if (play_active || rec_active) interval = 50;

  if (tusb_time_millis_api() - start_ms < interval) return;
  start_ms += interval;
  board_led_write(state);
  state = !state;
}
