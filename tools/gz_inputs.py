"""Buttons and stick from gz's input display, for a 1920x1080 recording with it at its default place.

usage: python tools/gz_inputs.py <video> <out.txt>

Writes a line per change: video_frame stick_x stick_y buttons. Templates in gz_input_templates.npz
came from recordings/inputs.mkv. L and the D-pad do nothing in the game here, so they aren't read.
"""
import os, subprocess, sys
import numpy as np

X0, Y0, W, H = 230, 1020, 1460, 50
ICON_ROWS = (8, 42)
ICON_HALF = 17
# Mean difference over an icon's own pixels that still counts as it showing. Icons are opaque.
MAX_ICON_DIFF = 25
BUTTONS = 'A B Start CU CD CL CR Z R'.split()
# Right edge of each digit cell, from ones to the sign, for each number
CELL_ENDS = ((221, 185, 149, 113), (407, 370, 334, 298))
CELL_W, CELL_ROWS = 36, (4, 46)
MIN_CELL_PIXELS = 25
MAX_CELL_DIFF = 0.12


def read_number(lum, ends, digits, labels):
    text = ''
    for end in ends:
        cell = lum[CELL_ROWS[0]:CELL_ROWS[1], end - CELL_W + 1:end + 1] > 150
        if cell.sum() < MIN_CELL_PIXELS:
            continue
        diffs = (digits != cell.ravel()).mean(axis=1)
        best = diffs.argmin()
        text = (labels[best] if diffs[best] < MAX_CELL_DIFF else '?') + text
    return text or '?'


def main(path, out):
    t = np.load(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'gz_input_templates.npz'))
    icons = []
    for name in BUTTONS:
        icon = t['icon_' + name].astype(np.int16)
        icons.append((name, int(t['x_' + name]), icon, icon.max(axis=2) > 110))
    digits = t['digits'].reshape(len(t['digits']), -1)
    labels = t['labels']
    p = subprocess.Popen(['ffmpeg', '-v', 'error', '-i', path, '-vf', f'crop={W}:{H}:{X0}:{Y0}', '-f', 'rawvideo',
                          '-pix_fmt', 'rgb24', '-'], stdout=subprocess.PIPE, bufsize=W * H * 12)
    n = 0
    last = None
    lines = []
    while True:
        b = p.stdout.read(W * H * 3)
        if len(b) < W * H * 3:
            break
        a = np.frombuffer(b, np.uint8).reshape(H, W, 3).astype(np.int16)
        lum = a.max(axis=2)
        x = read_number(lum, CELL_ENDS[0], digits, labels)
        y = read_number(lum, CELL_ENDS[1], digits, labels)
        pressed = []
        for name, cx, icon, mask in icons:
            slot = a[ICON_ROWS[0]:ICON_ROWS[1], cx - ICON_HALF:cx + ICON_HALF + 1]
            if np.abs(slot - icon)[mask].mean() < MAX_ICON_DIFF:
                pressed.append(name)
        now = (x, y, ' '.join(pressed))
        if now != last:
            lines.append(f'{n} {x} {y} {now[2]}'.rstrip())
            last = now
        n += 1
    with open(out, 'w', newline='\n') as f:
        f.write('# video_frame stick_x stick_y buttons, one line per change, read from gz input display\n')
        f.write('\n'.join(lines) + '\n')
    print('video frames', n, 'changes', len(lines), 'unread numbers', sum('?' in l.split()[1] + l.split()[2] for l in lines))


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
