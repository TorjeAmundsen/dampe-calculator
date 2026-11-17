# Getting the earliest Heart Piece

This guide is about the manual calculator, where you press 3 or 4 for each burst of Dampé's blinks. The README covers setup. Here
the question is what to do so it locks as early as possible, and what to press when.

A lock takes about 14 presses for a typical player and about 11 for a precise one. Dampé flutters about every 2.7 seconds, so that
gap is 8 seconds per read. Almost all of it comes down to two things: getting 3 and 4 right, and pressing at the same moment of
every flutter. The numbers below come from 80 simulated reads per row, scored against the real RNG, plus five real reads.

## Before you talk

Clear anything else that draws RNG first. That means the Gold Skulltula on the wall, soil Skulltulas, released bugs, rain and
critical health. Press **Start over** (F8) if you've pressed anything since your last talk.

Turn on **Sound** under Settings. It plays two rising notes when the read locks, so you can keep your eyes on Dampé and still know
when to stop.

<!-- ![An angle where both of his eyes stay clear](assets/docs/angle-clear.png) -->
<!-- Placeholder, to record: the same screenshot as in the README. -->


Stand where both of his eyes are clear of his jaw and of Link. Angles that hide an eye for part of his idle loop make you miss
flutters, and every miss costs time.

## Start pressing early

<!-- ![Navi coming out on a repeat talk and going away](assets/docs/navi-repeat-talk.webp) -->
<!-- Placeholder, to record: Animated WebP of a repeat talk: the Yes/No box opens, Navi comes out about 3 seconds later and goes back to Link. Mark the moment presses can start. Make it with python tools/clip.py <recording> assets/docs/navi-repeat-talk.webp --start <m:ss> --end <m:ss>. -->

Flutters count from the moment Navi has gone away, not from the Yes/No box. On a first talk she's gone before his question, so you
can press through his text. On a repeat talk his box opens right away and she comes about 3 to 4 seconds later, so wait for her
sparkles to end. Her sparkles draw RNG, and presses from before she's gone make the read wrong, not just slow.

## Get 3 and 4 right

A flutter is 3 or 4 quick blinks, and the count is half of what the calculator learns from it. In the five real reads logged so far,
3s were always pressed as 3, but 20 of 46 4s were pressed as 3. That's the single biggest thing to fix:

| 4s pressed as 3 | Average lock |
|---|---|
| None | press 12.7 |
| 1 in 10 | press 13.1 |
| 43 in 100, as logged | press 15.0 |

<!-- ![A flutter of 3 blinks next to one of 4, slowed down](assets/docs/flutter-3-vs-4.webp) -->
<!-- Placeholder, to record: Animated WebP zoomed on Dampe's eye, a 3 then a 4, at half speed, each labelled with its count. Make it with python tools/clip.py <recording> assets/docs/flutter-3-vs-4.webp --crop W:H:X:Y --width 480 --speed 0.5 --start <m:ss> --end <m:ss>, cropped around his eye. -->


A 4 takes 3 game frames longer than a 3, about 150 ms. Feel it as a longer burst rather than counting each blink. If you press the
wrong key, press either key again within 1.3 seconds to flip it. A miscount isn't fatal, since it's scored as unlikely rather than
impossible, but it holds the lock back.

## Press at the same moment every time

Press as the last blink ends and the eye stays open. How late you are doesn't matter, because the calculator fits one lateness to
the whole read. What matters is that it's the same lateness every time. This is the spread, measured as one standard deviation
around your average:

| Spread | Average lock |
|---|---|
| 25 ms | press 11.2 |
| 50 ms | press 11.5 |
| 75 ms | press 12.1 |
| 100 ms | press 13.1 |
| 135 ms | press 15.9 |

The real reads had 72 ms on 3s and 135 ms on 4s, so the 4s were also the less steady ones. Below about 75 ms it barely matters
any more, and above 100 ms each extra 10 ms costs about a press.

The countdown assumes you press 330 ms after a flutter ends, which is what the real reads averaged. If you're much earlier or later
than that, your A presses will be off by the same amount. Use `--lag-ms` with your own number.

If you didn't see a flutter, don't guess. A missed flutter costs the read less than a press that matches nothing.

## Taking amber or waiting for green

Amber cues come before the state is proven, with a percentage. In simulation, amber cues at 70% and up gave the Heart Piece 42 of
46 times with good presses, and 49 of 63 with sloppy ones. Green cues were right every time with steady presses. With very sloppy ones, 160 ms
and 1 in 5 miscounted, 2 of 214 failed. If a green press gives a rupee, the read was probably wrong, so start over.

If you're aiming for time, take amber when it's high and close, and wait for green when the amber cue is far away anyway.

## After the lock

You can stop pressing once it locks. Pressing on can still help. The states left can differ in when Link next blinks, and once one
of Dampé's blinks falls between theirs, your presses rule some out. Fewer states can mean an earlier window.

## The Ocarina

Once locked, the calculator also checks whether freezing Dampé with the Ocarina gets an earlier Heart Piece. It does this when the
plan beats the normal cue by at least 2 seconds, which happened on about 1 read in 5 in simulation. The countdown then becomes three
countdowns:

1. **Say No and pull Ocarina.** Hold the stick away from Dampé as you press No, or Link turns to face him and Navi comes out. Mash
   his reply closed with A or B, then pull the Ocarina. You have 15 game frames, 0.75 seconds. The C button does nothing until his
   reply is closed.
2. **Cancel Ocarina and talk to Dampé.** Stay still with it out, since walking out of his view makes him walk, and that draws RNG.
   At zero, put it away and talk to him facing away. Mash the text: the Yes/No box has to be up within 30 game frames.
3. **Press A on Yes** at zero, like any cue.

<!-- ![Saying No while holding away from Dampe, so Navi stays away](assets/docs/ocarina-hold-away.webp) -->
<!-- Placeholder, to record: Animated WebP of pressing No while holding the stick away, Link staying turned away and Navi staying in, then the same with the stick let go and Navi coming out. Make it with python tools/clip.py <recording> assets/docs/ocarina-hold-away.webp --start <m:ss> --end <m:ss>. -->


If Navi comes out at any point, the plan is lost. Wait for her to go away, start over, and read again.

The plan can start up to 45 seconds ahead, so a long first countdown is normal. **Ocarina** under Settings turns this off.

## Short version

Wait for Navi to go away, then press from the first flutter. Get the 4s right, and press at the same moment of every flutter. Turn sound on
to hear the lock. Take amber when it's high and close. Hold away from Dampé when you say No.
