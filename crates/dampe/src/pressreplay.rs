//! `dampe press-replay`: runs the manual solver on logged or simulated presses, and scores it against
//! the state that's known to be true. Logged presses use only their times, as they would without
//! video. The video's state is only for scoring.

use std::collections::HashSet;
use std::time::Instant;

use dampe_core::manual::{
    CUE_LEAD_FRAMES, DEFAULT_LAG_MS, Followed, GAME_FPS, LIKELY_LEAD_FRAMES, ManualTracker, Press,
};
use dampe_core::reading::{Candidate, Status, Window};
use dampe_core::rng::{Blinks, STATES, StateWalk, flutter_len, rolls_heart_piece};

use crate::presslog;

/// Game frames a press comes late, for finding the real frame of a cue.
const LAG_FRAMES: f64 = DEFAULT_LAG_MS / 1000.0 * GAME_FPS;
/// How far a reading's flutter can be from the true one to be the same one, in game frames.
const SAME_FLUTTER_FRAMES: f64 = 8.0;
/// A flutter is over this long before `now`, so its press can't still come.
const PAST_FRAMES: f64 = 10.0;
const MIN_WINDOW_FRAMES: i64 = 13;
const LIKELY_MIN_CHANCE: f64 = 0.7;
const SIMULATED_PRESSES: usize = 30;

/// A read to score. Press frames plus `shift` are the truth's frames.
struct Case {
    name: String,
    presses: Vec<Press>,
    truth: Candidate,
    shift: f64,
}

/// What came out of a read.
#[derive(Default)]
struct Outcome {
    /// Presses until the first lock, and whether what it locked was true.
    lock: Option<(usize, bool)>,
    /// Presses from which the likeliest reading stayed true, if it did.
    right_from: Option<usize>,
    /// Likely cues as chance and whether their target gives the Heart Piece.
    likely: Vec<(f64, bool)>,
    /// Locked cues: whether their target gives the Heart Piece.
    locked: Vec<bool>,
    scan_seconds: f64,
}

/// Whether `followed` has the true RNG state after the last true flutter that's over by press frame
/// `now`. Its next flutters may still differ: when Link next blinks can stay unknown for a long time,
/// for the state from the video as well, and windows allow for that by needing every such reading.
fn is_true(followed: &Followed, case: &Case, now: f64) -> bool {
    let end_of = |flutter: dampe_core::rng::ModelFlutter| {
        (flutter.start + flutter_len(flutter.count) + case.truth.offset) as f64 - case.shift + LAG_FRAMES
    };
    let mut truth = case.truth.blinks();
    let first = truth.next_flutter();
    let (mut truth_end, mut truth_state) = (end_of(first), truth.state);
    loop {
        let flutter = truth.next_flutter();
        if end_of(flutter) + PAST_FRAMES >= now {
            break;
        }
        (truth_end, truth_state) = (end_of(flutter), truth.state);
    }
    let mut blinks = Blinks::new(followed.seed, followed.link_first);
    loop {
        let flutter = blinks.next_flutter();
        let end = (flutter.start + flutter_len(flutter.count)) as f64 + followed.score.lateness;
        if end >= truth_end - SAME_FLUTTER_FRAMES {
            return end <= truth_end + SAME_FLUTTER_FRAMES && blinks.state == truth_state;
        }
    }
}

/// Whether aiming at the middle of `window` on a perfect press gives the Heart Piece.
fn target_rolls(window: Window, case: &Case) -> Option<bool> {
    let frame = (window.target() as f64 + case.shift - LAG_FRAMES).round() as i64 - case.truth.offset;
    (frame >= 0).then(|| rolls_heart_piece(StateWalk::new(case.truth.blinks()).state_for_roll(frame)))
}

fn run_case(case: &Case, verbose: bool) -> Outcome {
    let mut outcome = Outcome::default();
    let mut tracker = ManualTracker::new();
    let mut seen = HashSet::new();
    for (i, &press) in case.presses.iter().enumerate() {
        let now = press.frame;
        tracker.add(press);
        let started = Instant::now();
        tracker.update(now);
        outcome.scan_seconds = outcome.scan_seconds.max(started.elapsed().as_secs_f64());
        let right = tracker.followed().first().is_some_and(|top| is_true(top, case, now));
        match (right, outcome.right_from) {
            (true, None) => outcome.right_from = Some(i + 1),
            (false, _) => outcome.right_from = None,
            _ => {}
        }
        let status = tracker.status();
        if status == Status::Locked && outcome.lock.is_none() {
            outcome.lock = Some((i + 1, right));
        }
        if status == Status::Locked {
            if let Some(window) = tracker.next_window(now as i64 + CUE_LEAD_FRAMES, MIN_WINDOW_FRAMES)
                && seen.insert((window.first_roll, window.last_roll))
                && let Some(rolled) = target_rolls(window, case)
            {
                outcome.locked.push(rolled);
            }
        } else if let Some((window, chance)) =
            tracker.likely_window(now as i64 + LIKELY_LEAD_FRAMES, MIN_WINDOW_FRAMES, LIKELY_MIN_CHANCE)
            && seen.insert((window.first_roll, window.last_roll))
            && let Some(rolled) = target_rolls(window, case)
        {
            outcome.likely.push((chance, rolled));
        }
        if verbose {
            let top = tracker.followed().first();
            println!(
                "  press {:2}: {:7} readings, top state {:5.1}%, cost {:5.1}, {}{}",
                i + 1,
                tracker.followed().len(),
                tracker.top_state_share() * 100.0,
                top.map_or(0.0, |top| top.score.cost),
                if right { "true" } else { "not true" },
                if status == Status::Locked { ", locked" } else { "" },
            );
        }
    }
    outcome
}

fn report(cases: &[Case], verbose: bool) {
    let mut likely = Vec::new();
    let mut locked = Vec::new();
    let (mut locks, mut wrong_locks, mut lock_presses) = (0, 0, 0);
    for case in cases {
        if verbose {
            println!("{}: {} presses", case.name, case.presses.len());
        }
        let outcome = run_case(case, verbose);
        let lock = match outcome.lock {
            Some((at, true)) => format!("locked at {at}"),
            Some((at, false)) => format!("WRONG lock at {at}"),
            None => "no lock".to_owned(),
        };
        let right = outcome.right_from.map_or("never".to_owned(), |at| at.to_string());
        println!(
            "{}: {lock}, top reading true from press {right}, slowest update {:.1} s",
            case.name, outcome.scan_seconds
        );
        if let Some((at, right)) = outcome.lock {
            locks += 1;
            lock_presses += at;
            wrong_locks += u32::from(!right);
        }
        likely.extend(outcome.likely);
        locked.extend(outcome.locked);
    }
    println!(
        "{} reads: {locks} locked, {wrong_locks} wrong, on average at press {:.1}.",
        cases.len(),
        lock_presses as f64 / f64::from(locks.max(1))
    );
    println!(
        "Locked windows, target gives the Heart Piece: {} of {}.",
        locked.iter().filter(|&&r| r).count(),
        locked.len()
    );
    for (low, high) in [(0.7, 0.9), (0.9, 1.01)] {
        let band: Vec<_> = likely.iter().filter(|(chance, _)| (low..high).contains(chance)).collect();
        println!(
            "Likely windows at {:.0}% and up: {} of {}.",
            low * 100.0,
            band.iter().filter(|(_, rolled)| *rolled).count(),
            band.len()
        );
    }
}

/// A small generator, so simulated reads come out the same every time.
struct Random(u64);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        (-2.0 * self.next().max(1e-12).ln()).sqrt() * (std::f64::consts::TAU * self.next()).cos()
    }
}

/// Presses like a person's, from a random state.
fn simulated(i: usize, random: &mut Random, sigma: f64, four_as_three: f64) -> Case {
    let (seed, link) = ((random.next() * f64::from(STATES)) as u32, 1 + (random.next() * 99.0) as i64);
    let truth = Candidate { seed, link_first: link, offset: 0 };
    let mut blinks = truth.blinks();
    let presses = (0..SIMULATED_PRESSES)
        .map(|_| {
            let flutter = blinks.next_flutter();
            let frame = (flutter.start + flutter_len(flutter.count)) as f64 + LAG_FRAMES + sigma * random.normal();
            let count = if flutter.count == 4 && random.next() < four_as_three { 3 } else { flutter.count };
            Press { frame, count }
        })
        .collect();
    Case { name: format!("simulated {i}"), presses, truth, shift: 0.0 }
}

fn logged(text: &str) -> Vec<Case> {
    presslog::parse(text)
        .into_iter()
        .enumerate()
        .filter_map(|(i, read)| {
            let truth = read.proven?;
            let presses: Vec<Press> = read
                .presses
                .iter()
                .zip(&read.seconds)
                .map(|(press, seconds)| Press { frame: seconds * GAME_FPS, count: press.count })
                .collect();
            if presses.is_empty() {
                return None;
            }
            let shift = read.presses.iter().zip(&presses).map(|(log, press)| log.frame - press.frame).sum::<f64>()
                / presses.len() as f64;
            Some(Case { name: format!("read {i}"), presses, truth, shift })
        })
        .collect()
}

/// `dampe press-replay <file>`, or `dampe press-replay --simulate <reads> <spread ms> <share of 4s pressed as 3>`.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [flag, reads, spread, wrong] if flag == "--simulate" => {
            let reads: usize = reads.parse().map_err(|_| "reads needs a number")?;
            let spread: f64 = spread.parse().map_err(|_| "spread needs a number")?;
            let wrong: f64 = wrong.parse().map_err(|_| "share needs a number")?;
            let mut random = Random(0x9E37_79B9_7F4A_7C15);
            let cases: Vec<Case> =
                (0..reads).map(|i| simulated(i, &mut random, spread / 1000.0 * GAME_FPS, wrong)).collect();
            report(&cases, false);
            Ok(())
        }
        [path] => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("couldn't read '{path}': {e}"))?;
            report(&logged(&text), true);
            Ok(())
        }
        _ => {
            Err("usage: dampe press-replay <file>, or --simulate <reads> <spread ms> <share of 4s pressed as 3>"
                .to_owned())
        }
    }
}
