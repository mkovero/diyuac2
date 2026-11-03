// diyuac2 vendor control interface (draft protocol, see docs/vendor-protocol.md).

#ifndef VENDOR_CTRL_H_
#define VENDOR_CTRL_H_

#define VENDOR_PROTO_VERSION_MAJOR 0
#define VENDOR_PROTO_VERSION_MINOR 1

#define FW_VERSION_MAJOR 0
#define FW_VERSION_MINOR 1
#define FW_VERSION_PATCH 0

// bRequest codes
enum {
  VREQ_GET_INFO    = 0x00, // IN:  vendor_info_t
  VREQ_GET_SCRATCH = 0x01, // IN:  scratch buffer (transport test)
  VREQ_SET_SCRATCH = 0x02, // OUT: scratch buffer (transport test)
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

#endif
