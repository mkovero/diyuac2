#!/usr/bin/env python3
"""Minimal UAC2 host for diyuac2 over Linux usbfs: abuse tests.

Detaches snd-usb-audio, claims all interfaces and streams iso itself:
playback paced by the device's feedback, capture, feedback. With the Rust
firmware's debug loopback on, capture must return the playback data: frames
carry a counter (left = n, right = ~n), so any drop, repeat or corruption
shows as a discontinuity.

On top of the stream it does what snd-usb-audio never does: SET_CUR rate
changes while streaming, endpoint halt requests, odd OUT packet sizes,
alt-setting storms, malformed control requests.

  usbhost.py [test...]     tests: stream ratechange halt oddsize altstorm control all
  RATE=192000 SECS=10      stream rate and length of the plain stream test

Needs root. The kernel driver is re-attached at exit. DEBUG=1 prints the
first discontinuities.

Run it on bare metal. Through QEMU usb-host passthrough (libvirt hostdev)
the iso IN data arrives corrupted (missing frames, stale frames from
earlier transfers, zero-filled gaps) while the same firmware is clean on
the host. Larger usb-host iso buffers (isobufs=32, isobsize=64) help but
still lose about one frame per 0.2 s.
"""
import ctypes, errno, fcntl, os, struct, sys, time

VID, PID = 0xCAFE, 0x0AC2
ITF_AC, ITF_OUT, ITF_IN, ITF_VENDOR = 0, 1, 2, 3
EP_OUT, EP_FB, EP_IN = 0x01, 0x81, 0x82
EP_SIZE, FRAME = 200, 8
PKTS = 8                 # iso packets per URB: 1 ms
DEPTH = 4                # URBs in flight per data endpoint

libc = ctypes.CDLL(None, use_errno=True)


def _ioc(d, nr, size):
    return (d << 30) | (size << 16) | (ord("U") << 8) | nr


class IsoDesc(ctypes.Structure):
    _fields_ = [("length", ctypes.c_uint), ("actual_length", ctypes.c_uint), ("status", ctypes.c_uint)]


class UrbBase(ctypes.Structure):
    _fields_ = [("type", ctypes.c_ubyte), ("endpoint", ctypes.c_ubyte), ("status", ctypes.c_int),
                ("flags", ctypes.c_uint), ("buffer", ctypes.c_void_p), ("buffer_length", ctypes.c_int),
                ("actual_length", ctypes.c_int), ("start_frame", ctypes.c_int),
                ("number_of_packets", ctypes.c_int), ("error_count", ctypes.c_int),
                ("signr", ctypes.c_uint), ("usercontext", ctypes.c_void_p)]


class Urb(ctypes.Structure):
    _fields_ = UrbBase._fields_ + [("iso", IsoDesc * PKTS)]


class Ctrl(ctypes.Structure):
    _fields_ = [("bRequestType", ctypes.c_uint8), ("bRequest", ctypes.c_uint8), ("wValue", ctypes.c_uint16),
                ("wIndex", ctypes.c_uint16), ("wLength", ctypes.c_uint16), ("timeout", ctypes.c_uint32),
                ("data", ctypes.c_void_p)]


class IoctlReq(ctypes.Structure):
    _fields_ = [("ifno", ctypes.c_int), ("ioctl_code", ctypes.c_int), ("data", ctypes.c_void_p)]


CONTROL = _ioc(3, 0, ctypes.sizeof(Ctrl))
SETINTERFACE = _ioc(2, 4, 8)
SUBMITURB = _ioc(2, 10, ctypes.sizeof(UrbBase))
DISCARDURB = _ioc(0, 11, 0)
REAPURB = _ioc(1, 12, 8)
REAPURBNDELAY = _ioc(1, 13, 8)
CLAIMINTERFACE = _ioc(2, 15, 4)
RELEASEINTERFACE = _ioc(2, 16, 4)
IOCTL = _ioc(3, 18, ctypes.sizeof(IoctlReq))
CLEAR_HALT = _ioc(2, 21, 4)
DISCONNECT = _ioc(0, 22, 0)
CONNECT = _ioc(0, 23, 0)
URB_TYPE_ISO, URB_ISO_ASAP = 0, 2


def ioctl(fd, req, arg):
    if libc.ioctl(fd, ctypes.c_ulong(req), arg) < 0:
        e = ctypes.get_errno()
        raise OSError(e, os.strerror(e))


def find_device():
    base = "/sys/bus/usb/devices"
    for name in os.listdir(base):
        p = os.path.join(base, name)
        try:
            if (int(open(p + "/idVendor").read(), 16), int(open(p + "/idProduct").read(), 16)) == (VID, PID):
                return f"/dev/bus/usb/{int(open(p + '/busnum').read()):03d}/{int(open(p + '/devnum').read()):03d}"
        except (FileNotFoundError, ValueError):
            pass
    sys.exit("device not found")


class Dev:
    def __init__(self):
        self.fd = os.open(find_device(), os.O_RDWR)
        for itf in (ITF_AC, ITF_OUT, ITF_IN, ITF_VENDOR):
            try:
                ioctl(self.fd, IOCTL, ctypes.byref(IoctlReq(itf, DISCONNECT, None)))
            except OSError:
                pass                                  # no driver bound
            ioctl(self.fd, CLAIMINTERFACE, ctypes.byref(ctypes.c_uint(itf)))

    def close(self):
        for itf in (ITF_AC, ITF_OUT, ITF_IN, ITF_VENDOR):
            try:
                ioctl(self.fd, RELEASEINTERFACE, ctypes.byref(ctypes.c_uint(itf)))
                ioctl(self.fd, IOCTL, ctypes.byref(IoctlReq(itf, CONNECT, None)))
            except OSError:
                pass
        os.close(self.fd)

    def control(self, rtype, req, value, index, data=b"", length=None):
        """Returns IN data, OUT length, or raises OSError (EPIPE = stall)."""
        if rtype & 0x80:
            buf = ctypes.create_string_buffer(length)
            n = len(buf)
        else:
            buf = ctypes.create_string_buffer(bytes(data), max(len(data), 1))
            n = len(data)
        c = Ctrl(rtype, req, value, index, n, 1000, ctypes.cast(buf, ctypes.c_void_p))
        r = libc.ioctl(self.fd, ctypes.c_ulong(CONTROL), ctypes.byref(c))
        if r < 0:
            e = ctypes.get_errno()
            raise OSError(e, os.strerror(e))
        return buf.raw[:r] if rtype & 0x80 else r

    def set_interface(self, itf, alt):
        ioctl(self.fd, SETINTERFACE, ctypes.byref((ctypes.c_uint * 2)(itf, alt)))

    def set_rate(self, rate):
        return self.control(0x21, 0x01, 0x0100, 0x0400, struct.pack("<I", rate))

    def get_rate(self):
        return struct.unpack("<I", self.control(0xA1, 0x01, 0x0100, 0x0400, length=4))[0]

    def vendor_in(self, req, length):
        return self.control(0xC1, req, 0, ITF_VENDOR, length=length)

    def vendor_out(self, req, value=0):
        return self.control(0x41, req, value, ITF_VENDOR)

    def stats(self):
        v = struct.unpack("<II5I2iHH", self.vendor_in(0x03, 40))
        return dict(rate=v[0], fb=v[1], underruns=v[2], rec_overruns=v[3], rec_drops=v[4],
                    tx_err=v[5], rx_err=v[6], lead=(v[7], v[8]), flags=v[9])

    def alive(self):
        try:
            return len(self.vendor_in(0x00, 8)) == 8
        except OSError:
            return False


class Stream:
    """Playback (feedback paced, counter pattern) + capture (checked) + feedback."""

    def __init__(self, dev, rate, odd=None):
        self.dev, self.rate = dev, rate
        self.fb = (rate << 16) // 8000
        self.acc = 0
        self.n_out = 0                 # next frame counter to send
        self.odd = odd                 # callable(packet index) -> extra junk bytes / None
        self.pkt_index = 0
        self.expect = None             # next counter expected in capture
        self.frames_in = 0
        self.disc = []                 # (time, got, expected)
        self.fb_values = []
        self.urbs = {}
        self.t0 = time.monotonic()
        self.active = {EP_OUT: True, EP_FB: True, EP_IN: True}
        self.submit_errors = 0
        self.packet_errors = 0
        self.zeros_mid = 0

    def _new_urb(self, ep, sizes, payload=None):
        urb = Urb()
        total = sum(sizes)
        buf = ctypes.create_string_buffer(payload if payload is not None else total, total) if total else \
            ctypes.create_string_buffer(1)
        urb.type, urb.endpoint, urb.flags = URB_TYPE_ISO, ep, URB_ISO_ASAP
        urb.buffer, urb.buffer_length = ctypes.cast(buf, ctypes.c_void_p), total
        urb.number_of_packets = len(sizes)
        for i, s in enumerate(sizes):
            urb.iso[i].length = s
        key = id(urb)
        urb.usercontext = key
        self.urbs[key] = (urb, buf)
        try:
            ioctl(self.dev.fd, SUBMITURB, ctypes.byref(urb))
        except OSError:
            self.submit_errors += 1
            del self.urbs[key]

    def _out_urb(self):
        sizes, data = [], bytearray()
        for _ in range(len(Urb().iso)):
            self.acc += self.fb
            n = self.acc >> 16
            self.acc -= n << 16
            n = min(n, EP_SIZE // FRAME)
            pkt = bytearray()
            for _ in range(n):
                pkt += struct.pack("<II", self.n_out & 0xFFFFFFFF, ~self.n_out & 0xFFFFFFFF)
                self.n_out += 1
            if self.odd:
                extra = self.odd(self.pkt_index)
                if extra is not None:
                    if extra < 0:
                        # Zero-length packet: its frames go out in later
                        # packets (up to 25 frames each), as a host catching up.
                        self.n_out -= n
                        self.acc += n << 16
                        pkt = bytearray()
                    elif len(pkt) + extra <= EP_SIZE:
                        pkt += bytes(extra)
            self.pkt_index += 1
            sizes.append(len(pkt))
            data += pkt
        self._new_urb(EP_OUT, sizes, bytes(data))

    def start(self, out=True, inp=True):
        if out:
            for _ in range(DEPTH):
                self._out_urb()
            for _ in range(2):
                self._new_urb(EP_FB, [4] * PKTS)
        if inp:
            for _ in range(DEPTH):
                self._new_urb(EP_IN, [EP_SIZE] * PKTS)

    def _check_in(self, data):
        for i in range(0, len(data) - FRAME + 1, FRAME):
            a, b = struct.unpack_from("<II", data, i)
            self.frames_in += 1
            if a == 0 and b == 0:
                if self.expect is not None:
                    self.zeros_mid += 1
                continue                          # silence (not primed yet / ended)
            if b != (~a & 0xFFFFFFFF):
                self.disc.append((time.monotonic() - self.t0, a, "corrupt"))
                self.expect = None
                continue
            if self.expect is not None and a != self.expect:
                self.disc.append((time.monotonic() - self.t0, a, self.expect))
            self.expect = (a + 1) & 0xFFFFFFFF

    def pump(self, until):
        """Reap and resubmit until monotonic time `until`."""
        while time.monotonic() < until and self.urbs:
            p = ctypes.c_void_p()
            ioctl(self.dev.fd, REAPURB, ctypes.byref(p))
            urb = Urb.from_address(p.value)
            key = urb.usercontext
            if key not in self.urbs:
                continue
            urb, buf = self.urbs.pop(key)
            ep = urb.endpoint
            off = 0
            for i in range(urb.number_of_packets):
                d = urb.iso[i]
                if d.status:
                    self.packet_errors += 1
                if ep == EP_IN:
                    self._check_in(buf.raw[off:off + d.actual_length])
                elif ep == EP_FB and d.actual_length >= 4:
                    v = struct.unpack_from("<I", buf.raw, off)[0]
                    self.fb_values.append(v)
                    self.fb = v
                off += d.length
            if not self.active[ep] or urb.status not in (0, -errno.EXDEV):
                continue
            if ep == EP_OUT:
                self._out_urb()
            elif ep == EP_FB:
                self._new_urb(EP_FB, [4] * PKTS)
            else:
                self._new_urb(EP_IN, [EP_SIZE] * PKTS)

    def stop(self):
        for ep in self.active:
            self.active[ep] = False
        for urb, _ in list(self.urbs.values()):
            try:
                ioctl(self.dev.fd, DISCARDURB, ctypes.byref(urb))
            except OSError:
                pass
        deadline = time.monotonic() + 1
        while self.urbs and time.monotonic() < deadline:
            p = ctypes.c_void_p()
            if libc.ioctl(self.dev.fd, ctypes.c_ulong(REAPURBNDELAY), ctypes.byref(p)) < 0:
                time.sleep(0.01)
                continue
            self.urbs.pop(Urb.from_address(p.value).usercontext, None)


def run_stream(dev, rate, secs, hooks=(), odd=None):
    dev.set_rate(rate)
    dev.vendor_out(0x04)                          # clear stats
    dev.set_interface(ITF_OUT, 1)
    dev.set_interface(ITF_IN, 1)
    s = Stream(dev, rate, odd)
    s.start()
    t_end = time.monotonic() + secs
    for at, fn in sorted(hooks, key=lambda h: h[0]):
        s.pump(s.t0 + at)
        fn(s)
    s.pump(t_end)
    st = dev.stats()
    s.stop()
    dev.set_interface(ITF_OUT, 0)
    dev.set_interface(ITF_IN, 0)
    return s, st


def report(name, ok, msg):
    print(f"{'ok  ' if ok else 'FAIL'} {name}: {msg}", flush=True)
    return ok


def clean(st):
    return st["underruns"] == 0 and st["rec_overruns"] == 0 and st["tx_err"] == 0 and st["rx_err"] == 0


def summary(s, st):
    if os.environ.get("DEBUG"):
        for d in s.disc[:20]:
            print(f"     disc t={d[0]:.4f} got {d[1]} want {d[2]}")
        print(f"     zero frames mid-stream: {s.zeros_mid}")
    hz = [v * 8000 / 65536 for v in s.fb_values[len(s.fb_values) // 2:]] or [0]
    return (f"{s.frames_in} frames in, {len(s.disc)} discontinuities, pkt errors {s.packet_errors}, "
            f"fb {sum(hz) / len(hz):.2f} Hz, underruns {st['underruns']} overruns {st['rec_overruns']} "
            f"fifo {st['tx_err']}/{st['rx_err']} lead {st['lead'][0]}..{st['lead'][1]}")


RATE = int(os.environ.get("RATE", "192000"))
SECS = float(os.environ.get("SECS", "10"))


def t_stream(dev):
    s, st = run_stream(dev, RATE, SECS)
    # The first frames after priming may start mid-counter: one discontinuity
    # at the start is the prime, not an error.
    return report("stream", len(s.disc) == 0 and clean(st) and s.frames_in > RATE * (SECS - 1), summary(s, st))


def t_ratechange(dev):
    ok = True
    for a, b in ((192000, 48000), (48000, 192000), (192000, 96000)):
        def change(s, b=b):
            dev.set_rate(b)
            s.rate_changed = dev.get_rate()
        s, st = run_stream(dev, a, 4, hooks=[(2, change)])
        good = s.rate_changed == b and dev.alive()
        ok &= report(f"ratechange {a}->{b}", good,
                     f"GET_CUR {s.rate_changed}, {len(s.disc)} discontinuities (expected: rate switch "
                     f"restarts the rings), pkt errors {s.packet_errors}, submit errors {s.submit_errors}")
    s, st = run_stream(dev, RATE, 3)
    return report("ratechange: clean stream after", ok and len(s.disc) == 0 and clean(st), summary(s, st)) and ok


def t_halt(dev):
    def clear_halts(s):
        for ep in (EP_OUT, EP_FB, EP_IN):
            ioctl(dev.fd, CLEAR_HALT, ctypes.byref(ctypes.c_uint(ep)))

    def set_halts(s):
        for ep in (EP_OUT, EP_FB, EP_IN):
            try:
                dev.control(0x02, 0x03, 0, ep)       # SET_FEATURE(ENDPOINT_HALT)
            except OSError as e:
                s.set_halt_err = e.strerror
    s, st = run_stream(dev, RATE, 6, hooks=[(2, clear_halts), (3, set_halts), (4, clear_halts)])
    return report("halt", len(s.disc) == 0 and clean(st) and dev.alive(),
                  f"SET/CLEAR_FEATURE(ENDPOINT_HALT) on 0x01/0x81/0x82 mid-stream; " + summary(s, st))


def t_oddsize(dev):
    # Every 7th packet: +3 junk bytes (not a whole frame); every 101st: zero
    # length (its frames follow in later packets); every 13th: +7 junk bytes.
    def odd(i):
        if i % 101 == 5:
            return -1
        if i % 7 == 3:
            return 3
        if i % 13 == 1:
            return 7
        return None
    s, st = run_stream(dev, RATE, 6, odd=odd)
    # Junk bytes must not shift the frame alignment: no corrupt frames. Zero
    # length packets drop the lead a little; no discontinuities, because the
    # counter skips nothing.
    corrupt = sum(1 for d in s.disc if d[2] == "corrupt")
    return report("oddsize", corrupt == 0 and len(s.disc) == 0 and st["rec_overruns"] == 0 and dev.alive(),
                  f"{corrupt} corrupt; " + summary(s, st))


def t_altstorm(dev):
    dev.set_rate(RATE)
    t = time.monotonic()
    for i in range(500):
        dev.set_interface(ITF_OUT, i & 1)
        dev.set_interface(ITF_IN, (i >> 1) & 1)
    dev.set_interface(ITF_OUT, 0)
    dev.set_interface(ITF_IN, 0)
    dt = time.monotonic() - t
    # Storm with streams: alt 1, a few URBs, alt 0, repeat.
    for _ in range(50):
        dev.set_interface(ITF_OUT, 1)
        dev.set_interface(ITF_IN, 1)
        s = Stream(dev, RATE)
        s.start()
        s.pump(time.monotonic() + 0.02)
        s.stop()
        dev.set_interface(ITF_OUT, 0)
        dev.set_interface(ITF_IN, 0)
    s, st = run_stream(dev, RATE, 3)
    return report("altstorm", len(s.disc) == 0 and clean(st),
                  f"1000 SET_INTERFACE in {dt:.2f} s + 50 short streams; then " + summary(s, st))


def t_control(dev):
    ok = True

    def expect(name, fn, want):
        nonlocal ok
        try:
            r = fn()
            got = r if isinstance(r, int) else len(r)
            res = got
        except OSError as e:
            res = "stall" if e.errno == errno.EPIPE else e.strerror
        good = res == want and dev.alive()
        ok &= good
        print(f"     {'ok' if good else 'BAD'} {name}: {res} (want {want})")

    expect("GET_CUR freq, wLength 2", lambda: dev.control(0xA1, 1, 0x0100, 0x0400, length=2), 2)
    expect("GET_CUR freq, wLength 64", lambda: dev.control(0xA1, 1, 0x0100, 0x0400, length=64), 4)
    expect("GET_RANGE freq, wLength 2", lambda: dev.control(0xA1, 2, 0x0100, 0x0400, length=2), 2)
    expect("GET_RANGE freq, full", lambda: dev.control(0xA1, 2, 0x0100, 0x0400, length=255), 38)
    expect("SET_CUR freq, 3 bytes", lambda: dev.control(0x21, 1, 0x0100, 0x0400, b"\x00\xee\x02"), "stall")
    expect("SET_CUR freq, 8 bytes", lambda: dev.control(0x21, 1, 0x0100, 0x0400, bytes(8)), "stall")
    expect("SET_CUR freq 44100", lambda: dev.set_rate(44100), "stall")
    expect("SET_CUR freq 0", lambda: dev.set_rate(0), "stall")
    expect("GET_CUR unknown selector 0x05", lambda: dev.control(0xA1, 1, 0x0500, 0x0400, length=4), "stall")
    expect("GET_CUR unknown entity 0x09", lambda: dev.control(0xA1, 1, 0x0100, 0x0900, length=4), "stall")
    expect("class request to itf 1", lambda: dev.control(0xA1, 1, 0x0100, 0x0001, length=4), "stall")
    # usbfs refuses wLength > PAGE_SIZE (EINVAL), so 4096 is the largest testable.
    expect("GET_DESCRIPTOR config, wLength 4096", lambda: dev.control(0x80, 6, 0x0200, 0, length=4096), 227)
    expect("GET_DESCRIPTOR config, wLength 9", lambda: dev.control(0x80, 6, 0x0200, 0, length=9), 9)
    expect("GET_DESCRIPTOR string 200", lambda: dev.control(0x80, 6, 0x03C8, 0x0409, length=255), "stall")
    expect("GET_DESCRIPTOR type 0x0F (BOS)", lambda: dev.control(0x80, 6, 0x0F00, 0, length=255), "stall")
    expect("GET_STATUS device", lambda: dev.control(0x80, 0, 0, 0, length=2), 2)
    expect("vendor GET_STATS, wLength 2", lambda: dev.vendor_in(0x03, 2), 2)
    expect("vendor SET_SCRATCH, 64 bytes", lambda: dev.control(0x41, 0x02, 0, ITF_VENDOR, bytes(64)), "stall")
    expect("vendor unknown 0x7F", lambda: dev.vendor_in(0x7F, 8), "stall")
    expect("vendor DBG_LOOPBACK wValue 2", lambda: dev.vendor_out(0x12, 2), "stall")
    expect("vendor request, wrong itf 0", lambda: dev.control(0xC1, 0x00, 0, 0, length=8), "stall")
    dev.vendor_out(0x12, 1)                       # loopback back on
    s, st = run_stream(dev, RATE, 3)
    return report("control", ok and len(s.disc) == 0 and clean(st), summary(s, st))


TESTS = dict(stream=t_stream, ratechange=t_ratechange, halt=t_halt, oddsize=t_oddsize,
             altstorm=t_altstorm, control=t_control)


def main():
    names = sys.argv[1:] or ["all"]
    if names == ["all"]:
        names = list(TESTS)
    dev = Dev()
    fails = 0
    try:
        dev.vendor_out(0x12, 1)                   # debug loopback on
        for n in names:
            fails += not TESTS[n](dev)
    finally:
        dev.close()
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
