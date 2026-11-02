//! What a read has to say, whether its flutters were seen or pressed: the readings it has, and when a
//! dig gives the Heart Piece for them.

use crate::rng::{BOX_GONE_TO_ROLL, Blinks, STATES, StateWalk, rolls_heart_piece};

/// Link's first draw comes on one of these frames after the first renewal.
pub(crate) const LINK_PHASES: std::ops::RangeInclusive<i64> = 1..=99;
/// Game frames looked ahead for a Heart Piece.
pub(crate) const HORIZON: i64 = 1200;
/// The most likely readings looked at for windows. The others only count against them.
pub(crate) const LIKELY_READINGS: usize = 300;
/// How far ahead an early window can be, in game frames.
pub(crate) const LIKELY_HORIZON: i64 = 600;

/// Flutters a reading must predict correctly before it's trusted.
pub const CONFIRMATIONS: u32 = 2;

/// The chance every frame of an early window needs. On 10 RNG watch recordings, aiming at the middle
/// of windows at 90% and up was right 51 of 52 times, 70-89% 23 of 26, and 60-69% only 5 of 9.
pub const LIKELY_MIN_CONFIDENCE: f64 = 0.7;

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

/// One reading of the observations. Model frames plus `offset` are game frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub seed: u32,
    pub link_first: i64,
    pub offset: i64,
}

impl Candidate {
    /// The model, at the renewal before its first flutter.
    pub fn blinks(&self) -> Blinks {
        Blinks::new(self.seed, self.link_first)
    }
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

/// Runs `scan` over every state, split up between half of the cores, since the read has to run beside
/// OBS, and puts together what it finds.
pub(crate) fn scan_all_states<T: Send>(scan: impl Fn(u32, u32) -> Vec<T> + Sync) -> Vec<T> {
    let threads = (std::thread::available_parallelism().map_or(4, |n| n.get()) as u32 / 2).max(1);
    let chunk = STATES.div_ceil(threads);
    let scan = &scan;
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|i| {
                let (from, to) = (i * chunk, ((i + 1) * chunk).min(STATES));
                scope.spawn(move || scan(from, to))
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().expect("a scan thread panicked")).collect()
    })
}

/// The chance of a dig on each of `frames` giving the Heart Piece, from readings and their shares.
pub(crate) fn chances_by_frame(
    readings: impl Iterator<Item = (f64, Candidate)>,
    frames: std::ops::Range<i64>,
) -> Vec<f64> {
    let mut walks: Vec<_> =
        readings.map(|(share, candidate)| (share, StateWalk::new(candidate.blinks()), candidate.offset)).collect();
    frames
        .map(|frame| {
            walks
                .iter_mut()
                .map(
                    |(share, walk, offset)| {
                        if rolls_heart_piece(walk.state_for_roll(frame - *offset)) { *share } else { 0.0 }
                    },
                )
                .sum()
        })
        .collect()
}

/// The first window at least `min_frames` long where `chances`, which start at frame `from`, stay at
/// `min_chance` or above.
pub(crate) fn first_likely_window(chances: &[f64], from: i64, min_frames: i64, min_chance: f64) -> Option<Window> {
    let mut start = None;
    for (frame, &chance) in (from..).zip(chances) {
        if chance >= min_chance {
            start.get_or_insert(frame);
        } else if let Some(first) = start.take()
            && frame - first >= min_frames
        {
            return Some(Window { first_roll: first, last_roll: frame - 1 });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_likely_window_is_the_first_run_long_enough_that_ends() {
        let window = |first_roll, last_roll| Some(Window { first_roll, last_roll });
        let chances = [0.9, 0.1, 0.8, 0.8, 0.8, 0.1, 0.9, 0.9];
        assert_eq!(first_likely_window(&chances, 100, 3, 0.7), window(102, 104), "the first is too short");
        assert_eq!(first_likely_window(&chances, 100, 1, 0.7), window(100, 100));
        assert_eq!(first_likely_window(&chances, 100, 3, 0.85), None, "what runs off the end isn't known to be long");
    }
}
