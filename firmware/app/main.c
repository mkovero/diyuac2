// diyuac2 firmware, milestone M2: own UAC2 descriptor set + vendor control interface.
//
// No audio hardware yet: the audio task runs from the 1 ms system tick and
// loops playback (OUT) data back to capture (IN). With nothing playing,
// capture delivers silence. This is replaced by SAI DMA in M3, where the
// SAI frame clock becomes the device clock and feedback is measured from it.

#include <string.h>

#include "bsp/board_api.h"
#include "tusb.h"
#include "usb_descriptors.h"

//--------------------------------------------------------------------+
// State
//--------------------------------------------------------------------+

static uint32_t const sample_rates[] = {48000, 96000, 192000};
#define N_SAMPLE_RATES TU_ARRAY_SIZE(sample_rates)

static volatile uint32_t current_sample_rate = 48000;
static volatile bool     play_active;
static volatile bool     rec_active;

// One millisecond of audio at the highest rate.
#define FRAME_BYTES     (AUDIO_N_CHANNELS * AUDIO_BYTES_PER_SAMPLE)
#define MAX_MS_FRAMES   (AUDIO_MAX_SAMPLE_RATE / 1000)
static int32_t loop_buf[MAX_MS_FRAMES * AUDIO_N_CHANNELS];

static void audio_task(void);
static void led_task(void);

//--------------------------------------------------------------------+
// Main
//--------------------------------------------------------------------+

int main(void) {
  board_init();

  tusb_rhport_init_t dev_init = {
    .role  = TUSB_ROLE_DEVICE,
    .speed = TUSB_SPEED_HIGH,
  };
  tusb_init(BOARD_TUD_RHPORT, &dev_init);

  board_init_after_tusb();

  while (1) {
    tud_task();
    audio_task();
    led_task();
  }
}

//--------------------------------------------------------------------+
// Audio data path (placeholder clock: SysTick, loopback OUT -> IN)
//--------------------------------------------------------------------+

static void audio_task(void) {
  static uint32_t last_ms;
  uint32_t const now = tusb_time_millis_api();

  // Process every elapsed millisecond so a slow main-loop pass does not
  // change the long-term consumption rate.
  while (last_ms != now) {
    last_ms++;

    uint32_t const frames = current_sample_rate / 1000;
    uint32_t const bytes  = frames * FRAME_BYTES;

    uint32_t got = 0;
    if (play_active) {
      got = tud_audio_read(loop_buf, (uint16_t) bytes);
    }
    if (got < bytes) {
      memset((uint8_t *) loop_buf + got, 0, bytes - got);
    }
    if (rec_active) {
      tud_audio_write(loop_buf, (uint16_t) bytes);
    }
  }
}

//--------------------------------------------------------------------+
// Clock source requests
//--------------------------------------------------------------------+

static bool rate_supported(uint32_t rate) {
  for (size_t i = 0; i < N_SAMPLE_RATES; i++) {
    if (sample_rates[i] == rate) return true;
  }
  return false;
}

static bool clock_get_request(uint8_t rhport, tusb_control_request_t const *req) {
  uint8_t const ctrl_sel = TU_U16_HIGH(req->wValue);

  if (ctrl_sel == AUDIO20_CS_CTRL_SAM_FREQ) {
    if (req->bRequest == AUDIO20_CS_REQ_CUR) {
      audio20_control_cur_4_t cur = {.bCur = (int32_t) tu_htole32(current_sample_rate)};
      return tud_audio_buffer_and_schedule_control_xfer(rhport, req, &cur, sizeof(cur));
    }
    if (req->bRequest == AUDIO20_CS_REQ_RANGE) {
      audio20_control_range_4_n_t(N_SAMPLE_RATES) range = {.wNumSubRanges = tu_htole16(N_SAMPLE_RATES)};
      for (size_t i = 0; i < N_SAMPLE_RATES; i++) {
        range.subrange[i].bMin = (int32_t) sample_rates[i];
        range.subrange[i].bMax = (int32_t) sample_rates[i];
        range.subrange[i].bRes = 0;
      }
      return tud_audio_buffer_and_schedule_control_xfer(rhport, req, &range, sizeof(range));
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
  current_sample_rate = rate;
  return true;
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
    play_active = (alt != 0);
  } else if (itf == ITF_NUM_AUDIO_STREAMING_IN) {
    if (alt != 0) tud_audio_clear_ep_in_ff(); // start capture with minimal latency
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
  // Placeholder until M3: feedback from OUT FIFO fill level against the
  // SysTick-paced consumer. M3 replaces this with measured SAI frame counts.
  feedback_param->method      = AUDIO_FEEDBACK_METHOD_FIFO_COUNT;
  feedback_param->sample_freq = current_sample_rate;
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
  uint32_t const interval = (play_active || rec_active) ? 50 : blink_ms;

  if (tusb_time_millis_api() - start_ms < interval) return;
  start_ms += interval;
  board_led_write(state);
  state = !state;
}
