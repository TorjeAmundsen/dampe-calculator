# dampe-calculator

Gets the Heart Piece from Dampe's digging game in Ocarina of Time on demand, instead of the usual 1 in 10.

You press a key for each burst of Dampe's blinks, it works out the game's RNG from when they came, and it counts down to the
moment to press A. A separate proof-of-concept build reads the blinks from a video feed instead, described in [VIDEO.md](VIDEO.md).

It's been tested on one Wii VC setup, so expect rough edges, and please report what you find.

<!-- ![The manual window beside the game through a whole read, from the first press to the Heart Piece](assets/docs/manual-read.mp4) -->
<!-- Placeholder, to record: MP4 of the game and the manual window side by side. Talk to Dampe, press 3s and 4s, the lock, the countdown, A on Yes, and the Heart Piece. Make it with python tools/clip.py <recording> assets/docs/manual-read.mp4 --start <m:ss> --end <m:ss>. Then upload it by dragging the file into this file in GitHub's web editor, which puts a github.com/user-attachments link here: replace the image line above with that link and delete this comment. -->

## Setup

It works with OoT on **N64 or Wii VC, NTSC**, and has only been tried on Wii VC. GameCube's RNG differs, and PAL isn't supported.

The quickest way is [in your browser](https://torjeamundsen.github.io/dampe-calculator/), with nothing to download. Its keys 3
and 4 only work with the tab focused. For keys that work from any program, download the zip for Windows or Linux, 64-bit, from
the [releases page](https://github.com/TorjeAmundsen/dampe-calculator/releases) and run the program in it. Nothing needs
installing. Windows may warn about an unknown publisher, since the program isn't signed: choose More info, then Run anyway. On
Linux the keys only work with the window focused, and it hasn't been tried.

Both start on Wii VC, so on N64, choose that under **Settings**.

## Using it

Before talking to Dampe, make sure of these, since they draw RNG:

- The Gold Skulltula on the graveyard wall is dead, and no soil Skulltulas or released bugs are around.
- It isn't raining.
- You have more than 1 heart. Critical health makes the camera draw RNG.

Then:

1. Press **Start over** (**F8**) if you've pressed anything since your last talk. Nothing here can see a new talk or a dig, so old
   presses otherwise carry over.
2. Talk to him with his eyes visible, and wait for Navi to go away, since her sparkles draw RNG. On a repeat talk she comes
   about 3-4 seconds after the Yes/No box opens. Go to the Yes/No box whenever you like, and stay on it.
3. Press **3** or **4** as each burst of blinks ends, for how many blinks it had. Press either again within 1.3 s to flip a count
   you got wrong.
4. When a countdown starts, press A on Yes as it reaches zero. Anticipate it like a rhythm game rather than reacting. The dig ends
   what's been read, so Start over before talking to him again.

<!-- ![The manual window partway through a read, with candidates listed](assets/docs/manual-reading.png) -->
<!-- Placeholder, to record: screenshot of the manual window mid-read, with a few candidates and the recent presses showing. -->


It takes about 14 flutters to be certain, which is 40 seconds or so, plus the wait for a good moment. [GUIDE.md](GUIDE.md) is
about getting that lower. You can stop pressing once
it's locked, and with sound on, two rising notes say when that is. Pressing on can still help: the states left can differ in
when Link next blinks, and later flutters rule some out, which can bring an earlier window. Telling 3 from 4 takes practice, and sloppy timing or counts mostly cost time.

By default keys 3 and 4 work from any program, and other programs can't get them while it's on. Turn that off in Settings
if you need to type them, and then they only work with the window focused. It stays how you left it.

### Angles

Stand where both of his eyes stay clear of his jaw and of Link through his whole idle animation. If one is hidden for part of it,
you'll miss flutters, and each miss costs time. Talking to him again from a clearer angle is often faster.

<!-- ![An angle where both of his eyes stay clear](assets/docs/angle-clear.png) -->
<!-- Placeholder, to record: screenshot of the game from a good angle, eyes clear of his jaw and Link. -->

<!-- ![An angle where his jaw hides an eye](assets/docs/angle-hidden.png) -->
<!-- Placeholder, to record: screenshot of the game from a bad angle, his jaw or Link covering an eye for part of his idle animation. -->

### The Ocarina

Once it's locked it also looks for an earlier Heart Piece by freezing Dampe. While the Ocarina is out his blink timer stops and
Link's keeps going, so Link's single draw can come before Dampe's next pair. When one saves at least 2 seconds, it replaces the
usual countdown with three, and the line under the countdown says "Ocarina". It's on by default, and **Ocarina** in Settings turns
it off. It stays how you left it.

1. **Say No and pull Ocarina:** hold the stick away from Dampe and press No at zero, so Link doesn't face him and bring Navi out.
   Mash the text closed and have the Ocarina out within 15 game frames (0.75 s).
2. **Cancel Ocarina and talk to Dampé:** stay still until zero, then put it away and talk to him facing away. Mash the text: there
   are at least 30 game frames (1.5 s) to the Yes/No box.
3. **Press A on Yes** at zero.

<!-- ![An Ocarina plan followed from the first countdown to the Heart Piece](assets/docs/ocarina-plan.mp4) -->
<!-- Placeholder, to record: MP4 of the game and window through all three Ocarina countdowns: No while holding away, Ocarina out, put away, talking facing away, Yes, and the Heart Piece. Make it with python tools/clip.py <recording> assets/docs/ocarina-plan.mp4 --start <m:ss> --end <m:ss>. Then upload it by dragging the file into this file in GitHub's web editor, which puts a github.com/user-attachments link here: replace the image line above with that link and delete this comment. -->

Don't run far while holding away: leaving his view or getting too far makes him walk, which draws RNG. The freeze has been checked
against the RNG watch, but no plan has been followed to a Heart Piece yet.

### Green and amber

- **Green** means a state of the RNG is proven, though your own press timing still counts. If a green press gives a rupee, the
  state may have been wrong, so start over.
- **Amber** comes earlier, with a percentage, before anything is proven. It may give a rupee, and you can wait for green if you
  prefer. Aim for zero as exactly as you can.

<!-- ![An amber countdown with its percentage](assets/docs/amber-cue.png) -->
<!-- Placeholder, to record: screenshot of the manual window during an amber countdown. -->

<!-- ![A green countdown](assets/docs/green-cue.png) -->
<!-- Placeholder, to record: screenshot of the manual window during a green countdown. -->

A green cue only cancels if 3 presses in a row don't match. An amber one cancels if a press takes its chance under 70%. Stopping
never cancels either.

### The list of candidates

Under the countdown are the 3 likeliest states of the RNG, once there are any. The bar behind each is its chance of being the real
one. Each shows how many of your presses it explains (hit), flutters you didn't press (miss) and presses it can't explain (stray).
A row is gray until a state is locked, then green. The time on the right counts to the moment to press if that state is the real one, so you can go for one that hasn't become the main countdown
yet, at your own risk. "passed" means that moment has gone by, and "over 60 s" that none is coming within a minute.

<!-- ![The list of candidates once a state is locked](assets/docs/candidates-locked.png) -->
<!-- Placeholder, to record: screenshot of the manual window just after a lock, with the green rows and their times. -->

### Timing

The countdown allows for your platform's input latency (2 game frames on N64, 3 on Wii VC), and assumes you press each flutter
about 330 ms after it ends, which is what logged reads averaged. It never sees the game, so it can't learn your own lateness. If
green presses keep giving rupees, your presses may run earlier or later than that: set your own under **Lag** in the browser's
Settings, or with `--lag-ms` for the program.

## Options

    dampe live [--platform n64|vc] [--lag-ms <ms>] [--start-over-key <F1-F24>]

Running it with no options is `dampe live`. `--start-over-key` picks another key for Start over. Sound is off by default and set under
**Settings** in the window, with your platform. They're saved in `%APPDATA%\dampe-calculator`, or `~/.config/dampe-calculator` on Linux.

## Reporting problems

The **Log** button under **Settings** shows `dampe.log` in the settings folder, and the run before it is `dampe.previous.log`. Please include it, and
say your platform and what each dig gave, especially the percentage shown for amber presses.

## How it works

The start and count of each burst of blinks come from 2 RNG draws, and N64 and VC only use 23 bits of the RNG. So a number of them
pin down the state, even with a person's timing being a few frames off. Every state is compared to your presses, around how late
presses run in general, which needs no calibrating. The game's code then runs forward to find when a dig rolls the Heart Piece.

The [video build](VIDEO.md) reads the same bursts from the picture instead.

## Building and testing

Needs Rust. `cargo build --release` builds the manual version with a console, with no code for reading video in it, and adding
`--features video` builds the video version. `./release.ps1` builds the release zips into `dist/`, `./release.ps1 --video` the video
ones, and the Windows ones have no console. On Windows, building the Linux zip takes `cargo-zigbuild` and Zig.
`dampe replay` needs `ffmpeg` on the PATH, but using the program doesn't.

## License

GPL-3.0. The capture code is adapted from AutoSplit-rs. Thanks to the [OoT decompilation](https://github.com/zeldaret/oot)
project, and to the [Dampe HP RNG calculator](https://pagelens.dev/dampe-hp-rng-calculator/) that showed this could be done.
