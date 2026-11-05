#!/usr/bin/env python3
"""Long bit-exact loopback soak for diyuac2 (Rust firmware, `vendorctl.py
loopback on`).

Plays a 1 s random block over and over, records at the same time, and checks
every recorded frame against the block, in a streaming fashion (memory use
does not grow with the run length). Once the block is found in the
recording, every following frame must continue it exactly; a slip, drop or
corrupted frame is reported with its time, and the checker resynchronises.

  lbsoak.py [card] [secs] [rate] [channels]

channels: 2 (default), or 16 for the Rust firmware's 16-channel TDM alt
setting (12.3 MB/s each way at 192 kHz).
"""
import random, subprocess, sys, threading, time


def unity_mixer(card):
    """Feature Units to 0 dB, unmuted: the host mixer (WirePlumber, alsactl)
    may have set another level, and only 0 dB is bit-exact. Cards without
    the controls (C firmware) are left alone."""
    for name, val in (("PCM Playback Volume", "100%"), ("PCM Playback Switch", "on"),
                      ("Line Capture Volume", "100%"), ("Line Capture Switch", "on")):
        subprocess.run(["amixer", "-q", "-c", card, "cset", f"name={name}", val],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


card = sys.argv[1] if len(sys.argv) > 1 else "3"
secs = int(sys.argv[2]) if len(sys.argv) > 2 else 60
rate = int(sys.argv[3]) if len(sys.argv) > 3 else 192000
channels = int(sys.argv[4]) if len(sys.argv) > 4 else 2
unity_mixer(card)
FRAME = 4 * channels
SYNC = 256       # bytes of the block start searched for in the recording
block = random.Random(1234).randbytes(rate * FRAME)    # 1 s
fmt = ["-t", "raw", "-f", "S32_LE", "-c", str(channels), "-r", str(rate)]

rec = subprocess.Popen(["arecord", "-q", "-D", f"hw:{card},0", *fmt, "-d", str(secs + 3)],
                       stdout=subprocess.PIPE)
play = None


def feed():
    # Start playback 1 s into the recording; the main thread reads the
    # recording all along, so arecord never overruns.
    global play
    time.sleep(1.0)
    play = subprocess.Popen(["aplay", "-q", "-D", f"hw:{card},0", *fmt], stdin=subprocess.PIPE)
    try:
        for _ in range(secs):
            play.stdin.write(block)
        play.stdin.close()
    except BrokenPipeError:
        pass


threading.Thread(target=feed, daemon=True).start()

CHUNK = rate // 10 * FRAME    # 100 ms
synced = False
pos = 0          # expected offset in block of the next recorded byte
checked = 0      # frames checked while synced
errors = 0
t_frames = 0     # recorded frames, for timestamps
buf = b""
expect_sync = None   # recording frame where the next block start is due, after a mismatch
last_report = time.time()
while True:
    data = rec.stdout.read(CHUNK)
    if not data:
        break
    buf += data
    if not synced:
        off = buf.find(block[:SYNC])
        if off < 0:
            # Keep a frame-aligned tail that may hold the start of a match.
            keep = (SYNC + FRAME - 1) // FRAME * FRAME
            if len(buf) > keep:
                t_frames += (len(buf) - keep) // FRAME
                buf = buf[-keep:]
            continue
        if off % FRAME:
            print(f"MISALIGNED match at byte {off}: a sample was split")
        t_frames += off // FRAME
        buf = buf[off:]
        synced, pos = True, 0
        msg = f"synced at {t_frames / rate:.3f} s"
        if expect_sync is not None:
            # Frames lost (<0) or added (>0) between the mismatch and here,
            # modulo the 1 s block.
            slip = (t_frames - expect_sync + rate // 2) % rate - rate // 2
            msg += f", slip {slip:+d} frames"
            expect_sync = None
        print(msg, flush=True)
    # Compare as much of buf as possible, wrapping around the block.
    i = 0
    while len(buf) - i >= FRAME:
        n = min(len(buf) - i, len(block) - pos)
        n -= n % FRAME
        if buf[i:i + n] != block[pos:pos + n]:
            bad = next(k for k in range(0, n, FRAME) if buf[i + k:i + k + FRAME] != block[pos + k:pos + k + FRAME])
            at = (t_frames + (i + bad) // FRAME) / rate
            # Played data has ended: the rest is silence.
            if buf[i + bad:i + bad + 64] == bytes(64) and checked >= (secs - 1) * rate:
                print(f"stream end at {at:.3f} s")
                i = len(buf)
                synced = None
                break
            errors += 1
            got = buf[i + bad:i + bad + FRAME]
            zero = got == bytes(FRAME)
            print(f"MISMATCH at {at:.3f} s (frame {checked + bad // FRAME} of the stream), "
                  f"got {'zeros' if zero else got[:8].hex()}, resync", flush=True)
            expect_sync = t_frames + (i + bad) // FRAME + (len(block) - (pos + bad)) // FRAME
            checked += bad // FRAME
            t_frames += (i + bad) // FRAME
            buf = buf[i + bad + FRAME:]
            t_frames += 1
            i = 0
            synced = False
            break
        i += n
        checked += n // FRAME
        pos = (pos + n) % len(block)
    if synced is None:
        break
    if synced:
        t_frames += i // FRAME
        buf = buf[i:]
    if time.time() - last_report >= 60:
        last_report = time.time()
        print(f"{time.strftime('%H:%M:%S')} checked {checked} frames ({checked / rate:.0f} s), errors {errors}",
              flush=True)
    if synced is False and errors > 100:
        break

while play is None:
    time.sleep(0.1)
play.wait()
rec.kill()
rec.wait()
ok = errors == 0 and checked >= (secs - 1) * rate
print(f"{'PASS' if ok else 'FAIL'}: {checked} frames ({checked / rate:.1f} s) checked, {errors} mismatches")
sys.exit(0 if ok else 1)
