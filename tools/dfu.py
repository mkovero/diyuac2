#!/usr/bin/env python3
"""diyuac2 firmware update over USB DFU 1.1 (dependency-free, Linux usbfs).

The app (cafe:0ac2) has a DFU runtime interface; DFU_DETACH restarts it into
the bootloader (cafe:0ac3), which takes the new image and resets into it.
Stock dfu-util does the same (`dfu-util -d cafe:0ac2,cafe:0ac3 -D app.bin`);
this script needs nothing but Python.

  dfu.py update <app.bin>   # app or bootloader -> bootloader -> new app
  dfu.py detach             # app -> bootloader (stays in DFU mode)
  dfu.py status             # bootloader: DFU status and state
  dfu.py leave              # bootloader -> app (if valid)

app.bin: `arm-none-eabi-objcopy -O binary target/thumbv7em-none-eabihf/release/diyuac2 app.bin`
(the image linked for flash offset 0x40000, vector table first).
Needs write access to the device nodes (root, or udev rules for both IDs).
"""

import ctypes
import fcntl
import os
import struct
import sys
import time

VID = 0xCAFE
PID_APP, PID_BOOT = 0x0AC2, 0x0AC3
ITF_APP_DFU, ITF_BOOT_DFU = 4, 0
TRANSFER_SIZE = 4096

DETACH, DNLOAD, GETSTATUS, CLRSTATUS, GETSTATE, ABORT = 0, 1, 3, 4, 5, 6
RT_OUT = 0x21  # class, interface
RT_IN = 0xA1

STATES = {0: "appIDLE", 1: "appDETACH", 2: "dfuIDLE", 3: "dfuDNLOAD-SYNC", 4: "dfuDNBUSY",
          5: "dfuDNLOAD-IDLE", 6: "dfuMANIFEST-SYNC", 7: "dfuMANIFEST", 8: "dfuMANIFEST-WAIT-RESET",
          9: "dfuUPLOAD-IDLE", 10: "dfuERROR"}
STATUSES = {0: "OK", 1: "errTARGET", 2: "errFILE", 3: "errWRITE", 4: "errERASE", 5: "errCHECK_ERASED",
            6: "errPROG", 7: "errVERIFY", 8: "errADDRESS", 9: "errNOTDONE", 10: "errFIRMWARE",
            11: "errVENDOR", 12: "errUSBR", 13: "errPOR", 14: "errUNKNOWN", 15: "errSTALLEDPKT"}


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


def _io(nr):
    return (ord("U") << 8) | nr


def _iowr(nr, size):
    return (3 << 30) | (size << 16) | (ord("U") << 8) | nr


USBDEVFS_CONTROL = _iowr(0, ctypes.sizeof(CtrlTransfer))
USBDEVFS_RESET = _io(20)


def find(pid):
    base = "/sys/bus/usb/devices"
    for name in os.listdir(base):
        p = os.path.join(base, name)
        try:
            vid = int(open(os.path.join(p, "idVendor")).read(), 16)
            dpid = int(open(os.path.join(p, "idProduct")).read(), 16)
            if (vid, dpid) != (VID, pid):
                continue
            bus = int(open(os.path.join(p, "busnum")).read())
            dev = int(open(os.path.join(p, "devnum")).read())
        except (FileNotFoundError, ValueError):
            continue
        return f"/dev/bus/usb/{bus:03d}/{dev:03d}"
    return None


def wait_for(pid, secs):
    end = time.monotonic() + secs
    while time.monotonic() < end:
        path = find(pid)
        if path:
            # udev may still be applying permissions
            for _ in range(50):
                try:
                    return os.open(path, os.O_RDWR)
                except PermissionError:
                    time.sleep(0.1)
                except FileNotFoundError:
                    break
        time.sleep(0.1)
    sys.exit(f"device {VID:04x}:{pid:04x} did not appear within {secs} s")


def control(fd, rtype, req, itf, value=0, data=b"", length=0, timeout=5000):
    if rtype & 0x80:
        buf = ctypes.create_string_buffer(length)
    else:
        buf = ctypes.create_string_buffer(bytes(data), max(len(data), 1))
        length = len(data)
    xfer = CtrlTransfer(rtype, req, value, itf, length, timeout, ctypes.cast(buf, ctypes.c_void_p))
    n = fcntl.ioctl(fd, USBDEVFS_CONTROL, xfer)
    return buf.raw[:n] if rtype & 0x80 else n


def get_status(fd):
    raw = control(fd, RT_IN, GETSTATUS, ITF_BOOT_DFU, length=6)
    status, t0, t1, t2, state, _ = struct.unpack("<6B", raw)
    return status, state, t0 | t1 << 8 | t2 << 16


def fmt(status, state):
    return f"{STATES.get(state, state)}, {STATUSES.get(status, status)}"


def detach():
    """App -> bootloader. Returns an open fd of the bootloader."""
    path = find(PID_APP)
    if path:
        fd = os.open(path, os.O_RDWR)
        try:
            control(fd, RT_OUT, DETACH, ITF_APP_DFU, value=1000)
        finally:
            os.close(fd)
        print("app: DETACH sent")
    elif not find(PID_BOOT):
        sys.exit("no diyuac2 app or bootloader found")
    return wait_for(PID_BOOT, 10)


def clear_error(fd):
    status, state, _ = get_status(fd)
    if state == 10:
        control(fd, RT_OUT, CLRSTATUS, ITF_BOOT_DFU)
    elif state not in (2,):
        control(fd, RT_OUT, ABORT, ITF_BOOT_DFU)
    status, state, _ = get_status(fd)
    if state != 2:
        sys.exit(f"bootloader not idle: {fmt(status, state)}")


def poll_until(fd, done):
    while True:
        status, state, timeout = get_status(fd)
        if status != 0 or state == 10:
            sys.exit(f"bootloader: {fmt(status, state)}")
        if done(state):
            return state
        time.sleep(timeout / 1000)


def update(image_path):
    image = open(image_path, "rb").read()
    if len(image) < 8:
        sys.exit("image too short")
    sp, pc = struct.unpack("<2I", image[:8])
    if not (0x6004_0000 <= pc < 0x6004_0000 + len(image)):
        sys.exit(f"{image_path}: reset vector 0x{pc:08x} is not in an image at 0x60040000 (wrong link address?)")
    fd = detach()
    try:
        clear_error(fd)
        blocks = (len(image) + TRANSFER_SIZE - 1) // TRANSFER_SIZE
        t0 = time.monotonic()
        for i in range(blocks):
            chunk = image[i * TRANSFER_SIZE:(i + 1) * TRANSFER_SIZE]
            control(fd, RT_OUT, DNLOAD, ITF_BOOT_DFU, value=i, data=chunk)
            poll_until(fd, lambda s: s == 5)
            print(f"\rdownload {i + 1}/{blocks} blocks", end="", flush=True)
        dt = time.monotonic() - t0
        print(f"\rdownloaded {len(image)} bytes in {dt:.1f} s")
        control(fd, RT_OUT, DNLOAD, ITF_BOOT_DFU, value=blocks)
        state = poll_until(fd, lambda s: s in (7, 8, 2))
        print(f"manifest: {STATES[state]}")
        if state == 7:
            time.sleep(0.1)
            state = poll_until(fd, lambda s: s in (8, 2))
        try:
            fcntl.ioctl(fd, USBDEVFS_RESET, 0)
        except OSError:
            pass  # the device resets itself; the bus reset may race it
    finally:
        os.close(fd)
    fd = wait_for(PID_APP, 15)
    os.close(fd)
    print("app is back")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    cmd = sys.argv[1]
    if cmd == "update" and len(sys.argv) == 3:
        update(sys.argv[2])
    elif cmd == "detach":
        os.close(detach())
        print("bootloader is up")
    elif cmd == "status":
        fd = wait_for(PID_BOOT, 1)
        status, state, _ = get_status(fd)
        print(fmt(status, state))
    elif cmd == "leave":
        fd = wait_for(PID_BOOT, 1)
        control(fd, RT_OUT, DETACH, ITF_BOOT_DFU, value=0)
        os.close(fd)
        os.close(wait_for(PID_APP, 15))
        print("app is back")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
