# dampe-rng

Reads Dampe's blinks from a video feed to recover Ocarina of Time's RNG state, then cues you to press A so the dig
gives the Heart Piece. N64 and Wii VC, NTSC. GameCube and PAL aren't supported.

Video goes in, and a countdown comes out. It never sends any input to the game, and it won't.

## Use

1. In OBS, set Configure Virtual Camera to the game's source, then click Start Virtual Camera. Do this first.
2. Run `dampe live`. Use `--device <name>` for anything but OBS Virtual Camera, and `dampe devices` to list them.
3. Pay Dampe, stop him on a soil patch, and talk until the Yes/No box is open with Yes highlighted. Dampe's eyes have
   to be visible. If the camera ended up behind Link, talk to him again from another angle, then press Enter in the
   terminal to start over.
4. Wait for the countdown in the window, and press A as it reaches zero. It flashes on the last 3 seconds, and on the
   moment itself. Anticipate it like a rhythm game. Sound is off unless you turn it on in the window, and it's always
   on with `--no-window`, which has no other cue.
5. If you've talked to Dampe before, the box opens right away, while Navi is still flying around, and her sparkles
   draw RNG. It waits for her by itself, from how the picture looks. If that's wrong, press F8 once she has settled, or
   click Navi gone in the window, and only flutters from then on count. `--navi-key` picks another function key.
6. Choose your platform in the window, or with `--platform n64` or `--platform vc`. The countdown allows for its input
   latency (2 or 3 game frames) and the delay of your feed, which it measures from your presses and remembers.
   `--feed-delay-ms` sets that delay by hand instead.

These all draw RNG and break the read, so avoid them while it's reading: a live Gold Skulltula, released bugs, rain,
1 heart or less, walking, moving the camera, Navi's sparkles, and pausing. A mask makes Dampe skip the Yes/No box.

## How it works

Dampe's idle animation loops every 49 game frames (`gDampeRestAnim`'s `frameCount`), and the game draws the same pose
the same way each time. So a frame minus earlier frames from the same point of the loop leaves only what really
changed. Each blob of change is followed on its own, and only the eye goes half closed, closed, open, one game frame
each, 3 or 4 times. No face tracking is needed.

Each flutter's start and count come from 2 RNG draws, so a few of them pin down the 2^23 possible states (N64 and VC
use only the low 23 bits). A state only counts after it correctly predicted 3 more flutters, and contradicting the
video cancels it. Then the model runs forward to find when a roll is at least 0.9.

## Testing

`tests/truth` has the RNG value of every game frame in two recordings, read from a gz memory watch.

    dampe replay <video> --truth tests/truth/c1.txt

scores each lock by comparing its predicted state to the watch on every following frame, and each Heart Piece window
by the roll the watch's values would really give. `--session` runs a recording through the same code as `dampe live`. Recordings without color need `--no-box`, because the
Yes/No box is found by its colors.
