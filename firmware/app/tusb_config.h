#ifndef TUSB_CONFIG_H_
#define TUSB_CONFIG_H_

#ifdef __cplusplus
extern "C" {
#endif

//--------------------------------------------------------------------+
// Board / common
//--------------------------------------------------------------------+

#ifndef BOARD_TUD_RHPORT
#define BOARD_TUD_RHPORT      0
#endif

// High speed is mandatory for this device (full-speed fallback is a bug).
#define BOARD_TUD_MAX_SPEED   OPT_MODE_HIGH_SPEED

#ifndef CFG_TUSB_MCU
#error CFG_TUSB_MCU must be defined
#endif

#ifndef CFG_TUSB_OS
#define CFG_TUSB_OS           OPT_OS_NONE
#endif

// Keep logging off by default: it disturbs the isochronous streams.
#ifndef CFG_TUSB_DEBUG
#define CFG_TUSB_DEBUG        0
#endif

#define CFG_TUD_ENABLED       1
#define CFG_TUD_MAX_SPEED     BOARD_TUD_MAX_SPEED

#ifndef CFG_TUSB_MEM_SECTION
#define CFG_TUSB_MEM_SECTION
#endif

#ifndef CFG_TUSB_MEM_ALIGN
#define CFG_TUSB_MEM_ALIGN    __attribute__ ((aligned(4)))
#endif

#define CFG_TUD_ENDPOINT0_SIZE 64

//--------------------------------------------------------------------+
// Classes
//--------------------------------------------------------------------+

#define CFG_TUD_AUDIO         1
#define CFG_TUD_VENDOR        1 // claims the endpoint-less control interface
#define CFG_TUD_CDC           0
#define CFG_TUD_MSC           0
#define CFG_TUD_HID           0
#define CFG_TUD_MIDI          0

// Vendor interface has no endpoints; keep the class driver's buffers minimal.
#define CFG_TUD_VENDOR_TXRX_BUFFERED 0
#define CFG_TUD_VENDOR_RX_BUFSIZE    0
#define CFG_TUD_VENDOR_TX_BUFSIZE    0
#define CFG_TUD_VENDOR_EPSIZE        64

//--------------------------------------------------------------------+
// Audio class
//--------------------------------------------------------------------+

#include "usb_descriptors.h"

// Class control buffer: sample-rate RANGE reply is 2 + 3 * 12 = 38 bytes.
#define CFG_TUD_AUDIO_CTRL_BUF_SZ            64

// Playback (host -> device)
#define CFG_TUD_AUDIO_ENABLE_EP_OUT          1
#define CFG_TUD_AUDIO_ENABLE_FEEDBACK_EP     1
#define CFG_TUD_AUDIO_FUNC_1_EP_OUT_SZ_MAX   AUDIO_EP_SIZE
// FIFO-count feedback needs >= 4x EP size; app consumes once per ms (8 uframes).
#define CFG_TUD_AUDIO_FUNC_1_EP_OUT_SW_BUF_SZ (32 * AUDIO_EP_SIZE)

// Capture (device -> host)
#define CFG_TUD_AUDIO_ENABLE_EP_IN           1
#define CFG_TUD_AUDIO_EP_IN_FLOW_CONTROL     1
#define CFG_TUD_AUDIO_FUNC_1_EP_IN_SZ_MAX    AUDIO_EP_SIZE
#define CFG_TUD_AUDIO_FUNC_1_EP_IN_SW_BUF_SZ (32 * AUDIO_EP_SIZE)

#ifdef __cplusplus
}
#endif

#endif
