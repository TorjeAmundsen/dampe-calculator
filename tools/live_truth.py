"""RNG watch truth for a 1920x1080 gray Virtual Camera recording."""
import subprocess, sys, pickle
import numpy as np
sys.path.insert(0, '.')
from ocr1 import cells, norm

path, out = sys.argv[1], sys.argv[2]
L = "5c39ab62740" + "8fe1d"
cl = np.array(pickle.load(open('cl.pkl', 'rb')))
assert len(L) == 16
W, H = 1920, 1080
WX, WY, WW, WH = 1040 + 240, 1012, 390, 46
p = subprocess.Popen(['ffmpeg', '-v', 'error', '-i', path, '-f', 'rawvideo', '-pix_fmt', 'gray', '-'],
                     stdout=subprocess.PIPE, bufsize=W * H * 4)


def read(a):
    xs = cells(a)
    if len(xs) < 3 or len(xs) > 10:
        return None
    s = ''
    for x0, x1 in xs[2:]:
        s += L[int(np.abs(cl - norm(a, x0, x1)).mean(axis=(1, 2)).argmin())]
    return int(s, 16)


prev = None
cache = {}
lines = []
n = 0
while True:
    b = p.stdout.read(W * H)
    if len(b) < W * H:
        break
    a = np.frombuffer(b, np.uint8).reshape(H, W)
    s = a[125:645:3, 240:1680:3].astype(np.int16)
    if prev is None or (np.abs(s - prev) > 2).sum() >= 8:
        w = a[WY:WY + WH, WX:WX + WW]
        k = w.tobytes()
        if k not in cache:
            cache[k] = read(w)
        lines.append((n, cache[k]))
    prev = s
    n += 1
print('video frames', n, 'game frames', len(lines), 'unread', sum(v is None for _, v in lines))
A, I, M = 1664525, 1013904223, 0xffffffff
bad = big = 0
last = None
for _, v in lines:
    if v is None:
        continue
    if last is not None:
        x = last
        for k in range(3000):
            if x == v:
                break
            x = (x * A + I) & M
        else:
            bad += 1
        if k > 3:
            big += 1
    last = v
print('unreachable steps', bad, 'frames with >3 draws', big)
with open(out, 'w', newline='\n') as f:
    f.write('# video_frame rng_state, one line per game frame, read from the gz RNG watch\n')
    for i, v in lines:
        if v is not None:
            f.write(f'{i} {v:08x}\n')
