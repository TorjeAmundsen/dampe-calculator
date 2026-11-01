"""Checks reading from the first text box against the RNG watch: when it starts, when it locks, and whether cues roll.

Run from the project folder, with the binary as an argument.
"""
import os
import re
import subprocess
import sys

A, I = 1664525, 1013904223
MASK = 0x7fffff
RECORDINGS = [
    ("G:/Replay Buffer/2026-10-31 13-16-35.mkv", "c1"),
    ("G:/Replay Buffer/2026-10-31 13-20-58.mkv", "c2"),
    ("recordings/repeat1.mkv", "repeat1"),
    ("recordings/repeat2.mkv", "repeat2"),
    ("recordings/clean.mkv", "clean"),
    ("recordings/headon_jaw.mkv", "headon_jaw"),
    ("recordings/noangle2.mkv", "noangle2"),
    ("recordings/skybusy.mkv", "skybusy"),
    ("recordings/english1.mkv", "english1"),
    ("recordings/slow_eye.mkv", "slow_eye"),
]
binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/diag/release/dampe.exe")


def step(x):
    return (x * A + I) & MASK


def draws_between(a, b):
    n = 0
    while a != b and n < 300:
        a = step(a)
        n += 1
    return n


def rolls(vals, index):
    x = vals[index - 1]
    if draws_between(x, vals[index]) in (1, 3):
        x = step(x)
    for _ in range(5):
        x = step(x)
    return x / (MASK + 1) >= 0.9


for video, name in RECORDINGS:
    rows = [l.split() for l in open(f"tests/truth/{name}.txt") if not l.startswith("#")]
    frames, vals = [int(r[0]) for r in rows], [int(r[1], 16) & MASK for r in rows]
    lines = subprocess.run([binary, "replay", video, "--session"], capture_output=True, text=True).stdout.splitlines()
    base = None
    started = lock = None
    right = wrong = 0
    print(name)
    for line in lines:
        m = re.match(r"video frame (\d+): (.*)", line)
        if not m:
            continue
        at, event = int(m[1]), m[2]
        if event.startswith(("TextGone", "BoxOpened", "NaviDone", "BoxGone")):
            print(f"  {at / 59.94:7.1f} s  {event[:70]}")
        if event.startswith("TextOpened") or (event.startswith("BoxOpened") and base is None):
            started = at
            base = next(i for i, f in enumerate(frames) if f >= at)
            lock = None
        # She's here, so the read's first game frame is the next one, and everything so far is gone
        if event.startswith("NaviSeen"):
            base = next(i for i, f in enumerate(frames) if f > at)
            lock = None
        if event.startswith("Status(Locked)") and lock is None:
            lock = at
            print(f"  {at / 59.94:7.1f} s  first lock, {(at - started) / 59.94:.1f} s after starting")
        cue = re.search(r"Cue \{ box_frame: \d+, seconds: ([\d.]+).*first_roll: (\d+), last_roll: (\d+)", event)
        # A cue for after the box left can't be pressed, and the RNG goes its own way by then
        gone = next((int(l.split()[2][:-1]) for l in lines if ": BoxGone" in l), None)
        if cue and base is not None and (gone is None or frames[min(base + int(cue[2]), len(frames) - 1)] < gone):
            index = base + (int(cue[2]) + int(cue[3])) // 2
            if index < len(vals) and draws_between(vals[index - 1], vals[index]) <= 3:
                if rolls(vals, index):
                    right += 1
                else:
                    wrong += 1
    print(f"  cues checked: {right} right, {wrong} wrong")
