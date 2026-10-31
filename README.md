# dampe-rng

Gets the Heart Piece from Dampe's digging game in Ocarina of Time on demand, instead of the usual 1 in 10.

It watches Dampe's eyes in your video feed, works out the game's RNG from his blinks, and counts down to the moment to
press A. **Video goes in and a countdown comes out. It never sends any input to the game.**

This is a pre-release. It has been tested most on one Wii VC setup, so expect rough edges, and please report what you
find.

## What you need

- Windows 10 or 11, 64-bit, or 64-bit Linux. Linux hasn't been tried with a real feed yet, so expect more rough edges there.
- OoT on **N64 or Wii VC, NTSC**. GameCube doesn't work, since its RNG differs. PAL hasn't been tested and isn't supported.
- A capture card, and OBS Studio with Virtual Camera. Other video devices can work with `--device`.

## Setup

1. Download the zip for your system from the releases page, and take `dampe.exe` or `dampe` from it. Nothing needs installing. Windows may warn about an unknown publisher,
   since the program isn't signed: choose More info, then Run anyway.
2. In OBS, open Configure Virtual Camera next to Start Virtual Camera, and set it to the source with your game on it, so
   your stream layout isn't included.
3. Click **Start Virtual Camera**. Do this before starting the program.
4. Run it. In its window, choose N64 or Wii VC.

Use a stable 60 FPS in OBS if you can. It has been used a lot while OBS was recording, and it needs some CPU: about half
of one core on average, more in short bursts while it searches.

On Linux, Virtual Camera needs the `v4l2loopback` module, and sound needs `paplay`, `pw-play` or `aplay`. Your user needs
access to `/dev/video*`, usually through the `video` group. Devices that only offer MJPEG don't work, so use Virtual Camera
for those. Start it from a terminal the first time, since errors only show there.

## Using it

Before talking to Dampe, make sure of these, since they draw RNG the program can't see:

- The Gold Skulltula on the graveyard wall is dead, and no soil Skulltulas or released bugs are around.
- It isn't raining.
- You have more than 1 heart. Critical health makes the camera draw RNG.

Then:

1. Talk to him until the Yes/No box is up, with his eyes visible. It starts reading when it sees the box's red, green and
   blue text.
2. Stay on the box while it reads. Pressing buttons to get through the text before it is fine.
3. When the countdown starts, press A on Yes as it reaches zero. Anticipate it like a rhythm game rather than reacting.

It takes about 25 seconds to be certain when his eyes are clear, plus the wait for a good moment, which can be anything
from a few seconds to half a minute.

### Green and amber

- **Green** means a state of the RNG is proven. On test recordings every green moment was right, though your own press
  timing still counts.
- **Amber** comes earlier, with a percentage, before anything is proven. It may give a rupee, and you can wait for green
  if you prefer. Aim for zero as exactly as you can, since the percentage is for that moment.

A cue can cancel if the video stops matching. If a new countdown starts right away, follow the new one.

### Angles

His eyes have to be visible. If they're fully hidden for part of his animation, by his jaw or Link, it still gets there
but can take over a minute. Talking to him again from a clearer angle is usually faster. If the camera ends up behind
Link and his eyes can't be seen at all, it can't read him, so you'll need a new angle.

Starting a new talk resets it by itself. Use **Start over** if it seems stuck.

### Navi and starting by hand

Navi's sparkles draw RNG, so nothing before she's done can count. On a first talk she's done before the Yes/No box. On a
repeat talk the box opens right away and she comes about 3-4 seconds later, and it waits for her by itself.

Press **F8** or click **Start now** to start reading before the Yes/No box. This is useful on angles where Navi doesn't
appear at all, since his blinks count during all of his text. Press it once his first text box is up and the camera has
settled, since it can't read a moving picture. Only blinks from that moment on count, so if Navi is around, wait until
she's done. F8 works from any program, and only listens to your keyboard. `--navi-key F9` picks another function key. On Linux, F8 only
works while the window has focus.

### Timing

The countdown allows for your platform's input latency (2 game frames on N64, 3 on Wii VC) and the delay of your feed,
which it measures from your presses and remembers. The first few presses may be slightly off, and that's normal.
`--feed-delay-ms` sets the delay by hand instead.

## Options

    dampe live [--device <name>] [--platform n64|vc] [--feed-delay-ms <ms>] [--navi-key <F1-F24>]
               [--no-window] [--no-sound]
    dampe devices    list video devices
    dampe diag       measure what the feed delivers, for reporting problems

Running `dampe.exe` with no options is `dampe live`. Sound is off by default and set in the window, and it's always on
with `--no-window`, which has no other cue. Settings are in `%APPDATA%\dampe-rng`, or `~/.config/dampe-rng` on Linux.

## Reporting problems

The log has what's needed, so please include it. The **Log** button shows the file, which is
`dampe.log` in the settings folder, and the run before it is `dampe.previous.log`. Also say your platform, capture setup, and what each dig gave. For
amber presses, the percentage shown and the result are especially useful. Known limits:

- Only tested on Wii VC with Japanese text so far. N64, English text and analog capture are untested.
- On Linux the sound cue goes through an external player, whose delay isn't measured, so trust the window over the sound.
- If it stays on "Waiting for the Yes/No box" with the box clearly up, it isn't recognizing the box's colors.

## How it works

Dampe's idle animation repeats every 49 game frames, drawn the same way each time. So a frame minus earlier frames from
the same point of the loop leaves only what really changed, and his eye is what goes half closed, closed, open, one game
frame each, 3 or 4 times. No face tracking is needed.

The start and count of each burst of blinks come from 2 RNG draws, and N64 and VC only use 23 bits of the RNG. So a few of
them pin down the state. A state only counts as proven after 6 matches and 2 correct predictions, and contradicting the
video drops it. The game's code then runs forward to find when a dig rolls the Heart Piece.

## Building and testing

Needs Rust. `cargo build --release` builds it with a console. `./release.ps1` builds the release zips into `dist/`, and the
Windows one has no console. On Windows, building the Linux zip takes `cargo-zigbuild` and Zig.
`dampe replay` needs `ffmpeg` on the PATH, but using the program doesn't.

## License

GPL-3.0. The capture code is adapted from AutoSplit-rs. Thanks to the [OoT decompilation](https://github.com/zeldaret/oot)
project, and to the [Dampe HP RNG calculator](https://pagelens.dev/dampe-hp-rng-calculator/) that showed this could be done.
