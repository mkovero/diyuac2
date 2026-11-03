// diyuac2 vendor control interface: EP0 vendor requests addressed to ITF_NUM_VENDOR.

#include <string.h>

#include "tusb.h"
#include "usb_descriptors.h"
#include "vendor_ctrl.h"
#include "codec.h"

static uint8_t scratch[VENDOR_SCRATCH_SIZE];
static uint8_t scratch_rx[VENDOR_SCRATCH_SIZE];

static vendor_info_t const info = {
  .proto_major = VENDOR_PROTO_VERSION_MAJOR,
  .proto_minor = VENDOR_PROTO_VERSION_MINOR,
  .fw_major    = FW_VERSION_MAJOR,
  .fw_minor    = FW_VERSION_MINOR,
  .fw_patch    = FW_VERSION_PATCH,
  .reserved    = 0,
  .flags       = 0x0001, // stub
};

// TinyUSB routes every vendor-type control request here, any recipient.
bool tud_vendor_control_xfer_cb(uint8_t rhport, uint8_t stage, tusb_control_request_t const *req) {
  // Only requests addressed to our vendor interface are handled; others stall.
  if (req->bmRequestType_bit.recipient != TUSB_REQ_RCPT_INTERFACE ||
      tu_u16_low(req->wIndex) != ITF_NUM_VENDOR) {
    return false;
  }

  bool const dir_in = req->bmRequestType_bit.direction == TUSB_DIR_IN;

  switch (req->bRequest) {
    case VREQ_GET_INFO:
      if (!dir_in) return false;
      if (stage == CONTROL_STAGE_SETUP) {
        return tud_control_xfer(rhport, req, (void *) (uintptr_t) &info, sizeof(info));
      }
      return true;

    case VREQ_GET_SCRATCH:
      if (!dir_in) return false;
      if (stage == CONTROL_STAGE_SETUP) {
        return tud_control_xfer(rhport, req, scratch, sizeof(scratch));
      }
      return true;

    case VREQ_SET_SCRATCH:
      if (dir_in || req->wLength > sizeof(scratch_rx)) return false;
      if (stage == CONTROL_STAGE_SETUP) {
        return tud_control_xfer(rhport, req, scratch_rx, req->wLength);
      }
      if (stage == CONTROL_STAGE_DATA) {
        memset(scratch, 0, sizeof(scratch));
        memcpy(scratch, scratch_rx, req->wLength);
      }
      return true;

    case VREQ_GET_STATS:
      if (!dir_in) return false;
      if (stage == CONTROL_STAGE_SETUP) {
        static vendor_stats_t st;
        audio_get_stats(&st);
        return tud_control_xfer(rhport, req, &st, sizeof(st));
      }
      return true;

    case VREQ_CLEAR_STATS:
      if (dir_in || req->wLength != 0) return false;
      if (stage == CONTROL_STAGE_SETUP) {
        audio_clear_stats();
        return tud_control_status(rhport, req);
      }
      return true;

    case VREQ_DBG_CODEC_WRITE:
      if (dir_in || req->wLength != 0) return false;
      if (stage == CONTROL_STAGE_SETUP) {
        if (!codec_write_reg(req->wValue >> 9, req->wValue & 0x1FF)) return false;
        return tud_control_status(rhport, req);
      }
      return true;

    default:
      return false;
  }
}
