//! Decides when a state is trustworthy. A wrong flutter can produce a confident wrong state, so a
//! state only counts after it predicted further flutters it hadn't seen, and contradicting the
//! video ends it.

use crate::rng::{BOX_GONE_TO_ROLL, StateWalk, flutter_len, rolls_heart_piece};
use crate::solver::{Candidate, Observation, SEARCH_MISSES, scan_from};

/// Flutters a reading must have seen to be considered at all.
const MIN_HITS: u32 = 7;
/// Flutters from a first one before scanning from it.
const MIN_OBSERVATIONS: usize = 6;
/// Flutters a scan's results must already have seen to be kept and rechecked.
const MIN_HITS_TO_KEEP: u32 = 3;
/// Flutters between scans while nothing fits, so the computer isn't kept busy. A first flutter can
/// be a stray, which is why there's more than one scan.
const FLUTTERS_BETWEEN_SCANS: usize = 3;
/// Observations kept, about 3 minutes' worth with strays.
const MAX_OBSERVATIONS: usize = 100;
/// Flutters a reading must predict correctly before it's trusted.
pub const CONFIRMATIONS: u32 = 3;
/// A reading dies at this many of its flutters in a row that the video didn't show. Strays can't
/// kill one, because the video really does show things that aren't flutters. A wrong reading can't
/// keep hitting, so this ends it within a few seconds.
const MAX_MISSES_IN_A_ROW: u32 = 3;
/// The same for a reading that has already predicted flutters correctly.
const MAX_MISSES_IN_A_ROW_PROVEN: u32 = 5;
/// Each this many hits allows a reading one more miss while it's being narrowed down.
const HITS_PER_ALLOWED_MISS: u32 = 3;

/// Game frames looked ahead for a Heart Piece.
const HORIZON: i64 = 1200;

/// What the tracker knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Collecting flutters: this many so far.
    Collecting(usize),
    /// A search ran and more than one state still fits.
    Narrowing(usize),
    /// A state fits, and has predicted this many flutters since.
    Confirming(u32),
    /// A state is trusted.
    Locked,
}

/// Game frames a dig can start to give the Heart Piece, for every remaining reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First and last game frames the dig's first dust can land on, inclusive.
    pub first_roll: i64,
    pub last_roll: i64,
}

impl Window {
    /// The game frames the text box should disappear on, inclusive.
    pub fn box_gone(&self) -> (i64, i64) {
        (self.first_roll - BOX_GONE_TO_ROLL, self.last_roll - BOX_GONE_TO_ROLL)
    }
}

/// A reading being followed.
#[derive(Clone, Copy)]
struct Followed {
    candidate: Candidate,
    /// Its hits when it reached [`MIN_HITS`]. Only hits after that are predictions.
    hits_when_found: Option<u32>,
    confirmed: u32,
}

/// Follows flutters to a trusted state.
///
/// A scan of every state is slow, and almost nothing survives one. So a scan only happens now and
/// then, from a new first flutter, and what it leaves is rechecked as flutters come in.
#[derive(Default)]
pub struct Tracker {
    observations: Vec<Observation>,
    followed: Vec<Followed>,
    /// The readings with enough hits to count.
    candidates: Vec<Candidate>,
    /// The flutters worth scanning from.
    clean_starts: Vec<i64>,
    /// Where the last scan started, and how many flutters came in since.
    last_anchor: Option<i64>,
    added_since_scan: usize,
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The fewest predictions any counted reading has made.
    fn confirmed(&self) -> u32 {
        self.followed.iter().filter(|f| f.hits_when_found.is_some()).map(|f| f.confirmed).min().unwrap_or(0)
    }

    pub fn status(&self) -> Status {
        match self.candidates.len() {
            0 => Status::Collecting(self.observations.len()),
            // Proven readings can still differ in Link's next blink, until he blinks. That's no reason
            // for doubt: a window only has the frames every one of them agrees on
            _ if self.confirmed() >= CONFIRMATIONS => Status::Locked,
            _ if !self.same_trajectory() => Status::Narrowing(self.candidates.len()),
            _ => Status::Confirming(self.confirmed()),
        }
    }

    pub fn is_locked(&self) -> bool {
        self.status() == Status::Locked
    }

    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Forgets everything.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Adds a clean flutter from the video, reported on game frame `now`.
    pub fn observe(&mut self, observation: Observation, now: i64) {
        self.add(observation, true);
        self.update(now);
    }

    /// Adds a flutter without judging it yet. Scans are slow, so flutters that came in during one
    /// are added together and judged once.
    pub fn add(&mut self, observation: Observation, clean: bool) {
        if clean {
            self.clean_starts.push(observation.start);
        }
        let at = self.observations.partition_point(|o| o.start <= observation.start);
        self.observations.insert(at, observation);
        self.added_since_scan += 1;
    }

    /// Judges the flutters added so far, as of game frame `now`.
    pub fn update(&mut self, now: i64) {
        let excess = self.observations.len().saturating_sub(MAX_OBSERVATIONS);
        self.observations.drain(..excess);
        self.recheck(now);
        if self.candidates.is_empty() && self.scan(now) {
            self.recheck(now);
        }
    }

    fn recheck(&mut self, now: i64) {
        let observations = &self.observations;
        self.followed.retain_mut(|followed| {
            let result = followed.candidate.compare(observations, now);
            if followed.hits_when_found.is_none() && result.hits >= MIN_HITS {
                followed.hits_when_found = Some(result.hits);
            }
            match followed.hits_when_found {
                Some(hits) => {
                    followed.confirmed = result.hits.saturating_sub(hits);
                    // From some angles the jaw hides the eyes for part of the loop
                    let allowed = if followed.confirmed >= CONFIRMATIONS {
                        MAX_MISSES_IN_A_ROW_PROVEN
                    } else {
                        MAX_MISSES_IN_A_ROW
                    };
                    result.misses_since_hit < allowed
                }
                // On some angles about 1 flutter in 6 isn't read. A wrong reading can't keep hitting,
                // so misses are allowed in proportion to hits
                None => result.misses <= SEARCH_MISSES + result.hits / HITS_PER_ALLOWED_MISS,
            }
        });
        // A reading that has proven itself isn't put in doubt by a new one that hasn't
        if self.followed.iter().any(|f| f.confirmed >= CONFIRMATIONS) {
            self.followed.retain(|f| f.hits_when_found.is_none() || f.confirmed >= CONFIRMATIONS);
        }
        self.candidates = self.followed.iter().filter(|f| f.hits_when_found.is_some()).map(|f| f.candidate).collect();
    }

    /// Scans from the oldest flutter not scanned from yet, if it's time. Whether it did.
    fn scan(&mut self, now: i64) -> bool {
        // The scan after the first waits, so the computer isn't kept busy
        let first_scan = self.last_anchor.is_none();
        if !first_scan && self.added_since_scan < FLUTTERS_BETWEEN_SCANS {
            return false;
        }
        let last_anchor = self.last_anchor;
        let Some(&anchor_start) = self.clean_starts.iter().find(|&&start| last_anchor.is_none_or(|last| start > last))
        else {
            return false;
        };
        let anchor = self.observations.partition_point(|o| o.start < anchor_start);
        let from_anchor = &self.observations[anchor..];
        if from_anchor.len() < MIN_OBSERVATIONS {
            return false;
        }
        let found = scan_from(from_anchor, now, MIN_HITS_TO_KEEP);
        self.last_anchor = Some(from_anchor[0].start);
        self.added_since_scan = 0;
        self.followed.extend(found.into_iter().map(|candidate| Followed {
            candidate,
            hits_when_found: None,
            confirmed: 0,
        }));
        true
    }

    /// Whether every candidate agrees on the state after the flutters so far.
    fn same_trajectory(&self) -> bool {
        let Some(last) = self.observations.last() else { return false };
        // The last flutter's renewal has to be behind it, in case it's a stray
        let frame = last.start - flutter_len(last.count);
        let mut states = self.candidates.iter().map(|candidate| candidate.state_at_flutter(frame));
        let Some(first) = states.next() else { return false };
        states.all(|state| state == first)
    }

    /// The first time a dig gives the Heart Piece for every reading, from game frame `from` on,
    /// where the window is at least `min_frames` long.
    pub fn next_window(&self, from: i64, min_frames: i64) -> Option<Window> {
        if self.candidates.is_empty() {
            return None;
        }
        let mut walks: Vec<_> =
            self.candidates.iter().map(|candidate| (StateWalk::new(candidate.blinks()), candidate.offset)).collect();
        let mut run_start = None;
        for frame in from..from + HORIZON {
            let good = walks.iter_mut().all(|(walk, offset)| rolls_heart_piece(walk.state_for_roll(frame - *offset)));
            if good {
                run_start.get_or_insert(frame);
            } else if let Some(start) = run_start.take()
                && frame - start >= min_frames
            {
                return Some(Window { first_roll: start, last_roll: frame - 1 });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Blinks;

    fn flutters(seed: u32, link: i64, n: usize) -> Vec<Observation> {
        let mut blinks = Blinks::new(seed, link);
        (0..n)
            .map(|_| {
                let flutter = blinks.next_flutter();
                Observation { start: flutter.start + 500, count: flutter.count }
            })
            .collect()
    }

    fn next_different(seed: u32) -> u32 {
        (seed + 12_345) % crate::rng::STATES
    }

    /// Feeds flutters as the detector would, a few frames after each one ended.
    fn feed(tracker: &mut Tracker, observations: &[Observation]) {
        for &observation in observations {
            tracker.observe(observation, observation.start + flutter_len(observation.count) + 8);
        }
    }

    fn locked_after(observations: &[Observation]) -> Option<usize> {
        let mut tracker = Tracker::new();
        for (i, observation) in observations.iter().enumerate() {
            feed(&mut tracker, std::slice::from_ref(observation));
            if tracker.is_locked() {
                return Some(i + 1);
            }
        }
        None
    }

    #[test]
    fn locks_only_after_predicting_further_flutters() {
        let all = flutters(3_141_592, 40, 16);
        let locked = locked_after(&all).expect("never locked");
        assert!(locked >= MIN_HITS as usize + CONFIRMATIONS as usize, "locked after {locked}");
    }

    /// Live feeds show a stray for about every 2 flutters. That must only cost time.
    #[test]
    fn strays_do_not_stop_the_lock() {
        let mut all = Vec::new();
        for (i, flutter) in flutters(3_141_592, 40, 20).into_iter().enumerate() {
            all.push(flutter);
            if i % 2 == 0 {
                all.push(Observation { start: flutter.start + 19, count: 3 });
            }
        }
        assert!(locked_after(&all).is_some());
    }

    /// A stray as the first flutter of a scan wastes the scan, so unclean ones don't start one.
    #[test]
    fn scans_start_from_clean_flutters() {
        let all = flutters(3_141_592, 40, 12);
        let mut tracker = Tracker::new();
        tracker.add(Observation { start: all[0].start - 20, count: 3 }, false);
        for &observation in &all[..7] {
            tracker.add(observation, true);
        }
        let last = all[6];
        tracker.update(last.start + flutter_len(last.count) + 8);
        assert_eq!(tracker.last_anchor, Some(all[0].start));
        assert!(!tracker.candidates().is_empty());
    }

    /// Flutters the reading wanted going unseen, while others show up, have to end the lock.
    #[test]
    fn a_contradiction_ends_the_lock() {
        let all = flutters(3_141_592, 40, 18);
        let mut tracker = Tracker::new();
        feed(&mut tracker, &all[..17]);
        assert!(tracker.is_locked());
        let mut wrong = all[17];
        wrong.start += 2;
        feed(&mut tracker, &[wrong]);
        assert!(tracker.is_locked(), "one missed flutter isn't enough");
        let later = Observation { start: wrong.start + 600, count: 3 };
        feed(&mut tracker, &[later]);
        assert!(tracker.candidates().is_empty());
    }

    /// A flutter the video simply didn't show isn't a contradiction, and the lock survives it.
    #[test]
    fn one_missed_flutter_keeps_the_lock() {
        let all = flutters(3_141_592, 40, 19);
        let mut tracker = Tracker::new();
        feed(&mut tracker, &all[..17]);
        assert!(tracker.is_locked());
        feed(&mut tracker, &all[18..]);
        assert!(tracker.is_locked());
    }

    /// A new reading that hasn't predicted anything must not cancel a lock. This cancelled real cues.
    #[test]
    fn an_unproven_reading_does_not_end_a_lock() {
        let mut tracker = Tracker::new();
        let all = flutters(3_141_592, 40, 18);
        feed(&mut tracker, &all[..17]);
        assert!(tracker.is_locked());
        let candidate = tracker.candidates()[0];
        tracker.followed.push(Followed { candidate, hits_when_found: Some(1000), confirmed: 0 });
        feed(&mut tracker, &all[17..]);
        assert!(tracker.is_locked());
        assert!(tracker.followed.iter().all(|f| f.hits_when_found != Some(1000)));
    }

    /// Proven readings that differ in Link's next blink must not cancel a lock. This cancelled real
    /// cues: on real footage every such lock was right.
    #[test]
    fn proven_readings_that_differ_keep_the_lock() {
        let mut tracker = Tracker::new();
        feed(&mut tracker, &flutters(3_141_592, 40, 17));
        assert!(tracker.is_locked());
        let mut other = tracker.candidates()[0];
        other.seed = next_different(other.seed);
        tracker.followed.push(Followed { candidate: other, hits_when_found: Some(7), confirmed: CONFIRMATIONS });
        tracker.candidates.push(other);
        assert!(!tracker.same_trajectory(), "the control: these readings really differ");
        assert!(tracker.is_locked());
    }

    /// The window has to be judged by the model, not by anything the tracker computes itself.
    #[test]
    fn window_frames_roll_the_heart_piece() {
        let mut tracker = Tracker::new();
        feed(&mut tracker, &flutters(3_141_592, 40, 16));
        let window = tracker.next_window(2000, 13).expect("no window in the horizon");
        assert!(window.last_roll - window.first_roll + 1 >= 13);
        let candidate = tracker.candidates()[0];
        let mut walk = StateWalk::new(candidate.blinks());
        for frame in window.first_roll..=window.last_roll {
            assert!(rolls_heart_piece(walk.state_for_roll(frame - candidate.offset)));
        }
    }
}
