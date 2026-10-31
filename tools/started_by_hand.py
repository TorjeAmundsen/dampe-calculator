"""Starts reading by hand before the Yes/No box on first talks, and checks the cues against the RNG watch.

The start comes this many game frames after Navi's last big burst of draws. Run from the project
folder, with the binary as an argument.
"""
import os
import re
import subprocess
import sys

A, I = 1664525, 1013904223
MASK = 0x7fffff
AFTER_BURST = 25
RECORDINGS = [
    ("G:/Replay Buffer/2026-10-31 13-16-35.mkv", "c1"),
    ("G:/Replay Buffer/2026-10-31 13-20-58.mkv", "c2"),
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


def session(video, *more):
    run = subprocess.run([binary, "replay", video, "--session", *more], capture_output=True, text=True)
    return run.stdout.splitlines()


for video, name in RECORDINGS:
    rows = [l.split() for l in open(f"tests/truth/{name}.txt") if not l.startswith("#")]
    frames, vals = [int(r[0]) for r in rows], [int(r[1], 16) & MASK for r in rows]
    box = next(int(m[1]) for m in (re.match(r"video frame (\d+): BoxOpened", l) for l in session(video)) if m)
    box_index = next(i for i, f in enumerate(frames) if f >= box)
    burst_end = max(i for i in range(1, box_index) if draws_between(vals[i - 1], vals[i]) > 3)
    start_index = burst_end + AFTER_BURST
    mark = frames[start_index]
    print(f"{name}: box at video frame {box}, last burst ends at {frames[burst_end]}, starting by hand at {mark}")
    print(f"  that's {(box - mark) / 59.94:.1f} s before the box")
    # The first new game frame after the mark is game frame 0
    base = start_index + 1
    right = wrong = 0
    first_lock = None
    first_cue = None
    for line in session(video, "--navi-gone-at", str(mark)):
        m = re.match(r"video frame (\d+): (.*)", line)
        if not m:
            continue
        at, event = int(m[1]) - 1, m[2]
        if event.startswith("Status(Locked)") and first_lock is None:
            first_lock = at
        if event.startswith("BoxGone"):
            print("  ", event[:90])
        cue = re.search(r"Cue \{ box_frame: \d+.*first_roll: (\d+), last_roll: (\d+)", event)
        if cue:
            index = base + (int(cue[1]) + int(cue[2])) // 2
            if first_cue is None:
                first_cue = (event.split(" ")[0], at)
            if index < len(vals) and draws_between(vals[index - 1], vals[index]) <= 3:
                if rolls(vals, index):
                    right += 1
                else:
                    wrong += 1
    print(f"  first lock at video frame {first_lock}, {(first_lock - mark) / 59.94:.1f} s after starting" if first_lock else "  no lock")
    print(f"  {(first_lock - box) / 59.94:+.1f} s relative to the box opening" if first_lock else "")
    print(f"  first cue {first_cue}; cues checked: {right} right, {wrong} wrong")
