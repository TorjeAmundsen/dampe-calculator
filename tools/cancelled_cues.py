"""Would likely cues that got cancelled have worked anyway? Checks each against the RNG watch.

Run from the project folder, with the binary as an argument. Color recordings only, since the game
frames are counted from the box opening.
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
    ("recordings/v6.mkv", "v6"),
    ("recordings/headon_jaw.mkv", "headon_jaw"),
    ("recordings/partial_jaw.mkv", "partial_jaw"),
    ("recordings/clean.mkv", "clean"),
    ("recordings/repeat1.mkv", "repeat1"),
    ("recordings/repeat2.mkv", "repeat2"),
]
binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/diag/release/dampe.exe")


def step(x):
    return (x * A + I) & MASK


def load(name):
    rows = [l.split() for l in open(f"tests/truth/{name}.txt") if not l.startswith("#")]
    return [int(r[0]) for r in rows], [int(r[1], 16) & MASK for r in rows]


def rolls(vals, index):
    """Whether a dig whose first dust is on this game frame gives the Heart Piece."""
    x, target = vals[index - 1], vals[index]
    draws = 0
    y = x
    while y != target and draws < 200:
        y = step(y)
        draws += 1
    if draws in (1, 3):
        x = step(x)
    for _ in range(5):
        x = step(x)
    return x / (MASK + 1) >= 0.9


totals = {"kept": [0, 0], "cancelled": [0, 0]}
for video, name in RECORDINGS:
    frames, vals = load(name)
    out = subprocess.run([binary, "replay", video, "--session"], capture_output=True, text=True).stdout
    base = None
    current = None
    chance = None
    results = []

    def finish(cancel_video):
        target = (current[1] + current[2]) // 2
        index = base + target
        if index >= len(vals):
            return
        target_video = frames[index]
        # A cue "cancelled" after its moment went by wasn't really cancelled
        kind = "cancelled" if cancel_video is not None and cancel_video < target_video else "kept"
        right = rolls(vals, index)
        totals[kind][0] += 1
        totals[kind][1] += right
        results.append((kind, round(chance * 100), right))

    for line in out.splitlines():
        m = re.match(r"video frame (\d+): (.*)", line)
        if not m:
            continue
        at, event = int(m[1]) - 1, m[2]
        if event.startswith("BoxOpened"):
            base = next((i for i, f in enumerate(frames) if f >= at), None)
        elif event.startswith("LikelyCue"):
            e = re.search(r"first_roll: (\d+), last_roll: (\d+) \}, chance: ([\d.]+)", event)
            current = (at, int(e[1]), int(e[2]))
            chance = float(e[3])
        elif event.startswith("LikelyChance") and current:
            chance = float(re.search(r"chance: ([\d.]+)", event)[1])
        elif event.startswith("LikelyCancelled") and current:
            finish(at)
            current = None
        elif event.startswith("BoxGone"):
            current = None
    if current:
        finish(None)
    print(name, results)
for kind, (n, right) in totals.items():
    print(f"{kind}: {right} of {n} would have rolled the Heart Piece at the target frame")
