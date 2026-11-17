"""Cuts a clip out of a 1920x1080 recording for the docs, as an animated WebP or an MP4.

usage: python tools/clip.py <recording> <out.webp|out.mp4> [options]

  --start T          where the clip starts, in seconds or m:ss (default 0)
  --end T            where it ends, or
  --duration T       how long it runs
  --layout side      the 480x320 calculator from the top left beside the game feed, scaled down (default)
  --layout whole     the whole picture, for a recording of one window on its own
  --crop W:H:X:Y     any region instead of a layout, like a close-up of his eye
  --game-height H    the game feed's height in the side layout, keeping its 4:3 shape (default 360, exactly a
                     third, so every pixel stays the same size). The calculator gets black below it to match
  --width W          scale the whole layout to this width, keeping its shape
  --fps N            frames per second (default 30). The game runs at 20, so 30 loses nothing of it
  --speed X          play this many times faster, for skipping through a long wait, or 0.5 for half speed
  --quality Q        WebP quality from 0 to 100 (default 80)
  --lossless         lossless WebP: sharpest text, biggest file

The side layout expects the calculator at the top left, 480x320, and the game feed right of it, 1440x1080
from x 480. By default that makes 960x360. The game feed is scaled with nearest neighbour, so its pixels stay sharp. MP4s keep their
sound unless sped up.
"""
import argparse
import os
import subprocess
import sys

CALCULATOR = (480, 320)
GAME = (480, 0, 1440, 1080)


def seconds(text):
    """Seconds from text like 75, 75.5 or 1:15.5."""
    total = 0.0
    for part in text.split(':'):
        total = total * 60 + float(part)
    return total


def video_filter(args):
    if args.crop:
        chain = f'[0:v]crop={args.crop}'
    elif args.layout == 'side':
        height = args.game_height
        width, _ = CALCULATOR
        x, y, game_width, game_height = GAME
        calculator = f'[0:v]crop={width}:{CALCULATOR[1]}:0:0'
        # A taller game feed leaves the calculator a strip of black underneath
        if height > CALCULATOR[1]:
            calculator += f',pad={width}:{height}:0:0:color=black'
        scaled_width = round(height * game_width / game_height / 2) * 2
        game = f'[0:v]crop={game_width}:{game_height}:{x}:{y},scale={scaled_width}:{height}:flags=neighbor'
        chain = f'{calculator}[calculator];{game}[game];[calculator][game]hstack=inputs=2'
    else:
        chain = '[0:v]null'
    if args.width:
        chain += f',scale={args.width}:-2:flags=neighbor'
    if args.speed != 1:
        chain += f',setpts=PTS/{args.speed}'
    return chain + f',fps={args.fps}[out]'


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('recording')
    parser.add_argument('output')
    parser.add_argument('--start', default='0')
    ends = parser.add_mutually_exclusive_group()
    ends.add_argument('--end')
    ends.add_argument('--duration')
    parser.add_argument('--layout', choices=['side', 'whole'], default='side')
    parser.add_argument('--crop')
    parser.add_argument('--game-height', type=int, default=GAME[3] // 3)
    parser.add_argument('--width', type=int)
    parser.add_argument('--fps', type=float, default=30)
    parser.add_argument('--speed', type=float, default=1.0)
    parser.add_argument('--quality', type=int, default=80)
    parser.add_argument('--lossless', action='store_true')
    args = parser.parse_args()

    kind = os.path.splitext(args.output)[1].lower()
    if kind not in ('.webp', '.mp4'):
        sys.exit('The output has to end in .webp or .mp4')
    if args.game_height < CALCULATOR[1]:
        sys.exit(f'--game-height has to be at least {CALCULATOR[1]}, the calculator\'s height')

    start = seconds(args.start)
    command = ['ffmpeg', '-v', 'error', '-stats', '-y', '-ss', str(start)]
    if args.end:
        command += ['-t', str(seconds(args.end) - start)]
    elif args.duration:
        command += ['-t', str(seconds(args.duration))]
    command += ['-i', args.recording, '-filter_complex', video_filter(args), '-map', '[out]']

    if kind == '.webp':
        quality = ['-lossless', '1'] if args.lossless else ['-lossless', '0', '-quality', str(args.quality)]
        command += ['-c:v', 'libwebp_anim', *quality, '-loop', '0', '-an']
    else:
        command += ['-c:v', 'libx264', '-crf', '18', '-pix_fmt', 'yuv420p', '-movflags', '+faststart']
        if args.speed == 1:
            command += ['-map', '0:a?', '-c:a', 'aac', '-b:a', '160k']
    command.append(args.output)

    if subprocess.run(command).returncode != 0:
        sys.exit('ffmpeg failed')
    print(f'{args.output}: {os.path.getsize(args.output) / 1024:.0f} KB')


if __name__ == '__main__':
    main()
