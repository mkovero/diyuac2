// diyuac2 playback rate feedback, measured from the audio interface.
//
// In the USB SOF interrupt (via TinyUSB's tud_audio_feedback_interval_isr)
// the number of words the SAI transmitter has actually shifted out is
// sampled together with the USB microframe number. Feedback is the
// difference over a sliding window of about one second:
//
//   fb (16.16 samples per microframe) = d_words / 2 * 65536 / d_microframes
//
// The window slides by one 2 ms step at a time, so consecutive values are
// independent of each other's quantisation: the host's running total of
// requested samples follows the device's consumed total within one window.
//
// Startup: the window grows from 16 ms to 1024 ms after reset; before that
// the nominal value is sent.

#include "tusb.h"

#include "audio_hw.h"
#include "feedback.h"

#define UF_MASK      0x3FFFu // FRINDEX: 14-bit microframe counter
#define STEP_UF      16u     // sample every 2 ms
#define HIST         512u    // 512 * 2 ms = 1024 ms window
#define MIN_WIN_UF   128u    // first measured value after 16 ms
#define MAX_GAP_UF   256u    // SOF gap (suspend, bus reset): restart
#define CLAMP_PPM    1000u   // limit to nominal +- 0.1 %

typedef struct {
  uint16_t uf;
  uint32_t words;
} snap_t;

static snap_t   hist[HIST];
static uint32_t n_hist;
static uint32_t head;
static uint16_t last_uf;

static volatile bool     reset_req = true;
static volatile uint32_t fb_nom;
static volatile uint32_t fb_last;

uint32_t feedback_nominal(uint32_t rate) {
  return (uint32_t) (((uint64_t) rate << 16) / 8000u);
}

void feedback_reset(uint32_t rate) {
  fb_nom    = feedback_nominal(rate);
  fb_last   = fb_nom;
  reset_req = true;
}

uint32_t feedback_last(void) { return fb_last; }

// Called by TinyUSB from the SOF interrupt while the OUT stream is active,
// with frame_number = FRINDEX (microframes).
void tud_audio_feedback_interval_isr(uint8_t func_id, uint32_t frame_number, uint8_t interval_shift) {
  (void) interval_shift;
  uint16_t const uf = (uint16_t) (frame_number & UF_MASK);

  if (reset_req) {
    reset_req = false;
    n_hist    = 0;
    head      = 0;
    tud_audio_n_fb_set(func_id, fb_nom);
  } else {
    uint16_t const gap = (uint16_t) ((uf - last_uf) & UF_MASK);
    if (gap < STEP_UF) return;
    if (gap > MAX_GAP_UF) n_hist = head = 0;
  }

  uint32_t const words = audio_hw_tx_words_played();
  last_uf = uf;

  if (n_hist > 0) {
    snap_t const *old = &hist[n_hist < HIST ? 0 : head];
    uint32_t const d_uf = (uf - old->uf) & UF_MASK;
    if (d_uf >= MIN_WIN_UF) {
      uint32_t fb = (uint32_t) (((uint64_t) (words - old->words) << 15) / d_uf);
      uint32_t const lim = (uint32_t) (((uint64_t) fb_nom * CLAMP_PPM) / 1000000u);
      if (fb > fb_nom + lim) fb = fb_nom + lim;
      if (fb < fb_nom - lim) fb = fb_nom - lim;
      fb_last = fb;
      tud_audio_n_fb_set(func_id, fb);
    }
  }

  hist[head] = (snap_t) {.uf = uf, .words = words};
  head       = (head + 1) % HIST;
  if (n_hist < HIST) n_hist++;
}
