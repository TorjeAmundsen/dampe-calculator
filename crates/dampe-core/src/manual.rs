//! Finds the RNG state from flutters pressed by hand, with no video. A person's timing is off by a
//! few game frames, and all of it is late by an amount nobody knows, so presses can't be looked up by
//! exact frame the way the detector's flutters are. A reading is scored instead: how far each press
//! is from where the reading puts that flutter, around the lateness that fits the reading best.
//!
//! People rarely miss a flutter or press for nothing, but they do miscount, and mostly one way: on
//! logged presses a real 4 ended up pressed as a 3 about 1 time in 6, and a real 3 as a 4 almost never.

use std::sync::mpsc::{Receiver, Sender, channel};

use crate::reading::{
    CONFIRMATIONS, Candidate, HORIZON, LIKELY_HORIZON, LIKELY_MIN_CONFIDENCE, LIKELY_READINGS, LINK_PHASES, Reading,
    Readings, Status, Window, chances_by_frame, first_likely_window, first_window, scan_all_states,
};
use crate::rng::{BOX_GONE_TO_ROLL, Blinks};

/// Game frames per second, which is all there is without video to measure it from.
pub const GAME_FPS: f64 = 60000.0 / 1001.0 / 3.0;
/// How long after a flutter's end a press comes, on average: reaction and display. Reading flutters
/// doesn't need this, since it's the same for all of them, but cues do. Measured over 5 reads.
pub const DEFAULT_LAG_MS: f64 = 330.0;

/// A press this soon after the last one, with either key, flips that one's count between 3 and 4, and
/// that one keeps its time. Real flutters can't be this close: they end at least 38 game frames
/// apart, which is 1.9 s.
pub const FLIP_SECONDS: f64 = 1.3;

/// Spread of a press around its flutter's end, in game frames. Measured at 1.4 on 3s and 2.7 on 4s,
/// by someone unpracticed.
const SIGMA: f64 = 2.0;
/// A press this far from a flutter isn't for that flutter, in game frames. They end at least 38 apart.
const PAIR_FRAMES: f64 = 10.0;
/// The most one badly timed press can cost. It also doesn't move the fitted lateness.
const OUTLIER_COST: f64 = 6.0;
/// The cost of a flutter with no press: -ln(0.01).
const MISS_COST: f64 = 4.6;
/// The cost of a press with no flutter. Pair costs leave out the height of the bell curve, which is
/// -ln(1 / (SIGMA * sqrt(2 pi))) = 1.6, so this is what's left of a stray once that's taken off: -ln(0.01)
/// for it happening, ln(49) for landing anywhere in the 49 frames between flutters, and ln(2) for its
/// count. Costing it like a miss made one about 20 times too likely.
const STRAY_COST: f64 = 7.6;
/// The cost of each count pressed for each real count: -ln(chance).
const THREE_AS_THREE: f64 = 0.03;
const THREE_AS_FOUR: f64 = 3.5;
const FOUR_AS_FOUR: f64 = 0.2;
const FOUR_AS_THREE: f64 = 1.7;

/// A reading is dropped over this cost. A real one costs about 0.9 per press.
const BASE_LIMIT: f64 = 8.0;
const LIMIT_PER_PRESS: f64 = 1.3;

/// Presses before a scan: fewer leaves too many readings to follow. At 7, up to 13 million fit, and
/// keeping the cheapest [`MAX_FOLLOWED`] lost the real one on 5 of 40 simulated reads at the spread
/// measured on 4s, against none of 40 at 9. A lock needs more presses than this anyway.
const SCAN_PRESSES: usize = 9;
/// Presses between scans while nothing fits. The first press of a scan has to be a real flutter.
const PRESSES_BETWEEN_SCANS: usize = 3;
/// Readings followed after a scan, the cheapest ones.
const MAX_FOLLOWED: usize = 300_000;

/// How much less likely a Link phase is than if all were equally so, as a cost. His timer waits 20 to 99
/// frames, and a renewal lands inside a long wait more often than a short one, so his next draw is
/// `k` frames away in proportion to the waits at least that long: all 80 up to 20, then fewer. Those
/// weights add up to 59.5 of 99, which this takes off so the average stays where
/// [`NOT_FOUND_HEAD_START`] counts it.
fn link_phase_cost(link_first: i64) -> f64 {
    let waits_that_long = (100 - link_first.max(20)) as f64 / 80.0;
    -(waits_that_long * 99.0 / 59.5).ln()
}

/// The share of all readings' weight one state needs to be trusted.
const LOCK_SHARE: f64 = 0.99;
/// Presses a reading has to explain to be trusted.
const LOCK_PRESSES: u32 = 12;
/// What presses cost if the real reading isn't being followed, as one number to weigh against readings.
/// Readings get a head start of ln(2^23 states * 99 Link phases), since the best of that many fits
/// anything somewhat by chance. Scored on simulated presses unrelated to any state, this took locks from
/// 19 of 20 to 0, and with a stray first press, wrong locks from 8 of 20 to 0, for up to 2 presses later.
const NOT_FOUND_HEAD_START: f64 = 20.5;
/// Each press with no reading behind it: ln(30) for landing anywhere in the 30 frames a wait can take,
/// ln(2) for its count, less the 1.6 pair costs leave out (see [`STRAY_COST`]).
const NOT_FOUND_PER_PRESS: f64 = 2.49;

/// Presses in a row with no flutter anywhere near them that end a lock. Only those can: stopping once
/// it's locked is the point, so flutters with no press say nothing, and one stray press from a hand
/// that hasn't stopped yet isn't enough. A wrong state's flutters land near a press about 1 time in 3.
const LOCK_CONTRADICTIONS: u32 = 3;

/// Game frames between now and the first dust of the earliest cue, so the countdown can be followed:
/// the box leaves 33 frames before that, and the press about 10 before the box, which leaves a press
/// about 5 s away.
pub const CUE_LEAD_FRAMES: i64 = 100 + BOX_GONE_TO_ROLL;
/// The same for a likely cue. Its point is being early, so it's shorter: just its countdown of 3 s.
pub const LIKELY_LEAD_FRAMES: i64 = 70 + BOX_GONE_TO_ROLL;
/// How long before the box should leave those leads allow for: the default lag, and Wii VC's latency.
const TYPICAL_PRESS_LEAD_SECONDS: f64 = (DEFAULT_LAG_MS + 150.0) / 1000.0;
/// The shortest window worth pressing for: about a human's spread, plus the clock's.
const MIN_WINDOW_FRAMES: i64 = 13;
/// How far from a likely window's target the frames have to stay likely, for it to stay.
const LIKELY_TARGET_FRAMES: i64 = 4;
/// A chance this high is every reading agreeing.
const CERTAIN: f64 = 0.999;
/// Readings sent to be shown, more than fit a window of the usual size.
const SHOWN_READINGS: usize = 16;
/// How often to look for a window with no presses coming in, in seconds. It has to run beside OBS.
const WINDOW_LOOK_SECONDS: f64 = 0.5;

/// A flutter pressed by hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Press {
    /// When, in game frames on the clock presses are timed by.
    pub frame: f64,
    pub count: u8,
}

/// How a reading compares to the presses.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    /// Presses with a flutter of their own.
    pub pairs: u32,
    /// How late presses are around this reading's flutters, in game frames.
    pub lateness: f64,
    /// The pairs that lateness comes from: not the badly timed ones.
    timed: u32,
    /// Lower is better: minus the log of how likely the presses are.
    pub cost: f64,
    pub misses: u32,
    pub strays: u32,
}

#[inline]
fn count_cost(real: u8, pressed: u8) -> f64 {
    match (real, pressed) {
        (3, 3) => THREE_AS_THREE,
        (3, _) => THREE_AS_FOUR,
        (_, 4) => FOUR_AS_FOUR,
        _ => FOUR_AS_THREE,
    }
}

#[inline]
fn limit(pairs: u32) -> f64 {
    BASE_LIMIT + LIMIT_PER_PRESS * f64::from(pairs)
}

impl Score {
    /// A reading whose first flutter, of `count` and ending on model frame `end`, is `press`.
    #[inline]
    fn first(press: Press, end: i64, count: u8) -> Self {
        Self {
            pairs: 1,
            lateness: press.frame - end as f64,
            timed: 1,
            cost: count_cost(count, press.count),
            ..Self::default()
        }
    }

    #[inline]
    fn pair(&mut self, press: Press, late_by: f64, count: u8) {
        let n = f64::from(self.timed);
        // How far off it is around a lateness that's only known so well, so the sum doesn't depend on order
        let timing = late_by * late_by * n / (n + 1.0) / (2.0 * SIGMA * SIGMA);
        if timing >= OUTLIER_COST {
            self.cost += OUTLIER_COST;
        } else {
            self.cost += timing;
            self.lateness += late_by / (n + 1.0);
            self.timed += 1;
        }
        self.cost += count_cost(count, press.count);
        self.pairs += 1;
    }
}

/// Goes on from `score` through `presses`, up to `now` in press frames. `None` once it costs too much.
#[inline]
fn walk(mut blinks: Blinks, mut score: Score, presses: &[Press], now: f64) -> Option<Score> {
    let mut next = 0;
    loop {
        let flutter = blinks.next_flutter();
        let end = flutter.end() as f64 + score.lateness;
        loop {
            let Some(&press) = presses.get(next) else {
                // Past the last press: a flutter only counts as missed once its press can't come any more
                if end + PAIR_FRAMES >= now {
                    return Some(score);
                }
                score.misses += 1;
                score.cost += MISS_COST;
                break;
            };
            let late_by = press.frame - end;
            if late_by < -PAIR_FRAMES {
                score.strays += 1;
                score.cost += STRAY_COST;
                next += 1;
                continue;
            }
            if late_by > PAIR_FRAMES {
                score.misses += 1;
                score.cost += MISS_COST;
                break;
            }
            score.pair(press, late_by, flutter.count);
            next += 1;
            break;
        }
        if score.cost > limit(score.pairs) {
            return None;
        }
    }
}

/// Scores a reading whose first flutter is `presses[0]`.
fn compare(seed: u32, link_first: i64, presses: &[Press], now: f64) -> Option<Score> {
    let mut blinks = Blinks::new(seed, link_first);
    let first = blinks.next_flutter();
    let score = Score::first(presses[0], first.end(), first.count);
    walk(blinks, score, &presses[1..], now)
}

/// A reading that's still in the running.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Followed {
    pub seed: u32,
    pub link_first: i64,
    pub score: Score,
}

impl Followed {
    /// What shares come from, and what readings are ranked by. The limit on a reading is only about how
    /// it fits the presses, so this isn't in its score: a rare phase is still a possible one.
    fn total_cost(&self) -> f64 {
        self.score.cost + link_phase_cost(self.link_first)
    }

    /// Model frames plus this candidate's offset are press frames.
    pub fn candidate(&self) -> Candidate {
        Candidate { seed: self.seed, link_first: self.link_first, offset: self.score.lateness.round() as i64 }
    }

    /// Whether it has a flutter ending close enough to `press` to be the one pressed.
    fn has_flutter_near(&self, press: Press) -> bool {
        let mut blinks = Blinks::new(self.seed, self.link_first);
        loop {
            let flutter = blinks.next_flutter();
            let end = flutter.end() as f64 + self.score.lateness;
            if end + PAIR_FRAMES >= press.frame {
                return end - PAIR_FRAMES <= press.frame;
            }
        }
    }

    /// The RNG state after its last flutter whose press can't still come, as of press frame `now`.
    /// Readings with the same state have the same past whatever their seeds, and only differ in when
    /// Link next blinks. The flutter coming next can't tell, since a Link draw may or may not be in it.
    pub fn state_at(&self, now: f64) -> u32 {
        let mut blinks = Blinks::new(self.seed, self.link_first);
        let mut state = blinks.state;
        loop {
            let flutter = blinks.next_flutter();
            if flutter.end() as f64 + self.score.lateness + PAIR_FRAMES >= now {
                return state;
            }
            state = blinks.state;
        }
    }
}

fn scan_seeds(from: u32, to: u32, presses: &[Press], now: f64) -> Vec<Followed> {
    let mut found = Vec::new();
    for seed in from..to {
        // The first flutter has no Link draw before it, whatever the phase
        let mut start = Blinks::new(seed, i64::MAX);
        let flutter = start.next_flutter();
        let score = Score::first(presses[0], flutter.end(), flutter.count);
        for link_first in LINK_PHASES {
            let blinks = Blinks { link_next: link_first, ..start };
            if let Some(score) = walk(blinks, score, &presses[1..], now) {
                found.push(Followed { seed, link_first, score });
            }
        }
    }
    found
}

/// Every reading whose first flutter is `presses[0]`.
pub fn scan(presses: &[Press], now: f64) -> Vec<Followed> {
    scan_all_states(|from, to| scan_seeds(from, to, presses, now))
}

/// Follows presses to a state.
#[derive(Default)]
pub struct ManualTracker {
    presses: Vec<Press>,
    /// The press the followed readings start from.
    anchor: usize,
    /// Cheapest first.
    followed: Vec<Followed>,
    presses_at_scan: Option<usize>,
    /// As of the last update, in press frames.
    now: f64,
    /// Only readings of one state are left, and the rest aren't coming back, unless presses go on to
    /// contradict it: a state in doubt for a moment shouldn't cancel a cue.
    locked: bool,
    /// Presses judged since locking.
    locked_presses: usize,
    contradictions: u32,
}

impl ManualTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, press: Press) {
        self.presses.push(press);
    }

    /// The last press had the wrong count.
    pub fn flip_last(&mut self) {
        if let Some(press) = self.presses.last_mut() {
            press.count = 7 - press.count;
        }
    }

    /// Time went by with no press. Stopping once locked is fine, so this doesn't count any as missed.
    pub fn set_now(&mut self, now: f64) {
        self.now = now;
    }

    /// Judges the presses so far, as of `now` in press frames.
    pub fn update(&mut self, now: f64) {
        self.now = now;
        if self.locked {
            self.check_lock();
            if self.locked {
                return;
            }
        }
        let presses = &self.presses[self.anchor.min(self.presses.len())..];
        self.followed.retain_mut(|followed| match compare(followed.seed, followed.link_first, presses, now) {
            Some(score) => {
                followed.score = score;
                true
            }
            None => false,
        });
        if self.followed.is_empty() {
            self.scan(now);
        }
        self.followed.sort_unstable_by(|a, b| a.total_cost().total_cmp(&b.total_cost()));
        self.followed.truncate(MAX_FOLLOWED);
        if let Some(best) = self.followed.first()
            && best.score.pairs >= LOCK_PRESSES
            && self.top_state_share() >= LOCK_SHARE
        {
            let state = best.state_at(now);
            self.followed.truncate(LIKELY_READINGS);
            self.followed.retain(|followed| followed.state_at(now) == state);
            self.locked = true;
            self.locked_presses = self.presses.len();
            self.contradictions = 0;
        }
    }

    /// Looks at presses that came after locking.
    fn check_lock(&mut self) {
        for &press in &self.presses[self.locked_presses..] {
            if self.followed.iter().any(|followed| followed.has_flutter_near(press)) {
                self.contradictions = 0;
            } else {
                self.contradictions += 1;
            }
        }
        self.locked_presses = self.presses.len();
        if self.contradictions >= LOCK_CONTRADICTIONS {
            self.locked = false;
            self.followed.clear();
            self.presses_at_scan = None;
        }
    }

    /// Scans from recent presses, if it's time. Older ones have more of what went wrong behind them.
    fn scan(&mut self, now: f64) {
        let due = self.presses_at_scan.is_none_or(|at| self.presses.len() >= at + PRESSES_BETWEEN_SCANS);
        if self.presses.len() < SCAN_PRESSES || !due {
            return;
        }
        self.anchor = self.presses.len() - SCAN_PRESSES;
        self.presses_at_scan = Some(self.presses.len());
        self.followed = scan(&self.presses[self.anchor..], now);
    }

    pub fn followed(&self) -> &[Followed] {
        &self.followed
    }

    /// What presses that fit no reading cost: the real reading isn't being followed at all. Without
    /// this, whatever is left after a bad scan can reach any share, however badly it fits.
    fn not_found_cost(&self) -> f64 {
        NOT_FOUND_PER_PRESS * self.presses.len().saturating_sub(self.anchor) as f64 - NOT_FOUND_HEAD_START
    }

    /// The chance of each followed reading, in order, and the chance it's none of them. A lock has
    /// accepted its state, so nothing's held back for none of them after that.
    fn shares_and_not_found(&self) -> (Vec<f64>, f64) {
        let Some(best) = self.followed.first() else { return (Vec::new(), 0.0) };
        let best = best.total_cost();
        let weights: Vec<f64> = self.followed.iter().map(|f| (best - f.total_cost()).exp()).collect();
        let not_found = if self.locked { 0.0 } else { (best - self.not_found_cost()).exp() };
        let total = weights.iter().sum::<f64>() + not_found;
        (weights.into_iter().map(|weight| weight / total).collect(), not_found / total)
    }

    /// The chance of each followed reading, in order.
    pub fn shares(&self) -> Vec<f64> {
        self.shares_and_not_found().0
    }

    /// The likeliest reading's state's share of the weight. Looking at all of a big set would be slow,
    /// and with that many left, none of them has much of a share anyway.
    pub fn top_state_share(&self) -> f64 {
        let Some(best) = self.followed.first() else { return 0.0 };
        let state = best.state_at(self.now);
        self.shares()
            .iter()
            .zip(&self.followed)
            .take(LIKELY_READINGS)
            .filter(|(_, followed)| followed.state_at(self.now) == state)
            .map(|(share, _)| share)
            .sum()
    }

    pub fn status(&self) -> Status {
        match self.followed.len() {
            0 => Status::Collecting(self.presses.len()),
            _ if self.locked => Status::Locked,
            n => Status::Narrowing(n),
        }
    }

    /// The chance of a dig on each of `frames`, in press frames, giving the Heart Piece.
    fn chances(&self, frames: std::ops::Range<i64>) -> Vec<f64> {
        let shares = self.shares();
        let readings = shares.iter().zip(&self.followed).take(LIKELY_READINGS);
        chances_by_frame(readings.map(|(&share, followed)| (share, followed.candidate())), frames)
    }

    /// The first window from press frame `from` on, at least `min_frames` long, where every frame has
    /// at least `min_chance` of giving the Heart Piece. Also the chance at its middle.
    pub fn likely_window(&self, from: i64, min_frames: i64, min_chance: f64) -> Option<(Window, f64)> {
        let chances = self.chances(from..from + LIKELY_HORIZON);
        let window = first_likely_window(&chances, from, min_frames, min_chance)?;
        Some((window, chances[(window.target() - from) as usize]))
    }

    /// The lowest chance of any frame of `window` giving the Heart Piece.
    fn window_chance(&self, window: Window) -> f64 {
        self.chances(window.first_roll..window.last_roll + 1).into_iter().min_by(f64::total_cmp).unwrap_or(0.0)
    }

    /// The first window for every reading of the likeliest state, from press frame `from` on, since
    /// those can still differ in when Link next blinks.
    pub fn next_window(&self, from: i64, min_frames: i64) -> Option<Window> {
        let state = self.followed.first()?.state_at(self.now);
        let same_state: Vec<Candidate> = self
            .followed
            .iter()
            .take(LIKELY_READINGS)
            .filter(|followed| self.locked || followed.state_at(self.now) == state)
            .map(Followed::candidate)
            .collect();
        first_window(&same_state, from, min_frames, HORIZON)
    }

    /// The `limit` likeliest readings, to show.
    fn readings(&self, limit: usize) -> Readings {
        let (shares, not_found) = self.shares_and_not_found();
        let top = shares
            .iter()
            .zip(&self.followed)
            .take(limit)
            .map(|(&share, followed)| Reading {
                candidate: followed.candidate(),
                share,
                hits: followed.score.pairs,
                misses: followed.score.misses,
                extras: followed.score.strays,
                confirmed: if self.locked { CONFIRMATIONS } else { 0 },
                next_window: first_window(
                    &[followed.candidate()],
                    self.now as i64 + BOX_GONE_TO_ROLL,
                    MIN_WINDOW_FRAMES,
                    HORIZON,
                ),
            })
            .collect();
        Readings { top, total: self.followed.len(), not_found }
    }
}

/// What a read from presses has to say.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Status(Status),
    /// Press A so the text box leaves at `seconds`, on the clock presses are timed by.
    Cue {
        seconds: f64,
        window: Window,
    },
    /// Probably press A so the text box leaves at `seconds`: no state is proven yet.
    LikelyCue {
        seconds: f64,
        chance: f64,
    },
    /// The chance of the running likely cue changed.
    LikelyChance {
        chance: f64,
    },
    /// A likely cue isn't likely any more.
    LikelyCancelled,
    /// A likely cue's last frame went by, with nothing to take its place.
    LikelyEnded,
    /// A cue isn't valid any more: the presses contradicted it.
    CueCancelled,
}

struct Update {
    generation: u64,
    status: Status,
    readings: Readings,
    window: Option<Window>,
    likely: Option<(Window, f64)>,
}

enum Work {
    Press(Press),
    Flip(f64),
    Tick(f64),
    Reset(u64),
}

/// Game frames to add to the leads, for pressing further ahead of the box than they allow for.
fn extra_lead_frames(press_lead_seconds: f64) -> i64 {
    ((press_lead_seconds - TYPICAL_PRESS_LEAD_SECONDS) * GAME_FPS).ceil().max(0.0) as i64
}

/// Whether a press can still make a window.
fn still_ahead(window: &Window, now: f64) -> bool {
    window.box_gone().1 as f64 >= now
}

/// Scans can take seconds, and the window's clock doesn't wait.
fn search_thread(work: Receiver<Work>, updates: Sender<Update>, extra_lead: i64) {
    let mut tracker = ManualTracker::new();
    let mut likely: Option<Window> = None;
    let mut generation = 0;
    while let Ok(job) = work.recv() {
        let mut jobs = vec![job];
        jobs.extend(work.try_iter());
        let mut pressed = None;
        for job in jobs {
            match job {
                Work::Reset(new) => {
                    tracker = ManualTracker::new();
                    likely = None;
                    generation = new;
                    pressed = None;
                }
                Work::Press(press) => {
                    tracker.add(press);
                    pressed = Some(press.frame);
                }
                Work::Flip(now) => {
                    tracker.flip_last();
                    pressed = Some(now);
                }
                Work::Tick(now) => tracker.set_now(now),
            }
        }
        if let Some(now) = pressed {
            let before = tracker.now;
            let started = std::time::Instant::now();
            tracker.update(now);
            tracker.set_now(before.max(now + started.elapsed().as_secs_f64() * GAME_FPS));
        }
        let now = tracker.now;
        let locked = tracker.status() == Status::Locked;
        likely = likely.filter(|window| {
            let target = window.target();
            let around = Window { first_roll: target - LIKELY_TARGET_FRAMES, last_roll: target + LIKELY_TARGET_FRAMES };
            still_ahead(window, now) && tracker.window_chance(around) >= LIKELY_MIN_CONFIDENCE
        });
        let proven = likely.filter(|&window| locked && tracker.window_chance(window) >= CERTAIN);
        if proven.is_some() {
            likely = None;
        }
        let window = proven.or_else(|| {
            (locked && likely.is_none())
                .then(|| tracker.next_window(now as i64 + CUE_LEAD_FRAMES + extra_lead, MIN_WINDOW_FRAMES))
                .flatten()
        });
        let mut likely_update = likely.map(|window| {
            let target = window.target();
            (window, tracker.chances(target..target + 1)[0])
        });
        if likely.is_none() && window.is_none() {
            likely_update = tracker.likely_window(
                now as i64 + LIKELY_LEAD_FRAMES + extra_lead,
                MIN_WINDOW_FRAMES,
                LIKELY_MIN_CONFIDENCE,
            );
            likely = likely_update.map(|(window, _)| window);
        }
        let update = Update {
            generation,
            status: tracker.status(),
            readings: tracker.readings(SHOWN_READINGS),
            window,
            likely: likely_update,
        };
        if updates.send(update).is_err() {
            return;
        }
    }
}

/// A read from presses, the counterpart of `session::Session` for video. Times are seconds
/// on one clock, and frames are those seconds at [`GAME_FPS`]. Cues say when the text box should leave
/// on the clock presses are timed by: allowing for how late presses are is up to the caller.
pub struct ManualSession {
    work: Sender<Work>,
    updates: Receiver<Update>,
    generation: u64,
    status: Status,
    new_readings: Option<Readings>,
    cue: Option<Window>,
    likely_cue: Option<Window>,
    /// When the last flutter was pressed, not counting flips.
    last_press: Option<f64>,
    presses: usize,
    last_look: f64,
}

impl ManualSession {
    /// `press_lead_seconds` is how long before the box should leave a press has to come.
    pub fn new(press_lead_seconds: f64) -> Self {
        let extra_lead = extra_lead_frames(press_lead_seconds);
        let (work, work_receiver) = channel();
        let (update_sender, updates) = channel();
        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || search_thread(work_receiver, update_sender, extra_lead))
            .expect("couldn't start the search thread");
        Self {
            work,
            updates,
            generation: 0,
            status: Status::Collecting(0),
            new_readings: None,
            cue: None,
            likely_cue: None,
            last_press: None,
            presses: 0,
            last_look: 0.0,
        }
    }

    /// A press of `count` at `seconds`. True if it flipped the last one.
    pub fn press(&mut self, count: u8, seconds: f64) -> bool {
        if self.last_press.is_some_and(|last| seconds - last < FLIP_SECONDS) {
            let _ = self.work.send(Work::Flip(seconds * GAME_FPS));
            return true;
        }
        self.last_press = Some(seconds);
        self.presses += 1;
        let _ = self.work.send(Work::Press(Press { frame: seconds * GAME_FPS, count }));
        false
    }

    /// Flutters pressed in this read, not counting flips.
    pub fn presses(&self) -> usize {
        self.presses
    }

    /// Starts over: a new talk, or Navi came.
    pub fn restart(&mut self) {
        self.generation += 1;
        let _ = self.work.send(Work::Reset(self.generation));
        self.status = Status::Collecting(0);
        self.new_readings = Some(Readings::default());
        self.cue = None;
        self.likely_cue = None;
        self.last_press = None;
        self.presses = 0;
    }

    /// The readings, if they changed since the last call.
    pub fn take_readings(&mut self) -> Option<Readings> {
        self.new_readings.take()
    }

    /// When a text box leaving for `window` should leave, on the clock presses are timed by.
    pub fn window_seconds(window: &Window) -> f64 {
        let (first, last) = window.box_gone();
        (first + last) as f64 / 2.0 / GAME_FPS
    }

    /// Goes on as of `seconds`, adding what there is to say to `events`.
    pub fn tick(&mut self, seconds: f64, events: &mut Vec<Event>) {
        let now = seconds * GAME_FPS;
        while let Ok(update) = self.updates.try_recv() {
            if update.generation == self.generation {
                self.apply(update, now, events);
            }
        }
        if self.cue.is_some_and(|window| !still_ahead(&window, now)) {
            self.cue = None;
        }
        // With no presses coming in, the next window still has to be found
        let needs_window = self.status == Status::Locked && self.cue.is_none();
        if (needs_window || self.likely_cue.is_some()) && seconds - self.last_look >= WINDOW_LOOK_SECONDS {
            self.last_look = seconds;
            let _ = self.work.send(Work::Tick(now));
        }
    }

    fn apply(&mut self, update: Update, now: f64, events: &mut Vec<Event>) {
        if update.status != self.status {
            events.push(Event::Status(update.status.clone()));
        }
        self.status = update.status;
        self.new_readings = Some(update.readings);
        // A certain cue that's running is better than anything likely
        let update_likely = update.likely.filter(|_| self.cue.is_none());
        let new_likely = update_likely.map(|(window, _)| window);
        if self.likely_cue == new_likely
            && let Some((_, chance)) = update_likely
        {
            events.push(Event::LikelyChance { chance });
        }
        if self.likely_cue != new_likely {
            let doubted = self.likely_cue.is_some_and(|window| still_ahead(&window, now));
            if self.likely_cue.take().is_some() {
                if doubted && (new_likely.is_some() || update.window.is_none()) {
                    events.push(Event::LikelyCancelled);
                } else if !doubted && new_likely.is_none() && update.window.is_none() {
                    events.push(Event::LikelyEnded);
                }
            }
            if let Some((window, chance)) = update_likely {
                self.likely_cue = Some(window);
                events.push(Event::LikelyCue { seconds: Self::window_seconds(&window), chance });
            }
        }
        if self.status != Status::Locked {
            if self.cue.take().is_some() {
                events.push(Event::CueCancelled);
            }
            return;
        }
        if self.cue.is_none()
            && let Some(window) = update.window
        {
            self.cue = Some(window);
            events.push(Event::Cue { seconds: Self::window_seconds(&window), window });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{StateWalk, rolls_heart_piece};

    /// Presses exactly `late` frames after each flutter's end, with the right counts.
    fn exact(seed: u32, link: i64, n: usize, late: f64) -> Vec<Press> {
        let mut blinks = Blinks::new(seed, link);
        (0..n)
            .map(|_| {
                let flutter = blinks.next_flutter();
                Press { frame: flutter.end() as f64 + late, count: flutter.count }
            })
            .collect()
    }

    fn cost(presses: &[Press]) -> Option<Score> {
        compare(3_141_592, 40, presses, presses.last().expect("presses").frame)
    }

    /// A press being late is no evidence: everyone's late, by an amount no reading can tell.
    #[test]
    fn the_same_lateness_on_every_press_costs_nothing() {
        let late = cost(&exact(3_141_592, 40, 12, 7.0)).expect("it fits");
        assert!((late.lateness - 7.0).abs() < 1e-9, "{late:?}");
        let on_time = cost(&exact(3_141_592, 40, 12, 0.0)).expect("it fits");
        assert!((late.cost - on_time.cost).abs() < 1e-9);
        assert_eq!((late.pairs, late.misses, late.strays), (12, 0, 0));
    }

    /// The control for the rest: another seed's presses shouldn't fit.
    #[test]
    fn another_seeds_presses_do_not_fit() {
        assert!(cost(&exact(77_777, 40, 12, 7.0)).is_none());
    }

    #[test]
    fn timing_costs_more_the_further_off_and_stops_at_a_limit() {
        let base = exact(3_141_592, 40, 10, 5.0);
        let cost_with = |off: f64| {
            let mut presses = base.clone();
            presses[8].frame += off;
            cost(&presses).expect("it fits").cost
        };
        assert!(cost_with(0.0) < cost_with(2.0));
        assert!(cost_with(2.0) < cost_with(5.0));
        assert_eq!(cost_with(8.0), cost_with(9.0), "one bad press is only so bad");
    }

    /// A badly timed press says nothing about the lateness, so it shouldn't water down the next one.
    #[test]
    fn a_badly_timed_press_does_not_change_how_the_lateness_is_fitted() {
        let press = Press { frame: 0.0, count: 3 };
        let mut score = Score::first(press, 0, 3);
        score.pair(press, 0.0, 3);
        score.pair(press, 0.0, 3);
        score.pair(press, 9.5, 3);
        assert_eq!(score.lateness, 0.0, "the outlier itself doesn't count");
        assert_eq!(score.pairs, 4);
        score.pair(press, 3.0, 3);
        assert_eq!(score.lateness, 3.0 / 4.0, "3 good presses before it, not 4");
    }

    #[test]
    fn link_phases_are_weighted_by_the_waits_that_reach_them() {
        let weight = |phase| (-link_phase_cost(phase)).exp();
        assert_eq!(weight(1), weight(20));
        assert!((weight(20) / weight(90) - 8.0).abs() < 1e-9, "80 waits reach 20, 10 reach 90");
        let average = LINK_PHASES.map(weight).sum::<f64>() / 99.0;
        assert!((average - 1.0).abs() < 1e-9, "{average}");
    }

    /// Two readings that fit the presses the same.
    #[test]
    fn a_reading_with_a_likelier_link_phase_gets_more_share() {
        let presses = exact(3_141_592, 40, 8, 5.0);
        let score = cost(&presses).expect("it fits");
        let followed = |link_first| Followed { seed: 3_141_592, link_first, score };
        let tracker = ManualTracker { presses, followed: vec![followed(10), followed(90)], ..ManualTracker::default() };
        let shares = tracker.shares();
        assert!((shares[0] / shares[1] - 8.0).abs() < 1e-9, "{shares:?}");
    }

    /// Pressing 3 on a real 4 is common, the other way round isn't.
    #[test]
    fn a_three_pressed_for_a_four_costs_less_than_the_other_way_round() {
        let base = exact(3_141_592, 40, 12, 5.0);
        let four = base.iter().position(|p| p.count == 4).expect("no 4 in there");
        let three = base.iter().position(|p| p.count == 3).expect("no 3 in there");
        let wrong = |at: usize| {
            let mut presses = base.clone();
            presses[at].count = 7 - presses[at].count;
            cost(&presses).expect("it fits").cost
        };
        let right = cost(&base).expect("it fits").cost;
        assert!(right < wrong(four));
        assert!(wrong(four) < wrong(three));
    }

    #[test]
    fn a_missed_flutter_and_a_stray_press_are_counted_and_survived() {
        let mut presses = exact(3_141_592, 40, 14, 5.0);
        presses.remove(6);
        let stray = Press { frame: presses[8].frame + 20.0, count: 3 };
        presses.insert(9, stray);
        let score = cost(&presses).expect("it fits");
        assert_eq!((score.pairs, score.misses, score.strays), (13, 1, 1));
    }

    /// All of it together: the state comes out of every state there is, with some timing noise.
    #[test]
    fn presses_lock_the_true_state() {
        let mut presses = exact(3_141_592, 40, 20, 6.0);
        for (i, press) in presses.iter_mut().enumerate() {
            press.frame += [1.5, -2.0, 0.5, 2.5, -1.0][i % 5];
        }
        let mut tracker = ManualTracker::new();
        for &press in &presses[..SCAN_PRESSES - 1] {
            tracker.add(press);
            tracker.update(press.frame);
        }
        assert_eq!(tracker.status(), Status::Collecting(SCAN_PRESSES - 1));
        for &press in &presses[SCAN_PRESSES - 1..] {
            tracker.add(press);
            tracker.update(press.frame);
        }
        assert_eq!(tracker.status(), Status::Locked);
        let top = tracker.followed()[0];
        assert_eq!(top.seed, 3_141_592);
        // The state in doubt for a moment doesn't undo that, and stopping pressing doesn't either
        tracker.set_now(presses[19].frame + 2000.0);
        assert_eq!(tracker.status(), Status::Locked);
        tracker.set_now(presses[19].frame);
        let window = tracker.next_window(presses[19].frame as i64 + 100, 13).expect("no window in the horizon");
        let mut walk = StateWalk::new(top.candidate().blinks());
        for frame in window.first_roll..=window.last_roll {
            assert!(rolls_heart_piece(walk.state_for_roll(frame - top.candidate().offset)));
        }
    }

    /// A tracker locked on a reading that has just had `presses`.
    fn locked_on(presses: &[Press]) -> ManualTracker {
        let score = cost(presses).expect("it fits");
        let followed = vec![Followed { seed: 3_141_592, link_first: 40, score }];
        let locked_presses = presses.len();
        ManualTracker { presses: presses.to_vec(), followed, locked: true, locked_presses, ..ManualTracker::default() }
    }

    /// A tracker following one reading of `cost` over 16 presses.
    fn following_one_of_cost(cost: f64, locked: bool) -> ManualTracker {
        let presses = exact(3_141_592, 40, 16, 5.0);
        let score = Score { cost, ..self::cost(&presses).expect("it fits") };
        let followed = vec![Followed { seed: 3_141_592, link_first: 40, score }];
        ManualTracker { presses, followed, locked, ..ManualTracker::default() }
    }

    /// The only reading left isn't proven by being the only one: it has to fit better than the best of
    /// all states fits presses by chance.
    #[test]
    fn a_lone_reading_that_fits_badly_has_no_share_to_lock_with() {
        assert!(following_one_of_cost(30.0, false).top_state_share() < 0.01);
        assert!(following_one_of_cost(10.0, false).top_state_share() >= LOCK_SHARE);
    }

    /// Worth showing, and a lock has accepted its state.
    #[test]
    fn the_chance_of_none_of_them_is_shown_until_a_lock() {
        let shown = |locked| following_one_of_cost(30.0, locked).readings(1).not_found;
        assert!(shown(false) > 0.99);
        assert_eq!(shown(true), 0.0);
    }

    /// Stopping once it's locked is the point, and a hand doesn't stop on the exact press.
    #[test]
    fn a_lock_survives_stopping_and_a_stray_or_two() {
        let presses = exact(3_141_592, 40, 30, 6.0);
        let mut tracker = locked_on(&presses[..14]);
        // Stopped for many flutters, then pressing again, right ones
        tracker.add(presses[25]);
        tracker.update(presses[25].frame);
        assert_eq!(tracker.status(), Status::Locked);
        // Two strays, and a press that fits resets those, and a miscount is no contradiction
        for off in [20.0, 20.0] {
            let press = Press { frame: presses[26].frame + off, count: 3 };
            tracker.add(press);
            tracker.update(press.frame);
            assert_eq!(tracker.status(), Status::Locked);
        }
        let miscounted = Press { count: 7 - presses[27].count, ..presses[27] };
        tracker.add(miscounted);
        tracker.update(miscounted.frame);
        assert_eq!(tracker.contradictions, 0);
    }

    /// The control: presses that keep landing where it has no flutter do end it.
    #[test]
    fn presses_that_keep_contradicting_a_lock_end_it() {
        let presses = exact(3_141_592, 40, 30, 6.0);
        let mut tracker = locked_on(&presses[..14]);
        for &press in &presses[14..17] {
            let stray = Press { frame: press.frame + 20.0, count: press.count };
            assert_eq!(tracker.status(), Status::Locked);
            tracker.presses.push(stray);
            tracker.check_lock();
        }
        assert!(!tracker.locked);
        assert!(tracker.followed.is_empty(), "it has to search again");
    }

    /// From presses to a countdown, the way the app does it, through a flip and on with no more presses.
    #[test]
    fn a_session_cues_from_presses_and_goes_on_without_them() {
        let presses = exact(3_141_592, 40, 20, 6.0);
        let mut session = ManualSession::new(TYPICAL_PRESS_LEAD_SECONDS);
        let mut events = Vec::new();
        for (i, press) in presses.iter().enumerate() {
            let seconds = press.frame / GAME_FPS;
            assert!(!session.press(if i == 9 { 7 - press.count } else { press.count }, seconds));
            if i == 9 {
                assert!(session.press(3, seconds + 0.5), "either key flips");
            }
            // Each press is judged before the next, and a scan takes a while
            let mut updates = 0;
            let started = std::time::Instant::now();
            while i >= 6 && updates == 0 {
                assert!(started.elapsed().as_secs() < 600, "no update for press {i}");
                session.tick(seconds, &mut events);
                updates += usize::from(session.take_readings().is_some());
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        assert_eq!(session.presses(), 20);
        let cue_of = |event: &Event| match event {
            Event::Cue { seconds, window, .. } => Some((*seconds, *window)),
            _ => None,
        };
        // The clock goes on while waiting, as a real one does, or windows it was looking for never pass
        let mut wait_for_cue = |session: &mut ManualSession, seconds: f64| {
            let started = std::time::Instant::now();
            loop {
                assert!(started.elapsed().as_secs() < 600, "no cue");
                session.tick(seconds + started.elapsed().as_secs_f64(), &mut events);
                if let Some(cue) = events.iter().rev().find_map(cue_of) {
                    events.clear();
                    return cue;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        let (cue_seconds, window) = wait_for_cue(&mut session, presses[19].frame / GAME_FPS);
        let mut walk = StateWalk::new(Blinks::new(3_141_592, 40));
        assert!(rolls_heart_piece(walk.state_for_roll(window.target() - 6)));
        // Long after the last press, with no more of them, the next one still comes
        let (next_seconds, _) = wait_for_cue(&mut session, cue_seconds + 2.0);
        assert!(next_seconds > cue_seconds, "{next_seconds} {cue_seconds}");
        session.restart();
        assert_eq!(session.presses(), 0);
    }

    /// The soonest cue of each kind has to leave its 3, 2, 1 to be heard, at Wii VC's latency of 150 ms.
    #[test]
    fn the_soonest_cues_leave_room_for_the_countdown() {
        for lead in [LIKELY_LEAD_FRAMES, CUE_LEAD_FRAMES] {
            let window = Window { first_roll: lead, last_roll: lead + MIN_WINDOW_FRAMES - 1 };
            let press = ManualSession::window_seconds(&window) - (DEFAULT_LAG_MS + 150.0) / 1000.0;
            assert!(press >= 3.0, "{press} s with a lead of {lead}");
        }
    }

    /// A long lag means pressing long before the box leaves, which the countdown still has to fit.
    #[test]
    fn a_long_lag_moves_the_soonest_cues_back() {
        assert_eq!(extra_lead_frames(TYPICAL_PRESS_LEAD_SECONDS), 0);
        assert_eq!(extra_lead_frames(0.1), 0, "a short one has room to spare");
        let long = 2.15;
        let lead = LIKELY_LEAD_FRAMES + extra_lead_frames(long);
        let window = Window { first_roll: lead, last_roll: lead + MIN_WINDOW_FRAMES - 1 };
        let press = ManualSession::window_seconds(&window) - long;
        assert!(press >= 3.0, "{press} s");
    }

    /// What's said as a likely cue goes away, with its window from frame 100 to 112.
    fn events_as_a_likely_cue_goes(now: f64, window: Option<Window>) -> Vec<Event> {
        let mut session = ManualSession::new(TYPICAL_PRESS_LEAD_SECONDS);
        session.likely_cue = Some(Window { first_roll: 100, last_roll: 112 });
        let status = session.status.clone();
        let mut events = Vec::new();
        session.apply(
            Update { generation: 0, status, readings: Readings::default(), window, likely: None },
            now,
            &mut events,
        );
        events
    }

    /// Nothing else says the moment is over, and the window would go on showing its chance.
    #[test]
    fn a_likely_cue_that_went_by_says_so() {
        assert_eq!(events_as_a_likely_cue_goes(1000.0, None), [Event::LikelyEnded]);
    }

    #[test]
    fn a_likely_cue_still_ahead_is_cancelled_and_not_ended() {
        assert_eq!(events_as_a_likely_cue_goes(0.0, None), [Event::LikelyCancelled]);
    }

    /// The status has to be locked for a certain cue, so this only says nothing's ended.
    #[test]
    fn a_certain_cue_taking_over_is_not_an_end() {
        let window = Window { first_roll: 100, last_roll: 112 };
        assert!(!events_as_a_likely_cue_goes(1000.0, Some(window)).contains(&Event::LikelyEnded));
    }

    /// Flutters after the last press may just not have been pressed yet.
    #[test]
    fn a_flutter_only_counts_as_missed_once_its_press_can_not_come() {
        let presses = exact(3_141_592, 40, 8, 5.0);
        let last = presses.last().expect("presses").frame;
        let score = |now| compare(3_141_592, 40, &presses, now).expect("it fits");
        assert_eq!(score(last).misses, 0);
        assert!(score(last + 100.0).misses >= 1, "long after, the next one was missed");
        assert_eq!(compare(3_141_592, 40, &presses, last + 1000.0), None, "and nobody's pressing any more");
    }
}
