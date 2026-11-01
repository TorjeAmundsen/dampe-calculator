#!/bin/bash
# Scores a build against RNG truth on every recording: locks, Heart Piece windows, time to first lock
# and how likely-cue chances compare with what really happened. Run from anywhere.
# Takes the binary as an argument, target/diag/release/dampe.exe by default.
cd "$(dirname "$0")/.."
B="${1:-target/diag/release/dampe.exe}"
T="${TMPDIR:-/tmp}/dampe_calibrate_$$"
mkdir -p "$T"
i=0
for a in "G:/Replay Buffer/2026-10-31 13-16-35.mkv|c1|" "G:/Replay Buffer/2026-10-31 13-20-58.mkv|c2|" \
    "recordings/v2.mkv|v2|--no-box" "recordings/v3.mkv|v3|--no-box" "recordings/v6.mkv|v6|" \
    "recordings/headon_jaw.mkv|headon_jaw|" "recordings/partial_jaw.mkv|partial_jaw|" "recordings/clean.mkv|clean|" \
    "recordings/repeat1.mkv|repeat1|" "recordings/repeat2.mkv|repeat2|" "recordings/noangle2.mkv|noangle2|" "recordings/skybusy.mkv|skybusy|" "recordings/english1.mkv|english1|" "recordings/slow_eye.mkv|slow_eye|"; do
  IFS='|' read -r v t f <<< "$a"
  ( "$B" replay "$v" $f --truth "tests/truth/$t.txt" > "$T/$t.txt" 2>&1 ) &
  i=$((i+1))
  if [ $((i % 4)) -eq 0 ]; then wait; fi
done
wait
python - "$T" <<'PY'
import os,re,sys
T=sys.argv[1]
tot={}
print(f"{'recording':12} {'exact':>7} {'miss':>4} {'extra':>5} {'locks':>5} {'wrong':>5} {'frames right':>13} {'first lock':>10}")
L=W=F=FR=0
for name in ['c1','c2','v2','v3','v6','headon_jaw','partial_jaw','clean','repeat1','repeat2','noangle2','skybusy','english1','slow_eye']:
    text=open(f'{T}/{name}.txt').read()
    m=re.search(r'flutters in truth (\d+): exact (\d+), wrong count (\d+), wrong start (\d+), missed (\d+), extra (\d+)',text)
    l=re.search(r'locks (\d+): state right (\d+), wrong (\d+)',text)
    w=re.search(r'window frames right (\d+)/(\d+)',text)
    s=re.search(r'each lock: \[([\d.]+)',text)
    L+=int(l[1]);W+=int(l[3]);FR+=int(w[1]);F+=int(w[2])
    print(f"{name:12} {m[2]+'/'+m[1]:>7} {m[5]:>4} {m[6]:>5} {l[1]:>5} {l[3]:>5} {w[1]+'/'+w[2]:>13} {(s[1] if s else 'none'):>10}")
    e=re.search(r'early windows.*?: (\{.*\})',text)
    if e:
        for k,a,b,c in re.findall(r'(\d+): \((\d+), (\d+), (\d+)\)',e[1]):
            t=tot.setdefault(int(k),[0,0,0]);t[0]+=int(a);t[1]+=int(b);t[2]+=int(c)
    d=re.search(r'early at 0.6\+: (.*)',text)
    if d and os.environ.get('SHOW_EARLY'):
        print('   ',d[1][:400])
print(f'locks {L}, wrong {W}, window frames {FR}/{F}')
for k in sorted(tot):
    n,r,al=tot[k]
    print(f'  chance {k*10:3d}-{k*10+9:3d}%: {n:3d} windows, target right {r:3d} ({100*r/n:3.0f}%), every frame right {al:3d}')
PY
rm -rf "$T"
