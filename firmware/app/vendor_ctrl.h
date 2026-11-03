// diyuac2 vendor control interface (draft protocol, see docs/vendor-protocol.md).

#ifndef VENDOR_CTRL_H_
#define VENDOR_CTRL_H_

#define VENDOR_PROTO_VERSION_MAJOR 0
#define VENDOR_PROTO_VERSION_MINOR 2

#define FW_VERSION_MAJOR 0
#define FW_VERSION_MINOR 2
#define FW_VERSION_PATCH 0

// bRequest codes
enum {
  VREQ_GET_INFO    = 0x00, // IN:  vendor_info_t
  VREQ_GET_SCRATCH = 0x01, // IN:  scratch buffer (transport test)
  VREQ_SET_SCRATCH = 0x02, // OUT: scratch buffer (transport test)
  VREQ_GET_STATS   = 0x03, // IN:  vendor_stats_t
  VREQ_CLEAR_STATS = 0x04, // OUT: no data, zero counters and lead min/max
  // Debug (0x10..): not part of the stable protocol.
  VREQ_DBG_CODEC_WRITE = 0x10, // OUT: no data, wValue = reg << 9 | val (WM8960)
};

#define VENDOR_SCRATCH_SIZE 32

typedef struct TU_ATTR_PACKED {
  uint8_t  proto_major;
  uint8_t  proto_minor;
  uint8_t  fw_major;
  uint8_t  fw_minor;
  uint8_t  fw_patch;
  uint8_t  reserved;
  uint16_t flags; // bit 0: stub firmware (commands beyond transport test not implemented)
} vendor_info_t;

// Audio path statistics (diagnostics, for soak tests).
enum {
  STATS_FLAG_CODEC_OK    = 1u << 0,
  STATS_FLAG_PLAY_ACTIVE = 1u << 1,
  STATS_FLAG_REC_ACTIVE  = 1u << 2,
  STATS_FLAG_PLAY_PRIMED = 1u << 3,
};

typedef struct TU_ATTR_PACKED {
  uint32_t rate;           // current sample rate, Hz
  uint32_t feedback;       // last feedback, 16.16 samples per microframe
  uint32_t play_underruns; // playback ring ran dry (silence played)
  uint32_t rec_overruns;   // capture ring overrun before data was queued
  uint32_t rec_drops;      // capture frames dropped, USB IN FIFO full
  uint32_t tx_fifo_errors; // SAI transmit FIFO underruns
  uint32_t rx_fifo_errors; // SAI receive FIFO overruns
  int32_t  play_lead_min;  // playback ring lead over DMA, frames
  int32_t  play_lead_max;
  uint16_t flags;          // STATS_FLAG_*
  uint16_t reserved;
} vendor_stats_t;

TU_VERIFY_STATIC(sizeof(vendor_stats_t) == 40, "vendor_stats_t layout");

// Provided by the audio path (main.c).
void audio_get_stats(vendor_stats_t *st);
void audio_clear_stats(void);

#endif
