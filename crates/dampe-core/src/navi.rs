//! Tells when Navi has finished. Her sparkles draw RNG in bursts, which breaks the blink model, so
//! flutters from before that can't count. On a first talk she's done before the Yes/No box opens. On
//! a repeat talk the box opens right away, and she's still to come.
//!
//! The picture shows when she's done: the head turning towards Link and Navi flying in make it
//! differ from the same point of the idle loop, and it stops doing so about 8 game frames before her
//! last draw. That's been the same on all recordings so far, so a margin covers it.

use std::collections::VecDeque;

use crate::detector::Flutter;
use crate::picture::{HEIGHT, WIDTH};
use crate::rng::IDLE_LOOP_FRAMES;

/// Only the part of the picture above the text box, since the text changes on its own.
const ABOVE_BOX: usize = WIDTH * HEIGHT * 5 / 8;
/// The average difference to the same point of the loop that counts as settled. Before she's done
/// it's 6 to 10, after about 1, a bit more with a drifting sky.
const SETTLED_DIFFERENCE: u64 = 2;
/// Game frames in a row it has to stay that way.
const SETTLED_FRAMES: i64 = 15;
/// Game frames after the picture settled that flutters start counting: past her last draw.
const MARGIN_FRAMES: i64 = 25;
/// Game frames of the box after which it counts as settled anyway: something else can keep the
/// picture busy, and the blink model finds out if it was wrong.
const GIVE_UP_FRAMES: i64 = 300;

/// Follows the picture to see when Navi has finished.
pub struct NaviWatch {
    recent: VecDeque<Vec<u8>>,
    /// Game frames in a row the picture has repeated the loop.
    settled_for: i64,
    /// Whether a box is open that's waiting for her to finish.
    waiting: bool,
    /// Flutters starting before this game frame don't count.
    from: i64,
    /// Flutters seen while waiting, which may turn out to count.
    held: Vec<Flutter>,
}

impl Default for NaviWatch {
    fn default() -> Self {
        Self {
            recent: VecDeque::with_capacity(IDLE_LOOP_FRAMES + 1),
            settled_for: 0,
            waiting: false,
            from: 0,
            held: Vec::new(),
        }
    }
}

impl NaviWatch {
    /// The next game frame's picture, whether a box is open or not.
    pub fn frame(&mut self, picture: &[u8]) {
        let top = &picture[..ABOVE_BOX];
        if self.recent.len() == IDLE_LOOP_FRAMES {
            let old = self.recent.pop_front().expect("checked the length");
            let total: u64 = old.iter().zip(top).map(|(&a, &b)| u64::from(a.abs_diff(b))).sum();
            if total <= SETTLED_DIFFERENCE * ABOVE_BOX as u64 {
                self.settled_for += 1;
            } else {
                self.settled_for = 0;
            }
        }
        self.recent.push_back(top.to_vec());
    }

    /// A box opened. Returns whether Navi is already done.
    pub fn box_opened(&mut self) -> bool {
        // It takes a whole loop to know a frame repeated it, so a box opening just after she's done
        // has to wait for that, and looks back when it knows
        self.waiting = self.settled_for < SETTLED_FRAMES;
        self.from = 0;
        self.held.clear();
        !self.waiting
    }

    /// A flutter from the video, if it counts yet.
    pub fn admit(&mut self, flutter: Flutter) -> Option<Flutter> {
        if self.waiting {
            self.held.push(flutter);
            return None;
        }
        (flutter.start >= self.from).then_some(flutter)
    }

    /// A new read of the flutters up to game frame `through`: those that count, unless it's still
    /// waiting and keeps them for later.
    pub fn admit_reread(&mut self, through: i64, flutters: &[Flutter]) -> Option<Vec<Flutter>> {
        if self.waiting {
            self.held.retain(|flutter| flutter.start > through);
            self.held.extend(flutters.iter().filter(|flutter| flutter.start <= through));
            self.held.sort_by_key(|flutter| flutter.start);
            return None;
        }
        Some(flutters.iter().copied().filter(|flutter| flutter.start >= self.from).collect())
    }

    /// As of game frame `now` of the box: when it just stopped waiting, the game frame flutters count
    /// from, and the flutters that were held that count.
    pub fn advance(&mut self, now: i64) -> Option<(i64, Vec<Flutter>)> {
        let from = self.counts_from(now)?;
        self.from = from;
        let held = std::mem::take(&mut self.held);
        Some((from, held.into_iter().filter(|flutter| flutter.start >= from).collect()))
    }

    /// Someone said she's done, so only flutters from game frame `from` on count.
    pub fn mark_gone(&mut self, from: i64) {
        self.waiting = false;
        self.from = from;
        self.held.clear();
    }

    /// The read ended: nothing is waiting any more.
    pub fn end_read(&mut self) {
        self.mark_gone(0);
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    /// While waiting, as of game frame `now` of the box: the game frame flutters start counting from,
    /// once that's known.
    fn counts_from(&mut self, now: i64) -> Option<i64> {
        if !self.waiting {
            return None;
        }
        if self.settled_for >= SETTLED_FRAMES {
            self.waiting = false;
            // The first frame of the run is compared to the frame a whole loop before it
            let settled = now - self.settled_for + 1 - IDLE_LOOP_FRAMES as i64;
            return Some((settled + MARGIN_FRAMES).max(0));
        }
        if now >= GIVE_UP_FRAMES {
            self.waiting = false;
            return Some(now);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture that repeats on the idle loop.
    fn looping(frame: usize) -> Vec<u8> {
        let mut picture = vec![50u8; WIDTH * HEIGHT];
        let x = frame % IDLE_LOOP_FRAMES * 5;
        for row in 10..60 {
            picture[row * WIDTH + x..row * WIDTH + x + 100].fill(200);
        }
        picture
    }

    /// The same, with most of it changing, like Navi and the head turning.
    fn busy(frame: usize) -> Vec<u8> {
        let mut picture = looping(frame);
        picture[..ABOVE_BOX].iter_mut().enumerate().for_each(|(i, pixel)| {
            if (i + frame * 7) % 3 == 0 {
                *pixel = 180;
            }
        });
        picture
    }

    fn settled_watch() -> NaviWatch {
        let mut watch = NaviWatch::default();
        for frame in 0..200 {
            watch.frame(&looping(frame));
        }
        watch
    }

    #[test]
    fn a_box_opening_after_she_is_done_counts_from_its_start() {
        let mut watch = settled_watch();
        assert!(watch.box_opened());
        assert_eq!(watch.counts_from(0), None, "nothing to wait for");
    }

    /// Repeat talks: she's still busy when the box opens.
    #[test]
    fn a_box_opening_during_her_waits_and_starts_past_the_last_busy_frame() {
        let mut watch = NaviWatch::default();
        for frame in 0..100 {
            watch.frame(&busy(frame));
        }
        assert!(!watch.box_opened());
        // She goes on for 60 frames of the box
        let mut known = None;
        for now in 0..200 {
            let frame = 100 + now as usize;
            watch.frame(&if now < 60 { busy(frame) } else { looping(frame) });
            if let Some(from) = watch.counts_from(now) {
                known = Some((now, from));
                break;
            }
        }
        let (now, from) = known.expect("never settled");
        // A frame only counts once a whole clean loop is behind it
        assert_eq!(now, 60 + IDLE_LOOP_FRAMES as i64 + SETTLED_FRAMES - 1);
        assert_eq!(from, 60 + MARGIN_FRAMES);
    }

    /// What came during her can't count, what came after she was done can't be lost.
    #[test]
    fn held_flutters_only_count_from_when_she_was_done() {
        let flutter = |start| Flutter { start, count: 3, fit: 1.0 };
        let mut watch = NaviWatch::default();
        for frame in 0..100 {
            watch.frame(&busy(frame));
        }
        watch.box_opened();
        assert_eq!(watch.admit(flutter(30)), None);
        assert_eq!(watch.admit(flutter(100)), None);
        let mut released = None;
        for now in 0..200 {
            let frame = 100 + now as usize;
            watch.frame(&if now < 60 { busy(frame) } else { looping(frame) });
            released = released.or(watch.advance(now));
        }
        let (from, held) = released.expect("never settled");
        assert_eq!(from, 60 + MARGIN_FRAMES);
        assert_eq!(held, [flutter(100)]);
        assert_eq!(watch.admit(flutter(from - 1)), None, "the control: not waiting, and it's still too early");
        assert_eq!(watch.admit(flutter(from)), Some(flutter(from)));
    }

    #[test]
    fn it_gives_up_on_a_picture_that_never_settles() {
        let mut watch = NaviWatch::default();
        for frame in 0..100 {
            watch.frame(&busy(frame));
        }
        watch.box_opened();
        for now in 0..GIVE_UP_FRAMES {
            watch.frame(&busy(100 + now as usize));
            assert_eq!(watch.counts_from(now), None);
        }
        assert_eq!(watch.counts_from(GIVE_UP_FRAMES), Some(GIVE_UP_FRAMES));
    }
}
