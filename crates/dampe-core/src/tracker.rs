//! Decides when a state is trustworthy. A wrong flutter can produce a confident wrong state, so a
//! state only counts after it predicted further flutters it hadn't seen, and contradicting the
//! video ends it.

use crate::rng::{BOX_GONE_TO_ROLL, StateWalk, flutter_len, rolls_heart_piece};
use crate::solver::{Candidate, Match, Observation, SEARCH_MISSES, scan_from};

/// Flutters a reading must have seen to be considered at all. With [`CONFIRMATIONS`], that makes 8.
/// Against RNG watch values, 7 gave a wrong state with wrong Heart Piece frames, and 8 gave none in
/// 23 locks.
const MIN_HITS: u32 = 6;
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
pub const CONFIRMATIONS: u32 = 2;
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

/// How likely a read's parts are, for weighing readings against each other: a real flutter being
/// read at all, not being read, and a stray landing exactly where a reading has nothing. Learning the
/// first two per read made no measurable difference on 10 recordings.
const P_READ: f64 = 0.85;
const P_MISSED: f64 = 0.15;
const P_STRAY: f64 = 0.003;

fn log_weight(m: &Match) -> f64 {
    f64::from(m.hits) * P_READ.ln() + f64::from(m.misses) * P_MISSED.ln() + f64::from(m.extras) * P_STRAY.ln()
}

/// The readings with less than this many hits are no better than what wasn't found at all: the real
/// reading can be missing from the set, when a scan started from a stray.
const UNEXPLAINED_HITS: f64 = 4.0;
/// The most likely readings looked at for windows. The others only count against them.
const LIKELY_READINGS: usize = 300;
/// How far ahead an early window can be, in game frames.
const LIKELY_HORIZON: i64 = 600;

/// The chance every frame of an early window needs. On 10 RNG watch recordings, aiming at the middle
/// of windows at 90% and up was right 51 of 52 times, 70-89% 23 of 26, and 60-69% only 5 of 9.
pub const LIKELY_MIN_CONFIDENCE: f64 = 0.7;

/// A window that probably gives the Heart Piece, before a state is proven.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LikelyWindow {
    pub window: Window,
    /// The lowest chance of any of its frames, 0 to 1.
    pub confidence: f64,
    /// The chance of the Heart Piece when aiming at its middle, 0 to 1.
    pub chance: f64,
    /// The most hits any reading has.
    pub best_hits: u32,
}

/// Game frames a dig can start to give the Heart Piece, for every remaining reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First and last game frames the dig's first dust can land on, inclusive.
    pub first_roll: i64,
    pub last_roll: i64,
}

impl Window {
    /// The frame to aim for: its middle.
    pub fn target(&self) -> i64 {
        (self.first_roll + self.last_roll) / 2
    }

    /// The game frames the text box should disappear on, inclusive.
    pub fn box_gone(&self) -> (i64, i64) {
        (self.first_roll - BOX_GONE_TO_ROLL, self.last_roll - BOX_GONE_TO_ROLL)
    }
}

/// A reading and how it's doing, for showing how the read works.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub candidate: Candidate,
    /// The chance this is the real one, 0 to 1.
    pub share: f64,
    pub hits: u32,
    pub misses: u32,
    pub extras: u32,
    /// Flutters it predicted before they were seen.
    pub confirmed: u32,
    /// When a dig next gives the Heart Piece for this reading alone, if that's soon.
    pub next_window: Option<Window>,
}

/// The most likely readings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readings {
    pub top: Vec<Reading>,
    /// How many there are in all.
    pub total: usize,
    /// The chance the real one isn't among them at all, 0 to 1.
    pub not_found: f64,
}

/// A reading being followed.
#[derive(Clone, Copy)]
struct Followed {
    candidate: Candidate,
    /// Its hits when it reached [`MIN_HITS`]. Only hits after that are predictions.
    hits_when_found: Option<u32>,
    confirmed: u32,
    /// How it compared to the observations last time.
    latest: Match,
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

    /// Replaces what was read up to game frame `through` with a better read. New flutters this finds
    /// aren't predictions, so readings' counts of those stay as they were.
    pub fn replace_through(&mut self, through: i64, flutters: &[(Observation, bool)], now: i64) {
        let before: Vec<u32> =
            self.followed.iter().map(|f| f.candidate.compare(&self.observations, now).hits).collect();
        self.observations.retain(|o| o.start > through);
        self.clean_starts.retain(|&start| start > through);
        for &(observation, clean) in flutters.iter().filter(|(o, _)| o.start <= through) {
            self.add(observation, clean);
        }
        self.clean_starts.sort_unstable();
        let observations = &self.observations;
        for (followed, before) in self.followed.iter_mut().zip(before) {
            if let Some(hits) = &mut followed.hits_when_found {
                let after = followed.candidate.compare(observations, now).hits;
                *hits = (*hits + after).saturating_sub(before);
            }
        }
        // The first flutters may be different ones now, and worth scanning from
        self.last_anchor = None;
        self.update(now);
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
            followed.latest = result;
            // Flutters from before a reading's first one say nothing about it
            let first = followed.candidate.blinks().next_flutter().start + followed.candidate.offset;
            followed.latest.extras -=
                (observations.partition_point(|o| o.start < first) as u32).min(followed.latest.extras);
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
        // The newest flutter with enough after it. Older ones can't do better, since they have more
        // of what went wrong behind them, and a scan a long way back might never catch up
        let last_anchor = self.last_anchor;
        let observations = &self.observations;
        let Some(&anchor_start) = self.clean_starts.iter().rev().find(|&&start| {
            let later = observations.len() - observations.partition_point(|o| o.start < start);
            later >= MIN_OBSERVATIONS && last_anchor.is_none_or(|last| start > last)
        }) else {
            return false;
        };
        let anchor = self.observations.partition_point(|o| o.start < anchor_start);
        let from_anchor = &self.observations[anchor..];
        let found = scan_from(from_anchor, now, MIN_HITS_TO_KEEP);
        self.last_anchor = Some(from_anchor[0].start);
        self.added_since_scan = 0;
        self.followed.extend(found.into_iter().map(|candidate| Followed {
            candidate,
            hits_when_found: None,
            confirmed: 0,
            latest: Match::default(),
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

    /// Every reading with its chance of being the real one, likeliest first, going by how well it
    /// explains what was seen, and the chance the real one isn't among them.
    fn shares(&self) -> Option<(Vec<(f64, &Followed)>, f64)> {
        let mut weighted: Vec<(f64, &Followed)> =
            self.followed.iter().map(|followed| (log_weight(&followed.latest), followed)).collect();
        weighted.sort_by(|a, b| b.0.total_cmp(&a.0));
        let &(best, best_followed) = weighted.first()?;
        // The chance the real reading isn't here at all, as a reading that explains only a few
        let explained = f64::from(best_followed.latest.hits + best_followed.latest.extras);
        let unexplained =
            UNEXPLAINED_HITS * P_READ.ln() + (explained - UNEXPLAINED_HITS).max(0.0) * P_STRAY.ln() - best;
        let total: f64 = weighted.iter().map(|(weight, _)| (weight - best).exp()).sum::<f64>() + unexplained.exp();
        for (weight, _) in &mut weighted {
            *weight = (*weight - best).exp() / total;
        }
        Some((weighted, unexplained.exp() / total))
    }

    /// The `limit` most likely readings as of game frame `now`, where their Heart Piece windows have
    /// to be at least `min_frames` long.
    pub fn readings(&self, limit: usize, now: i64, min_frames: i64) -> Readings {
        let Some((shares, not_found)) = self.shares() else { return Readings::default() };
        let top = shares
            .iter()
            .take(limit)
            .map(|&(share, followed)| Reading {
                candidate: followed.candidate,
                share,
                hits: followed.latest.hits,
                misses: followed.latest.misses,
                extras: followed.latest.extras,
                confirmed: followed.confirmed,
                next_window: first_window(&[followed.candidate], now + BOX_GONE_TO_ROLL, min_frames, HORIZON),
            })
            .collect();
        Readings { top, total: shares.len(), not_found }
    }

    /// The chance of each frame in `frames` giving the Heart Piece, going by how well each reading
    /// explains what was seen, and the most hits any reading has.
    fn chances(&self, frames: std::ops::Range<i64>) -> Option<(Vec<f64>, u32)> {
        let (shares, _) = self.shares()?;
        let best_hits = shares.first()?.1.latest.hits;
        let mut walks: Vec<_> = shares
            .iter()
            .take(LIKELY_READINGS)
            .map(|&(share, followed)| (share, StateWalk::new(followed.candidate.blinks()), followed.candidate.offset))
            .collect();
        let chances = frames
            .map(|frame| {
                walks
                    .iter_mut()
                    .map(
                        |(weight, walk, offset)| {
                            if rolls_heart_piece(walk.state_for_roll(frame - *offset)) { *weight } else { 0.0 }
                        },
                    )
                    .sum()
            })
            .collect();
        Some((chances, best_hits))
    }

    /// The first window from game frame `from` on, at least `min_frames` long, where every frame has
    /// at least `min_confidence` of giving the Heart Piece.
    pub fn likely_window(&self, from: i64, min_frames: i64, min_confidence: f64) -> Option<LikelyWindow> {
        let (chances, best_hits) = self.chances(from..from + LIKELY_HORIZON)?;
        let mut run: Option<(i64, f64)> = None;
        for (frame, &chance) in (from..).zip(&chances) {
            if chance >= min_confidence {
                let (start, lowest) = run.unwrap_or((frame, chance));
                run = Some((start, lowest.min(chance)));
            } else if let Some((start, lowest)) = run.take()
                && frame - start >= min_frames
            {
                let window = Window { first_roll: start, last_roll: frame - 1 };
                let chance = chances[(window.target() - from) as usize];
                return Some(LikelyWindow { window, confidence: lowest, chance, best_hits });
            }
        }
        None
    }

    /// The chance of a dig on game frame `frame` giving the Heart Piece.
    pub fn chance_at(&self, frame: i64) -> f64 {
        self.chances(frame..frame + 1).map_or(0.0, |(chances, _)| chances[0])
    }

    /// The lowest chance of any frame of `window` giving the Heart Piece.
    pub fn window_confidence(&self, window: Window) -> f64 {
        self.chances(window.first_roll..window.last_roll + 1)
            .and_then(|(chances, _)| chances.into_iter().min_by(f64::total_cmp))
            .unwrap_or(0.0)
    }

    /// The first time a dig gives the Heart Piece for every reading, from game frame `from` on,
    /// where the window is at least `min_frames` long.
    pub fn next_window(&self, from: i64, min_frames: i64) -> Option<Window> {
        first_window(&self.candidates, from, min_frames, HORIZON)
    }
}

/// The first time a dig gives the Heart Piece for all of `candidates`, from game frame `from` on, where
/// the window is at least `min_frames` long, looking `horizon` ahead.
pub(crate) fn first_window(candidates: &[Candidate], from: i64, min_frames: i64, horizon: i64) -> Option<Window> {
    if candidates.is_empty() {
        return None;
    }
    let mut walks: Vec<_> =
        candidates.iter().map(|candidate| (StateWalk::new(candidate.blinks()), candidate.offset)).collect();
    let mut run_start = None;
    for frame in from..from + horizon {
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
        tracker.add(all[0], true);
        tracker.add(Observation { start: all[1].start - 20, count: 3 }, false);
        for &observation in &all[1..7] {
            tracker.add(observation, false);
        }
        let last = all[6];
        tracker.update(last.start + flutter_len(last.count) + 8);
        assert_eq!(tracker.last_anchor, Some(all[0].start));
    }

    /// After a stretch of misreads, a scan has to start from what's recent, or it never recovers.
    #[test]
    fn scans_start_from_recent_flutters() {
        let all = flutters(3_141_592, 40, 30);
        let mut tracker = Tracker::new();
        // A long stretch of nothing useful
        for i in 0..20 {
            tracker.add(Observation { start: 100 + i * 37, count: 3 }, true);
        }
        for &observation in &all[..20] {
            tracker.add(observation, true);
        }
        let last = all[19];
        tracker.update(last.start + flutter_len(last.count) + 8);
        assert_eq!(tracker.last_anchor, Some(all[14].start));
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

    /// A new reading that hasn't predicted anything must not cancel a lock. This cancelled real cues.
    #[test]
    fn an_unproven_reading_does_not_end_a_lock() {
        let mut tracker = Tracker::new();
        let all = flutters(3_141_592, 40, 18);
        feed(&mut tracker, &all[..17]);
        assert!(tracker.is_locked());
        let candidate = tracker.candidates()[0];
        tracker.followed.push(Followed {
            candidate,
            hits_when_found: Some(1000),
            confirmed: 0,
            latest: Match::default(),
        });
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
        tracker.followed.push(Followed {
            candidate: other,
            hits_when_found: Some(7),
            confirmed: CONFIRMATIONS,
            latest: Match::default(),
        });
        tracker.candidates.push(other);
        assert!(!tracker.same_trajectory(), "the control: these readings really differ");
        assert!(tracker.is_locked());
    }

    /// A state just found has 6 hits against no competition, so it's likely but not yet proven.
    #[test]
    fn a_likely_window_comes_before_the_lock_and_rolls_the_heart_piece() {
        let all = flutters(3_141_592, 40, 16);
        let mut tracker = Tracker::new();
        feed(&mut tracker, &all[..7]);
        assert!(!tracker.is_locked());
        let last = all[6];
        let from = last.start + 100;
        // One may not come up right away: the Heart Piece is a 1 in 10 roll
        let likely = (0..10)
            .find_map(|i| tracker.likely_window(from + i * LIKELY_HORIZON, 13, LIKELY_MIN_CONFIDENCE))
            .expect("no likely window");
        let truth = tracker.followed.iter().find(|f| f.candidate.seed == 3_141_592).expect("the true reading");
        let mut walk = StateWalk::new(truth.candidate.blinks());
        for frame in likely.window.first_roll..=likely.window.last_roll {
            assert!(rolls_heart_piece(walk.state_for_roll(frame - truth.candidate.offset)));
        }
        assert!(tracker.window_confidence(likely.window) >= LIKELY_MIN_CONFIDENCE);
    }

    /// With almost nothing seen, nothing should look likely.
    #[test]
    fn nothing_is_likely_from_a_few_flutters() {
        let all = flutters(3_141_592, 40, 3);
        let mut tracker = Tracker::new();
        feed(&mut tracker, &all);
        assert_eq!(tracker.likely_window(all[2].start + 100, 13, LIKELY_MIN_CONFIDENCE), None);
    }

    /// What's shown has to add up, and the real one has to come out on top.
    #[test]
    fn readings_are_shares_of_one_with_the_real_one_first() {
        assert_eq!(Tracker::new().readings(6, 0, 13), Readings::default());
        let mut tracker = Tracker::new();
        feed(&mut tracker, &flutters(3_141_592, 40, 16));
        let readings = tracker.readings(6, 2000, 13);
        assert!(readings.top.len() <= 6 && readings.total >= readings.top.len());
        assert_eq!(readings.top[0].candidate.seed, 3_141_592);
        assert!(readings.top.windows(2).all(|pair| pair[0].share >= pair[1].share));
        let (shares, not_found) = tracker.shares().expect("it has readings");
        let total = shares.iter().map(|(share, _)| share).sum::<f64>() + not_found;
        assert!((total - 1.0).abs() < 1e-9, "{total}");
        assert!(readings.top[0].hits >= MIN_HITS);
        // Its own window, which the one that's cued has to fit inside, since that's for every reading
        let own = readings.top[0].next_window.expect("no window in the horizon");
        let mut walk = StateWalk::new(readings.top[0].candidate.blinks());
        for frame in own.first_roll..=own.last_roll {
            assert!(rolls_heart_piece(walk.state_for_roll(frame - readings.top[0].candidate.offset)));
        }
        let cued = tracker.next_window(2000 + BOX_GONE_TO_ROLL, 13).expect("no window in the horizon");
        assert!(own.first_roll <= cued.first_roll && cued.last_roll <= own.last_roll, "{own:?} {cued:?}");
    }

    /// The chance is the share of the readings' weight that gets the Heart Piece on that frame.
    #[test]
    fn the_chance_is_the_share_of_readings_that_roll_it() {
        let all = flutters(3_141_592, 40, 16);
        let mut tracker = Tracker::new();
        feed(&mut tracker, &all[..16]);
        let truth = tracker.followed[0];
        // Far ahead, where a reading and one with another seed differ
        let other = Candidate { seed: truth.candidate.seed + 1, ..truth.candidate };
        let mut walk_a = StateWalk::new(truth.candidate.blinks());
        let mut walk_b = StateWalk::new(other.blinks());
        let frame = (3000..20000)
            .find(|&frame| {
                rolls_heart_piece(walk_a.state_for_roll(frame - truth.candidate.offset))
                    && !rolls_heart_piece(walk_b.state_for_roll(frame - other.offset))
            })
            .expect("no such frame");
        let strong = Match { hits: 30, ..Match::default() };
        tracker.followed = vec![
            Followed { candidate: truth.candidate, hits_when_found: None, confirmed: 0, latest: strong },
            Followed { candidate: other, hits_when_found: None, confirmed: 0, latest: strong },
        ];
        assert!((tracker.chance_at(frame) - 0.5).abs() < 0.01, "{}", tracker.chance_at(frame));
        tracker.followed.truncate(1);
        assert!(tracker.chance_at(frame) > 0.99, "the control: {}", tracker.chance_at(frame));
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
