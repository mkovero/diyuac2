#!/usr/bin/env python3
"""diyuac2 vendor control interface client (draft protocol v0.3).

Dependency-free: talks to the device through Linux usbfs ioctls.
Needs write access to the device node (root, or a udev rule, see docs/vendor-protocol.md).

  vendorctl.py info
  vendorctl.py scratch-write <hex bytes>
  vendorctl.py scratch-read
  vendorctl.py selftest
  vendorctl.py stats [interval_s]     # once, or repeatedly
  vendorctl.py clear-stats
  vendorctl.py codec-write <reg> <val>  # debug, WM8960: 7-bit reg, 9-bit value (hex ok)
  vendorctl.py loopback on|off      # debug: capture returns the played data (TX ring)
  vendorctl.py isr [interval_s]       # debug: USB ISR cycle profile (clear-stats resets it)
"""

import ctypes
import fcntl
import os
import struct
import sys
import time

VID, PID = 0xCAFE, 0x0AC2
ITF_VENDOR = 3

VREQ_GET_INFO = 0x00
VREQ_GET_SCRATCH = 0x01
VREQ_SET_SCRATCH = 0x02
VREQ_GET_STATS = 0x03
VREQ_CLEAR_STATS = 0x04
VREQ_DBG_CODEC_WRITE = 0x10
VREQ_DBG_ISR_STATS = 0x11
VREQ_DBG_LOOPBACK = 0x12
SCRATCH_SIZE = 32

STATS_FMT = "<9I2H5I"  # vendor_stats_t, 60 bytes (40 before Rust firmware 0.4.1)
STATS_FMT_OLD = "<9I2H"
STATS_FIELDS = ("rate", "feedback", "play_underruns", "rec_overruns", "rec_drops",
                "tx_fifo_errors", "rx_fifo_errors", "play_lead_min", "play_lead_max",
                "flags", "reserved",
                "usb_out_errors", "usb_in_errors", "usb_fb_errors", "usb_out_missed", "usb_in_missed")
FLAG_NAMES = ("codec_ok", "play", "rec", "primed", "codec_out_of_spec", "loopback", "gain", "tdm16")

# bmRequestType: vendor, recipient interface
RT_IN = 0x80 | 0x40 | 0x01
RT_OUT = 0x00 | 0x40 | 0x01


class CtrlTransfer(ctypes.Structure):
    _fields_ = [
        ("bRequestType", ctypes.c_uint8),
        ("bRequest", ctypes.c_uint8),
        ("wValue", ctypes.c_uint16),
        ("wIndex", ctypes.c_uint16),
        ("wLength", ctypes.c_uint16),
        ("timeout", ctypes.c_uint32),
        ("data", ctypes.c_void_p),
    ]


def _iowr(typ, nr, size):
    return (3 << 30) | (size << 16) | (ord(typ) << 8) | nr


USBDEVFS_CONTROL = _iowr("U", 0, ctypes.sizeof(CtrlTransfer))


def find_device():
    base = "/sys/bus/usb/devices"
    for name in os.listdir(base):
        p = os.path.join(base, name)
        try:
            vid = int(open(os.path.join(p, "idVendor")).read(), 16)
            pid = int(open(os.path.join(p, "idProduct")).read(), 16)
        except (FileNotFoundError, ValueError):
            continue
        if (vid, pid) == (VID, PID):
            bus = int(open(os.path.join(p, "busnum")).read())
            dev = int(open(os.path.join(p, "devnum")).read())
            return f"/dev/bus/usb/{bus:03d}/{dev:03d}"
    sys.exit(f"device {VID:04x}:{PID:04x} not found")


def control(fd, rtype, req, length=0, data=b"", value=0):
    if rtype & 0x80:
        buf = ctypes.create_string_buffer(length)
    else:
        buf = ctypes.create_string_buffer(bytes(data), len(data))
        length = len(data)
    xfer = CtrlTransfer(rtype, req, value, ITF_VENDOR, length, 1000, ctypes.cast(buf, ctypes.c_void_p))
    n = fcntl.ioctl(fd, USBDEVFS_CONTROL, xfer)
    return buf.raw[:n] if rtype & 0x80 else n


def get_info(fd):
    raw = control(fd, RT_IN, VREQ_GET_INFO, 8)
    pmaj, pmin, fmaj, fmin, fpat, _res, flags = struct.unpack("<6BH", raw)
    return {"protocol": f"{pmaj}.{pmin}", "firmware": f"{fmaj}.{fmin}.{fpat}", "flags": flags}


def get_stats(fd):
    raw = control(fd, RT_IN, VREQ_GET_STATS, struct.calcsize(STATS_FMT))
    fmt = STATS_FMT if len(raw) == struct.calcsize(STATS_FMT) else STATS_FMT_OLD
    st = dict(zip(STATS_FIELDS, struct.unpack(fmt, raw)))
    for k in ("play_lead_min", "play_lead_max"):
        if st[k] >= 1 << 31:
            st[k] -= 1 << 32
    return st


def format_stats(st):
    fb = st["feedback"]
    hz = fb * 8000 / 65536
    flags = ",".join(n for i, n in enumerate(FLAG_NAMES) if st["flags"] & (1 << i)) or "-"
    return (f"rate {st['rate']} fb 0x{fb:08x} = {hz:.3f} Hz ({hz - st['rate']:+.3f}) "
            f"underruns {st['play_underruns']} rec_overruns {st['rec_overruns']} "
            f"rec_drops {st['rec_drops']} sai_fifo_err tx {st['tx_fifo_errors']} rx {st['rx_fifo_errors']} "
            f"lead {st['play_lead_min']}..{st['play_lead_max']} [{flags}]"
            + (f" usb err out {st['usb_out_errors']} in {st['usb_in_errors']} fb {st['usb_fb_errors']}"
               f" missed out {st['usb_out_missed']} in {st['usb_in_missed']}" if "usb_out_errors" in st else ""))


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    cmd = sys.argv[1]
    fd = os.open(find_device(), os.O_RDWR)
    try:
        if cmd == "info":
            info = get_info(fd)
            names = [n for bit, n in ((1, "stub"), (2, "FULL SPEED: no audio")) if info["flags"] & bit]
            tag = f" ({', '.join(names)})" if names else ""
            print(f"protocol {info['protocol']}, firmware {info['firmware']}, flags 0x{info['flags']:04x}{tag}")
        elif cmd == "scratch-write":
            data = bytes.fromhex("".join(sys.argv[2:]))
            print(f"wrote {control(fd, RT_OUT, VREQ_SET_SCRATCH, data=data)} bytes")
        elif cmd == "scratch-read":
            print(control(fd, RT_IN, VREQ_GET_SCRATCH, SCRATCH_SIZE).hex(" "))
        elif cmd == "selftest":
            print("info:", get_info(fd))
            pattern = os.urandom(SCRATCH_SIZE)
            control(fd, RT_OUT, VREQ_SET_SCRATCH, data=pattern)
            back = control(fd, RT_IN, VREQ_GET_SCRATCH, SCRATCH_SIZE)
            print("scratch round trip:", "OK" if back == pattern else f"MISMATCH {back.hex()} != {pattern.hex()}")
            try:
                control(fd, RT_IN, 0x7F, 8)
                print("unknown request: NOT stalled (bad)")
            except OSError as e:
                print(f"unknown request: stalled as expected ({e.strerror})")
        elif cmd == "stats":
            interval = float(sys.argv[2]) if len(sys.argv) > 2 else 0
            while True:
                print(time.strftime("%H:%M:%S"), format_stats(get_stats(fd)), flush=True)
                if not interval:
                    break
                time.sleep(interval)
        elif cmd == "codec-write":
            reg, val = int(sys.argv[2], 0), int(sys.argv[3], 0)
            if reg > 0x7F or val > 0x1FF:
                sys.exit("reg <= 0x7f, val <= 0x1ff")
            control(fd, RT_OUT, VREQ_DBG_CODEC_WRITE, value=(reg << 9) | val)
            print(f"R{reg:#04x} = {val:#05x}")
        elif cmd == "isr":
            interval = float(sys.argv[2]) if len(sys.argv) > 2 else 0
            while True:
                raw = control(fd, RT_IN, VREQ_DBG_ISR_STATS, 40)
                n, mx, sof, xfer, setup, busy, elapsed, hz = struct.unpack("<5IQQI", raw)
                us = lambda c: c * 1e6 / hz
                mean = us(busy / n) if n else 0
                load = 100 * busy / elapsed if elapsed else 0
                print(time.strftime("%H:%M:%S"),
                      f"isr n {n} mean {mean:.2f} us max {us(mx):.2f} us "
                      f"(sof {us(sof):.2f} xfer {us(xfer):.2f} setup {us(setup):.2f}) "
                      f"load {load:.2f}%", flush=True)
                if not interval:
                    break
                time.sleep(interval)
        elif cmd == "loopback":
            on = sys.argv[2] == "on"
            control(fd, RT_OUT, VREQ_DBG_LOOPBACK, value=int(on))
            print("loopback", "on" if on else "off")
        elif cmd == "clear-stats":
            control(fd, RT_OUT, VREQ_CLEAR_STATS)
            print("cleared")
        else:
            sys.exit(__doc__)
    finally:
        os.close(fd)


if __name__ == "__main__":
    main()
