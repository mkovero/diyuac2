#!/usr/bin/env python3
"""Bit-exact playback -> capture loopback test (M2 firmware loops OUT to IN).

Plays pseudo-random 24-bit-in-32 stereo data and records simultaneously
with ALSA hw: devices, then locates the played data in the recording and
compares the overlap bit for bit.

  loopback_test.py <alsa card> <rate> [seconds]
"""

import os
import random
import subprocess
import sys
import tempfile
import time

FRAME = 8  # 2 ch * 4 bytes


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    card, rate = sys.argv[1], int(sys.argv[2])
    secs = int(sys.argv[3]) if len(sys.argv) > 3 else 10
    dev = f"hw:{card},0"

    rnd = random.Random(rate)
    nframes = rate * secs
    # 24 valid bits, left-justified in 32-bit slot: low byte zero.
    play = b"".join(
        (rnd.getrandbits(24) << 8).to_bytes(4, "little") for _ in range(nframes * 2)
    )

    with tempfile.TemporaryDirectory() as d:
        pf, rf = os.path.join(d, "play.raw"), os.path.join(d, "rec.raw")
        open(pf, "wb").write(play)
        fmt = ["-f", "S32_LE", "-c", "2", "-r", str(rate), "-t", "raw"]
        rec = subprocess.Popen(["arecord", "-D", dev, *fmt, "-d", str(secs + 3), rf],
                               stderr=subprocess.PIPE, text=True)
        time.sleep(1.0)
        p = subprocess.run(["aplay", "-D", dev, *fmt, pf], stderr=subprocess.PIPE, text=True)
        _, rec_err = rec.communicate()
        recd = open(rf, "rb").read()

    problems = [l for l in (p.stderr + rec_err).splitlines() if "run" in l.lower()]

    # Locate a probe from 1 s into the played data.
    probe_at = rate * FRAME
    probe = play[probe_at:probe_at + 64 * FRAME]
    idx = recd.find(probe)
    if idx < 0:
        print(f"{rate} Hz: FAIL, played data not found in recording ({len(recd)} bytes)")
        sys.exit(1)
    offset = idx - probe_at  # recording byte position of played byte 0
    start = max(0, -offset)
    end = min(len(play), len(recd) - offset)
    a = play[start:end]
    b = recd[start + offset:end + offset]
    mism = sum(1 for i in range(0, len(a), FRAME) if a[i:i + FRAME] != b[i:i + FRAME])
    covered = (end - start) / len(play) * 100
    print(f"{rate} Hz: offset {offset // FRAME} frames, compared {(end - start) // FRAME} frames "
          f"({covered:.1f}% of played), mismatched frames: {mism}, xrun messages: {len(problems)}")
    for l in problems:
        print("   ", l)
    sys.exit(0 if mism == 0 and not problems and covered > 99.9 else 1)


if __name__ == "__main__":
    main()
