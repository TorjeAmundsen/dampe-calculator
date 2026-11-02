# dampe-calculator

Gets the Heart Piece from Dampe's digging game in Ocarina of Time on demand, instead of the usual 1 in 10.

You press a key for each burst of Dampe's blinks, it works out the game's RNG from when they came, and it counts down to the
moment to press A. **It never sends any input to the game, and this version doesn't read video at all.** There's a separate
build that reads the blinks from a video feed, described [below](#the-video-build).

It's been tested on one Wii VC setup, so expect rough edges, and please report what you find.

## What you need

- Windows 10 or 11, 64-bit, or 64-bit Linux. On Linux the keys only work with the window focused, and it hasn't been tried.
- OoT on **N64 or Wii VC, NTSC**. GameCube doesn't work, since its RNG differs. PAL hasn't been tested and isn't supported.
  Only Wii VC has been tried.

## Setup

The quickest way is [in your browser](https://torjeamundsen.github.io/dampe-calculator/), with nothing to download. There, keys 3
and 4 only work with its tab focused, so use the program below if you need them while another program has focus.

Otherwise, download the zip for your system from the [releases page](https://github.com/TorjeAmundsen/dampe-calculator/releases), take
`dampe.exe` or `dampe` from it, and run it. Nothing needs installing. Windows may warn about an unknown publisher, since the
program isn't signed: choose More info, then Run anyway. It starts on Wii VC: on N64, choose that under **Settings** in its window.

## Using it

Before talking to Dampe, make sure of these, since they draw RNG:

- The Gold Skulltula on the graveyard wall is dead, and no soil Skulltulas or released bugs are around.
- It isn't raining.
- You have more than 1 heart. Critical health makes the camera draw RNG.

Then:

1. Press **Start over** (**F8**) if you've pressed anything since your last talk. Nothing here can see a new talk or a dig, so old
   presses otherwise carry over.
2. Talk to him with his eyes visible, and wait for Navi to finish, since her sparkles draw RNG. On a repeat talk she comes
   about 3-4 seconds after the Yes/No box opens. Go to the Yes/No box whenever you like, and stay on it.
3. Press **3** or **4** as each burst of blinks ends, for how many blinks it had. Press either again within 1.3 s to flip a count
   you got wrong.
4. When a countdown starts, press A on Yes as it reaches zero. Anticipate it like a rhythm game rather than reacting. The dig ends
   what's been read, so Start over before talking to him again.

It takes about 14 flutters to be certain, which is 40 seconds or so, plus the wait for a good moment. You can stop pressing once
it's locked. Telling 3 from 4 takes practice, and sloppy timing or counts mostly cost time.

By default keys 3 and 4 work from any program, and other programs can't get them while it's on. Turn that off in Settings
if you need to type them, and then they only work with the window focused. It stays how you left it.

### Green and amber

- **Green** means a state of the RNG is proven, though your own press timing still counts. If a green press gives a rupee, the
  state may have been wrong, so start over.
- **Amber** comes earlier, with a percentage, before anything is proven. It may give a rupee, and you can wait for green if you
  prefer. Aim for zero as exactly as you can.

A green cue only cancels if 3 presses in a row don't match. An amber one cancels if a press takes its chance under 70%. Stopping
never cancels either.

### The list of candidates

Under the countdown are the 3 likeliest states of the RNG, once there are any. The bar behind each is its chance of being the real
one. Each shows how many of your presses it explains (hit), flutters you didn't press (miss) and presses it can't explain (stray).
A row is gray until a state is locked, then green. The time on the right counts to the moment to press if that state is the real one, so you can go for one that hasn't become the main countdown
yet, at your own risk. "passed" means that moment has gone by, and "over 60 s" that none is coming within a minute.

### Timing

The countdown allows for your platform's input latency (2 game frames on N64, 3 on Wii VC) and 330 ms for how late your presses
are after a flutter ends. Nothing here can see when the box really left, so it can't correct that by itself. If green presses
keep failing, `--lag-ms` changes the 330.

## Options

    dampe live [--platform n64|vc] [--lag-ms <ms>] [--start-over-key <F1-F24>]

Running it with no options is `dampe live`. `--start-over-key` picks another key for Start over. Sound is off by default and set under
**Settings** in the window, with your platform. They're saved in `%APPDATA%\dampe-calculator`, or `~/.config/dampe-calculator` on Linux.

## Reporting problems

The **Log** button under **Settings** shows `dampe.log` in the settings folder, and the run before it is `dampe.previous.log`. Please include it, and
say your platform and what each dig gave, especially the percentage shown for amber presses.

## The video build

Built with `--features video`, this watches Dampe's eyes in your video feed instead of you pressing keys. Video goes in and a
countdown comes out. The rest of this section is about that build.

### What you need

- Windows 10 or 11, 64-bit, or 64-bit Linux. Linux hasn't been tried with a real feed yet, so expect more rough edges there.
- OoT on **N64 or Wii VC, NTSC**. GameCube doesn't work, since its RNG differs. PAL hasn't been tested and isn't supported.
- A capture card, and OBS Studio with Virtual Camera. Other video devices can work with `--device`.

### Setup

1. Download the zip for your system from the [releases page](https://github.com/TorjeAmundsen/dampe-calculator/releases), and take `dampe.exe` or `dampe` from it. Nothing needs installing. Windows may warn about an unknown publisher,
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

### Using it

Before talking to Dampe, make sure of these, since they draw RNG the program can't see:

- The Gold Skulltula on the graveyard wall is dead, and no soil Skulltulas or released bugs are around.
- It isn't raining.
- You have more than 1 heart. Critical health makes the camera draw RNG.

Then:

1. Talk to him with his eyes visible. It starts reading at his first text box, and counts his blinks once Navi is done.
2. Go through the text to the Yes/No box whenever you like, and stay on that box until the countdown.
3. When the countdown starts, press A on Yes as it reaches zero. Anticipate it like a rhythm game rather than reacting.

It takes about 25 seconds to be certain when his eyes are clear, plus the wait for a good moment, which can be anything
from a few seconds to half a minute.

#### Green and amber

- **Green** means a state of the RNG is proven. On test recordings every green moment was right, though your own press
  timing still counts.
- **Amber** comes earlier, with a percentage, before anything is proven. It may give a rupee, and you can wait for green
  if you prefer. Aim for zero as exactly as you can, since the percentage is for that moment.

A cue can cancel if the video stops matching. If a new countdown starts right away, follow the new one.

#### The list of candidates

Above the feed are the likeliest states of the RNG it's considering, as many as fit the window. The line above them says how
many there are, and the chance the real one is none of them. Many can share a seed, differing only in Link's first blink,
until he blinks. The bar behind each is its
chance of being the real one, and its color says how far it's got: gray while searching, blue once it has predicted a blink,
and green with both dots lit, which is what proving a state takes. Each shows its seed and the frame of Link's first blink,
then how it matches what was seen: blinks it expected and saw (hit), expected and didn't see (miss), and saw but didn't expect
(stray). The time on the right is when that state alone would give the Heart Piece, so it isn't a cue to follow.

#### Angles

His eyes have to be visible. If they're fully hidden for part of his animation, by his jaw or Link, it still gets there
but can take over a minute. Talking to him again from a clearer angle is usually faster. If the camera ends up behind
Link and his eyes can't be seen at all, it can't read him, so you'll need a new angle.

Starting a new talk resets it by itself. Use **Start over** if it seems stuck.

#### Navi and Force start

Navi's sparkles draw RNG, so nothing before she's done can count. It sees her bright light and starts over until it's gone. On a first talk she's done before the Yes/No box. On a
repeat talk the box opens right away and she comes about 3-4 seconds later, and it waits for her by itself.

You normally never need **Force start** (**F8**), because reading starts by itself at his first text box. Don't press it
to get things going, like in a loading zone or while walking up to him: it reads whatever is on screen. A text box opening
replaces a read forced before it, so that only costs time until then.

Use it with a text box up if it isn't seeing Navi finish, or with no box if it won't start at all. Only blinks from that
moment on count, so if Navi is around, wait until she's done. F8 works from any program, and only listens to your keyboard.
On Linux it only works while the window has focus. `--navi-key F9` picks another function key.

#### Timing

The countdown allows for your platform's input latency (2 game frames on N64, 3 on Wii VC) and the delay of your feed,
which it measures from your presses and remembers. The first few presses may be slightly off, and that's normal.
`--feed-delay-ms` sets the delay by hand instead.

### Options

    dampe live [--device <name>] [--platform n64|vc] [--feed-delay-ms <ms>] [--navi-key <F1-F24>]
               [--no-window] [--no-sound]
    dampe devices    list video devices
    dampe diag       measure what the feed delivers, for reporting problems

Running it with no options is `dampe live`, which reads video in this build, and `dampe live --manual` is the manual mode above. Sound is off by default and set in the window, and it's always on
with `--no-window`, which has no other cue. Settings are in `%APPDATA%\dampe-calculator`, or `~/.config/dampe-calculator` on Linux.

### Reporting problems

The log has what's needed, so please include it. The **Log** button shows the file, which is
`dampe.log` in the settings folder, and the run before it is `dampe.previous.log`. Also say your platform, capture setup, and what each dig gave. For
amber presses, the percentage shown and the result are especially useful. Known limits:

- Tested on Wii VC, mostly with Japanese text. N64 and analog capture are untested.
- On Linux the sound cue goes through an external player, whose delay isn't measured, so trust the window over the sound.
- If it keeps saying to talk to Dampe with a text box clearly up, it isn't recognizing the box. Force start gets it going.

## How it works

The start and count of each burst of blinks come from 2 RNG draws, and N64 and VC only use 23 bits of the RNG. So a number of them
pin down the state, even with a person's timing being a few frames off. Every state is compared to your presses, around how late
presses run in general, which needs no calibrating. The game's code then runs forward to find when a dig rolls the Heart Piece.

The video build reads the same bursts from the picture. Dampe's idle animation repeats every 49 game frames, drawn the same way
each time, so a frame minus earlier frames from the same point of the loop leaves only what really changed, and his eye is what
goes half closed, closed, open, one game frame each, 3 or 4 times. No face tracking is needed.

## Building and testing

Needs Rust. `cargo build --release` builds the manual version with a console, with no code for reading video in it, and adding
`--features video` builds the video version. `./release.ps1` builds the release zips into `dist/`, `./release.ps1 --video` the video
ones, and the Windows ones have no console. On Windows, building the Linux zip takes `cargo-zigbuild` and Zig.
`dampe replay` needs `ffmpeg` on the PATH, but using the program doesn't.

## License

GPL-3.0. The capture code is adapted from AutoSplit-rs. Thanks to the [OoT decompilation](https://github.com/zeldaret/oot)
project, and to the [Dampe HP RNG calculator](https://pagelens.dev/dampe-hp-rng-calculator/) that showed this could be done.
