#!/usr/bin/env python3
"""Bit-exact loopback test for diyuac2 (playback looped into capture: R2
firmware, or the Rust firmware with `vendorctl.py loopback on`).

  loopback.py [card] [secs] [rate]

Plays N seconds of random 32-bit stereo frames on hw:CARD and records at the
same time, then searches the recording for the played block and compares it
byte for byte.
"""
import os, random, subprocess, sys, time

card = sys.argv[1] if len(sys.argv) > 1 else "3"
secs = float(sys.argv[2]) if len(sys.argv) > 2 else 5.0
rate = int(sys.argv[3]) if len(sys.argv) > 3 else 48000
ch, bps = 2, 4
frame = ch * bps
n = int(secs * rate)
rnd = random.Random(1234)
data = rnd.randbytes(n * frame)
play, rec = "/tmp/lb_play.raw", "/tmp/lb_rec.raw"
open(play, "wb").write(data)
fmt = ["-t", "raw", "-f", "S32_LE", "-c", str(ch), "-r", str(rate)]
r = subprocess.Popen(["arecord", "-q", "-D", f"hw:{card},0", *fmt, "-d", str(int(secs + 3)), rec])
time.sleep(1.0)
p = subprocess.run(["aplay", "-q", "-D", f"hw:{card},0", *fmt, play])
r.wait()
got = open(rec, "rb").read()
print(f"aplay rc={p.returncode} arecord rc={r.returncode} recorded {len(got)//frame} frames")
off = got.find(data[:64])
if off < 0:
    print("FAIL: start of played block not found in recording")
    sys.exit(1)
seg = got[off:off + len(data)]
if len(seg) < len(data):
    print(f"FAIL: recording truncated, {len(seg)//frame} of {n} frames after offset")
    sys.exit(1)
if seg == data:
    print(f"PASS: {n} frames bit-exact, offset {off//frame} frames (off % frame = {off % frame})")
    sys.exit(0)
bad = next(i for i in range(len(data)) if seg[i] != data[i])
nbad = sum(1 for i in range(0, len(data), frame) if seg[i:i+frame] != data[i:i+frame])
print(f"FAIL: first mismatch at frame {bad//frame}, {nbad} frames differ")
sys.exit(1)
