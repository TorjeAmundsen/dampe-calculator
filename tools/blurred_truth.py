import subprocess, sys, pickle
import numpy as np, cv2
sys.path.insert(0, '.')
from ocr1 import cells, norm
path, out = sys.argv[1], sys.argv[2]
L = "5c39ab62740" + "8fe1d"
cl = np.array(pickle.load(open('cl.pkl', 'rb')))
W, H = 1920, 1080
A, I, M = 1664525, 1013904223, 0xffffffff
p = subprocess.Popen(['ffmpeg', '-v', 'error', '-i', path, '-f', 'rawvideo', '-pix_fmt', 'gray', '-'], stdout=subprocess.PIPE, bufsize=W*H*4)
GOOD = []
def read(a):
    w = cv2.resize(a, (390, 46), interpolation=cv2.INTER_CUBIC).astype(np.float32)
    w = np.clip((w - 16) * 255 / 150, 0, 255).astype(np.uint8)
    xs = cells(w)
    if not (3 <= len(xs) <= 10):
        return None
    if len(xs) != 10:
        return ('slice', w)
    GOOD.append(xs)
    s = ''
    for x0, x1 in xs[2:]:
        s += L[int(np.abs(cl - norm(w, x0, x1)).mean(axis=(1, 2)).argmin())]
    return int(s, 16)
def sliced(w, wider=0):
    xs = np.median(np.array(GOOD[:400]), axis=0).astype(int)
    s = ''
    for x0, x1 in xs[2:]:
        s += L[int(np.abs(cl - norm(w, max(x0 - wider, 0), x1 + wider)).mean(axis=(1, 2)).argmin())]
    return int(s, 16)
prev = None; cache = {}; lines = []; n = 0
while True:
    b = p.stdout.read(W*H)
    if len(b) < W*H: break
    a = np.frombuffer(b, np.uint8).reshape(H, W)
    s = a[160:640:3, 300:1620:3].astype(np.int16)
    if prev is None or (np.abs(s - prev) > 2).sum() >= 8:
        w = a[994:1036, 1262:1622]
        k = w.tobytes()
        if k not in cache: cache[k] = read(w)
        lines.append((n, cache[k]))
    prev = s; n += 1
sl = sum(isinstance(v, tuple) for _, v in lines)
print('needing fixed slices', sl)
fixed = []
last = None
for i, v in lines:
    if isinstance(v, tuple):
        v = sliced(v[1])
    fixed.append((i, v))
lines = fixed
got = sum(v is not None for _, v in lines)
print('video', n, 'game', len(lines), 'read', got)
# keep what the RNG can reach from an agreed value within 300 draws; start from the longest agreeing run
vals = [(i, v) for i, v in lines if v is not None]
def reach(a, b, lim=300):
    x = a
    for k in range(lim + 1):
        if x == b: return k
        x = (x*A + I) & M
    return None
best = []
for st in range(0, min(len(vals), 400)):
    run = [vals[st]]
    for i, v in vals[st+1:st+60]:
        if reach(run[-1][1], v) is not None: run.append((i, v))
    if len(run) > len(best): best = run; bs = st
print('start index', bs, 'agreeing', len(best), 'of 60')
keep = [vals[bs]]; dropped = 0
for i, v in vals[bs+1:]:
    if reach(keep[-1][1], v) is not None: keep.append((i, v))
    else: dropped += 1
print('kept', len(keep), 'dropped misreads', dropped, 'first frame', keep[0][0], 'last', keep[-1][0])
with open(out, 'w', newline='\n') as f:
    f.write('# video_frame rng_state, one line per game frame, read from the gz RNG watch\n')
    known = dict(keep); last = keep[0][1]
    for i, _ in lines:
        last = known.get(i, last)
        if i >= keep[0][0]: f.write(f'{i} {last:08x}'+chr(10))
