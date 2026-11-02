//! What a read has to say, whether its flutters were seen or pressed: the readings it has, and when a
//! dig gives the Heart Piece for them.

use crate::rng::{BOX_GONE_TO_ROLL, Blinks, StateWalk, rolls_heart_piece};

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
