"""On a 1920x1080 color recording: box open/close, RNG bursts, and when the picture starts repeating on the idle loop."""
import subprocess, sys
import numpy as np
video, truth = sys.argv[1:3]
A, I, M = 1664525, 1013904223, 0xffffffff
lines = [l.split() for l in open(truth) if not l.startswith('#')]
frames = np.array([int(l[0]) for l in lines])
vals = [int(l[1], 16) for l in lines]
index = {int(f): i for i, f in enumerate(frames)}
n = len(frames)
draws = [0] * n
for i in range(1, n):
    x = vals[i - 1]
    for k in range(3000):
        if x == vals[i]:
            draws[i] = k
            break
        x = (x * A + I) & M
    else:
        draws[i] = -1
SW, SH = 240, 86
TW, TH = 300, 75
fw, fh = 1920, 1080
vf = f"split=2[a][b];[a]crop=1440:516:240:124,scale={SW}:{SH}:flags=area,format=gray[s];[b]crop=1200:300:360:650,scale={TW}:{TH}:flags=area,format=rgb24,format=rgb24[t]"
# two pipes are awkward: read the frame in full and cut it here
p = subprocess.Popen(['ffmpeg', '-v', 'error', '-i', video, '-vf', 'scale=480:270:flags=area', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], stdout=subprocess.PIPE)
W, H = 480, 270
scene = np.zeros((n, 129, 360), np.int16)
box = np.zeros(n, bool)
counts = np.zeros((n, 3), int)
v = 0
while True:
    b = p.stdout.read(W * H * 3)
    if len(b) < W * H * 3:
        break
    if v in index:
        a = np.frombuffer(b, np.uint8).reshape(H, W, 3).astype(np.int16)
        i = index[v]
        scene[i] = a[31:160, 60:420].mean(axis=2)
        t = a[162:238, 90:390]
        r, g, bl = t[..., 0], t[..., 1], t[..., 2]
        counts[i] = (((bl > 150) & (r < 110) & (bl - g > 60)).sum(), ((g > 150) & (r < 130) & (bl < 130)).sum(), ((r > 150) & (g < 90) & (bl < 90)).sum())
        box[i] = (counts[i] >= (5, 8, 5)).all()
    v += 1
print('video frames', v, 'game frames', n, 'unreachable', sum(d == -1 for d in draws))
runs = []
i = 0
while i < n:
    if box[i]:
        j = i
        while j + 1 < n and (box[j + 1] or (j + 2 < n and box[j + 2])):
            j += 1
        if j - i > 3:
            runs.append((i, j))
        i = j + 1
    else:
        i += 1
print('box runs, game frames:', runs)
bursts = []
i = 1
while i < n:
    if draws[i] > 3 or draws[i] == -1:
        j = i
        while j + 1 < n and any(d > 3 or d == -1 for d in draws[j + 1:j + 6]):
            j += 1
        bursts.append((i, j, max(draws[i:j + 1])))
        i = j + 1
    else:
        i += 1
print('bursts of more than 3 draws (first, last, biggest):', bursts)
L = 49
diff = np.full(n, np.nan)
for i in range(n - L):
    diff[i] = np.abs(scene[i] - scene[i + L]).mean()
print('difference to the same point 49 frames later, every 10 game frames:')
print([(i, round(float(diff[i]), 2)) for i in range(0, n - L, 10)])
for a, b in runs:
    print(f'box {a}..{b} ({(b - a) / 19.98:.1f} s):')
    ins = [x for x in bursts if a - 150 <= x[0] <= b]
    print('  bursts near or in it, offset from open:', [(x[0] - a, x[1] - a, x[2]) for x in ins])
    calm = None
    for i in range(a - 100 if a > 100 else 0, b - L):
        if np.nan_to_num(diff[i:i + 15], nan=99).max() < 1.0:
            calm = i
            break
    print('  first of 15 frames repeating under 1.0, offset from open:', None if calm is None else calm - a)
