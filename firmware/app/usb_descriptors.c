#include <string.h>

#include "bsp/board_api.h"
#include "tusb.h"
#include "usb_descriptors.h"

// Development IDs. 0xCAFE is TinyUSB's test VID: bench use only, replace before release.
#define USB_VID 0xCAFE
#define USB_PID 0x0AC2

enum {
  STRID_LANGID = 0,
  STRID_MANUFACTURER,
  STRID_PRODUCT,
  STRID_SERIAL,
  STRID_AUDIO,
  STRID_VENDOR,
};

//--------------------------------------------------------------------+
// Device
//--------------------------------------------------------------------+

static tusb_desc_device_t const desc_device = {
  .bLength            = sizeof(tusb_desc_device_t),
  .bDescriptorType    = TUSB_DESC_DEVICE,
  .bcdUSB             = 0x0200,
  // IAD is used for the audio function
  .bDeviceClass       = TUSB_CLASS_MISC,
  .bDeviceSubClass    = MISC_SUBCLASS_COMMON,
  .bDeviceProtocol    = MISC_PROTOCOL_IAD,
  .bMaxPacketSize0    = CFG_TUD_ENDPOINT0_SIZE,
  .idVendor           = USB_VID,
  .idProduct          = USB_PID,
  .bcdDevice          = 0x0001,
  .iManufacturer      = STRID_MANUFACTURER,
  .iProduct           = STRID_PRODUCT,
  .iSerialNumber      = STRID_SERIAL,
  .bNumConfigurations = 1,
};

uint8_t const *tud_descriptor_device_cb(void) {
  return (uint8_t const *) &desc_device;
}

//--------------------------------------------------------------------+
// Configuration (high speed)
//--------------------------------------------------------------------+

#define CONFIG_TOTAL_LEN (TUD_CONFIG_DESC_LEN + DIYUAC2_AUDIO_DESC_LEN + DIYUAC2_VENDOR_DESC_LEN)

// Self-powered: the board is not powered from the device port's VBUS.
static uint8_t const desc_configuration[] = {
  TUD_CONFIG_DESCRIPTOR(1, ITF_NUM_TOTAL, 0, CONFIG_TOTAL_LEN, TUSB_DESC_CONFIG_ATT_SELF_POWERED, 0),
  DIYUAC2_AUDIO_DESCRIPTOR(STRID_AUDIO),
  DIYUAC2_VENDOR_DESCRIPTOR(ITF_NUM_VENDOR, STRID_VENDOR),
};

TU_VERIFY_STATIC(sizeof(desc_configuration) == CONFIG_TOTAL_LEN, "Incorrect config descriptor size");

uint8_t const *tud_descriptor_configuration_cb(uint8_t index) {
  (void) index;
  return desc_configuration;
}

//--------------------------------------------------------------------+
// Full speed: not supported. Report an empty other-speed configuration
// so hosts see that the device has no function at full speed.
//--------------------------------------------------------------------+

static tusb_desc_device_qualifier_t const desc_device_qualifier = {
  .bLength            = sizeof(tusb_desc_device_qualifier_t),
  .bDescriptorType    = TUSB_DESC_DEVICE_QUALIFIER,
  .bcdUSB             = 0x0200,
  .bDeviceClass       = TUSB_CLASS_MISC,
  .bDeviceSubClass    = MISC_SUBCLASS_COMMON,
  .bDeviceProtocol    = MISC_PROTOCOL_IAD,
  .bMaxPacketSize0    = CFG_TUD_ENDPOINT0_SIZE,
  .bNumConfigurations = 1,
  .bReserved          = 0,
};

uint8_t const *tud_descriptor_device_qualifier_cb(void) {
  return (uint8_t const *) &desc_device_qualifier;
}

static uint8_t const desc_other_speed_config[] = {
  9, TUSB_DESC_OTHER_SPEED_CONFIG, U16_TO_U8S_LE(9), 0, 1, 0,
  TU_BIT(7) | TUSB_DESC_CONFIG_ATT_SELF_POWERED, 0,
};

uint8_t const *tud_descriptor_other_speed_configuration_cb(uint8_t index) {
  (void) index;
  return desc_other_speed_config;
}

//--------------------------------------------------------------------+
// Strings
//--------------------------------------------------------------------+

static char const *string_desc_arr[] = {
  [STRID_LANGID]       = (const char[]) {0x09, 0x04}, // English (0x0409)
  [STRID_MANUFACTURER] = "diyuac2",
  [STRID_PRODUCT]      = "diyuac2",
  [STRID_SERIAL]       = NULL, // chip unique ID
  [STRID_AUDIO]        = "diyuac2 Audio",
  [STRID_VENDOR]       = "diyuac2 Control",
};

static uint16_t desc_str[32 + 1];

uint16_t const *tud_descriptor_string_cb(uint8_t index, uint16_t langid) {
  (void) langid;
  size_t chr_count;

  switch (index) {
    case STRID_LANGID:
      memcpy(&desc_str[1], string_desc_arr[0], 2);
      chr_count = 1;
      break;

    case STRID_SERIAL:
      chr_count = board_usb_get_serial(desc_str + 1, 32);
      break;

    default: {
      if (index >= TU_ARRAY_SIZE(string_desc_arr)) return NULL;
      char const *str = string_desc_arr[index];
      chr_count = strlen(str);
      if (chr_count > 32) chr_count = 32;
      for (size_t i = 0; i < chr_count; i++) desc_str[1 + i] = (uint16_t) str[i];
      break;
    }
  }

  desc_str[0] = (uint16_t) ((TUSB_DESC_STRING << 8) | (2 * chr_count + 2));
  return desc_str;
}
