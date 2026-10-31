//! A whole read, from pictures to when to press A. Used by the live app and by replays, so replays
//! test what the live app does. It only ever reads video.

use std::sync::mpsc::{Receiver, Sender, channel};

use crate::detector::{Cadence, Detector, Flutter, Health, Rect};
use crate::picture::{GameArea, HEIGHT, Luma, Normalizer, WIDTH};
use crate::rng::{BOX_GONE_TO_ROLL, StateWalk, rolls_heart_piece};
use crate::solver::{Candidate, Observation};
use crate::textbox::{Change, FRAMES_TO_CLOSE, TextboxWatch};
use crate::tracker::{Status, Tracker, Window};

/// Game frames between now and the earliest cue, so the countdown can be followed.
const CUE_LEAD_FRAMES: i64 = 100;
/// The shortest window worth pressing for: about a human's spread, plus the clock's.
const MIN_WINDOW_FRAMES: i64 = 13;
/// Game frames for the clock's line through arrival times.
const CLOCK_FRAMES: usize = 400;
/// The clock until it has seen enough: an NTSC console at 3 video frames per game frame.
const DEFAULT_GAME_FPS: f64 = 60000.0 / 1001.0 / 3.0;
const CLOCK_MIN_FRAMES: usize = 40;

/// Game frames to seconds, as a straight line through when they arrived. A camera's own timestamps
/// come from its own timer, not the game.
#[derive(Default)]
pub struct GameClock {
    samples: std::collections::VecDeque<(i64, f64)>,
}

impl GameClock {
    pub fn add(&mut self, frame: i64, seconds: f64) {
        self.samples.push_back((frame, seconds));
        if self.samples.len() > CLOCK_FRAMES {
            self.samples.pop_front();
        }
    }

    /// When `frame` arrives, or arrived.
    pub fn seconds_at(&self, frame: i64) -> f64 {
        let n = self.samples.len() as f64;
        let Some(&(last_frame, last_seconds)) = self.samples.back() else { return 0.0 };
        if self.samples.len() < CLOCK_MIN_FRAMES {
            return last_seconds + (frame - last_frame) as f64 / DEFAULT_GAME_FPS;
        }
        let mean_frame = self.samples.iter().map(|s| s.0 as f64).sum::<f64>() / n;
        let mean_seconds = self.samples.iter().map(|s| s.1).sum::<f64>() / n;
        let (mut covariance, mut variance) = (0.0, 0.0);
        for &(f, s) in &self.samples {
            covariance += (f as f64 - mean_frame) * (s - mean_seconds);
            variance += (f as f64 - mean_frame).powi(2);
        }
        mean_seconds + covariance / variance * (frame as f64 - mean_frame)
    }

    /// Game frames per second, as measured.
    pub fn fps(&self) -> f64 {
        if self.samples.len() < CLOCK_MIN_FRAMES {
            return DEFAULT_GAME_FPS;
        }
        1.0 / (self.seconds_at(1) - self.seconds_at(0))
    }
}

/// What a read has to say.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Flutter(Flutter),
    Status(Status),
    /// Game frames were counted as skipped, from a gap of this many seconds.
    Skipped {
        frames: i64,
        gap: f64,
    },
    /// The Yes/No box opened: everything starts from here.
    BoxOpened,
    /// Press A so the text box leaves on `box_frame`, which happens at `seconds`.
    Cue {
        box_frame: i64,
        seconds: f64,
        window: Window,
    },
    /// A cue isn't valid any more: the video contradicted it.
    CueCancelled,
    /// The text box left on `game_frame`, at `seconds`. This ends the read.
    BoxGone {
        game_frame: i64,
        seconds: f64,
        heart_piece: Option<bool>,
    },
}

struct Update {
    status: Status,
    candidates: Vec<Candidate>,
    window: Option<Window>,
}

enum Work {
    Observe(Observation, bool, i64),
    Reset,
}

/// Searches can take seconds, and video doesn't wait.
fn tracker_thread(work: Receiver<Work>, updates: Sender<Update>) {
    let mut tracker = Tracker::new();
    while let Ok(job) = work.recv() {
        let mut now = None;
        let mut jobs = vec![job];
        // Whatever came in during the last search gets judged together
        jobs.extend(work.try_iter());
        for job in jobs {
            match job {
                Work::Reset => {
                    tracker.reset();
                    now = None;
                }
                Work::Observe(observation, clean, at) => {
                    tracker.add(observation, clean);
                    now = Some(at);
                }
            }
        }
        let Some(now) = now else { continue };
        tracker.update(now);
        let locked = tracker.is_locked();
        let window = locked.then(|| tracker.next_window(now + CUE_LEAD_FRAMES, MIN_WINDOW_FRAMES)).flatten();
        let update = Update { status: tracker.status(), candidates: tracker.candidates().to_vec(), window };
        if updates.send(update).is_err() {
            return;
        }
    }
}

/// A read in progress.
pub struct Session {
    normalizer: Normalizer,
    cadence: Cadence,
    detector: Detector,
    clock: GameClock,
    picture: Vec<u8>,
    textbox: TextboxWatch,
    /// Whether to wait for the Yes/No box, which takes color. Recordings without it read everything.
    needs_box: bool,
    game_frame: i64,
    frames_seen: u64,
    last_new_frame: Option<f64>,
    work: Sender<Work>,
    updates: Receiver<Update>,
    status: Status,
    candidates: Vec<Candidate>,
    cue: Option<Window>,
    /// Wait for each search instead of continuing, so a replay isn't ahead of its own searches.
    wait_for_searches: bool,
}

impl Session {
    pub fn new(wait_for_searches: bool, needs_box: bool) -> Self {
        let (work, work_receiver) = channel();
        let (update_sender, updates) = channel();
        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || tracker_thread(work_receiver, update_sender))
            .expect("couldn't start the search thread");
        Self {
            normalizer: Normalizer::new(),
            cadence: Cadence::new(),
            detector: Detector::new(),
            clock: GameClock::default(),
            picture: vec![0; WIDTH * HEIGHT],
            textbox: TextboxWatch::default(),
            needs_box,
            game_frame: -1,
            frames_seen: 0,
            last_new_frame: None,
            work,
            updates,
            status: Status::Collecting(0),
            candidates: Vec::new(),
            cue: None,
            wait_for_searches,
        }
    }

    pub fn fps(&self) -> f64 {
        self.clock.fps()
    }

    /// Where the game is in the feed's frames, once that's known.
    pub fn game_area(&self) -> Option<GameArea> {
        self.normalizer.game_area()
    }

    /// Counts up with each new game frame.
    pub fn frames_seen(&self) -> u64 {
        self.frames_seen
    }

    pub fn blobs(&self) -> impl Iterator<Item = Rect> + '_ {
        self.detector.blobs()
    }

    pub fn expected_eye(&self) -> Option<Rect> {
        self.detector.expected_eye()
    }

    /// How the detector has been doing since the last call.
    pub fn take_health(&mut self) -> Health {
        self.detector.take_health()
    }

    /// Starts over: new talk, new camera angle.
    pub fn restart(&mut self) {
        let _ = self.work.send(Work::Reset);
        while self.updates.try_recv().is_ok() {}
        self.cadence = Cadence::new();
        self.detector = Detector::new();
        self.status = Status::Collecting(0);
        self.candidates.clear();
        self.cue = None;
        self.game_frame = -1;
        self.last_new_frame = None;
        self.clock = GameClock::default();
    }

    /// Starts over for a new box: nothing from before it counts.
    fn start_read(&mut self) {
        let _ = self.work.send(Work::Reset);
        while self.updates.try_recv().is_ok() {}
        self.detector = Detector::new();
        self.status = Status::Collecting(0);
        self.candidates.clear();
        self.cue = None;
        self.game_frame = -1;
        self.last_new_frame = None;
        self.clock = GameClock::default();
    }

    /// The next video frame, which arrived at `seconds`. `chroma` is its U and V, alternating, for
    /// every 2 by 2 pixels.
    pub fn push(&mut self, frame: Luma<'_>, chroma: &[u8], seconds: f64, events: &mut Vec<Event>) {
        self.collect_updates(events);
        if !self.cadence.is_new_frame(frame) || !self.normalizer.process(frame, &mut self.picture) {
            return;
        }
        self.frames_seen += 1;
        if self.needs_box
            && let Some(area) = self.normalizer.game_area()
        {
            match self.textbox.observe(frame, chroma, area) {
                Some(Change::Opened) => {
                    self.start_read();
                    events.push(Event::BoxOpened);
                }
                Some(Change::Closed) => {
                    // It left on the first frame without it
                    let game_frame = self.game_frame + 1 - i64::from(FRAMES_TO_CLOSE - 1);
                    let heart_piece = (!self.candidates.is_empty()).then(|| self.would_roll_heart_piece(game_frame));
                    let seconds = self.clock.seconds_at(game_frame);
                    events.push(Event::BoxGone { game_frame, seconds, heart_piece });
                    self.start_read();
                    return;
                }
                None => {}
            }
            if !self.textbox.is_open() {
                return;
            }
        }
        if let Some(last) = self.last_new_frame {
            let skipped = skipped_frames(seconds - last);
            if skipped > 0 {
                events.push(Event::Skipped { frames: skipped, gap: seconds - last });
            }
            self.game_frame += skipped;
            self.detector.skip(skipped);
        }
        self.last_new_frame = Some(seconds);
        self.game_frame += 1;
        self.clock.add(self.game_frame, seconds);

        let (_, flutters) = self.detector.push(&self.picture);
        for flutter in flutters {
            events.push(Event::Flutter(flutter));
            let observation = Observation { start: flutter.start, count: flutter.count };
            let _ = self.work.send(Work::Observe(observation, flutter.is_clean(), self.game_frame));
            if self.wait_for_searches
                && let Ok(update) = self.updates.recv()
            {
                self.apply(update, events);
            }
        }
        if let Some(window) = self.cue
            && window.box_gone().1 < self.game_frame
        {
            // That one went by: the next read of the tracker brings another
            self.cue = None;
        }
    }

    fn collect_updates(&mut self, events: &mut Vec<Event>) {
        while let Ok(update) = self.updates.try_recv() {
            self.apply(update, events);
        }
    }

    fn apply(&mut self, update: Update, events: &mut Vec<Event>) {
        if update.status != self.status {
            events.push(Event::Status(update.status.clone()));
        }
        self.status = update.status;
        self.candidates = update.candidates;
        if self.status != Status::Locked {
            if self.cue.take().is_some() {
                events.push(Event::CueCancelled);
            }
            return;
        }
        if self.cue.is_none()
            && let Some(window) = update.window
        {
            let (first, last) = window.box_gone();
            let box_frame = (first + last) / 2;
            self.cue = Some(window);
            events.push(Event::Cue { box_frame, seconds: self.clock.seconds_at(box_frame), window });
        }
    }

    /// Whether the box leaving on `game_frame` means the Heart Piece, for every reading.
    fn would_roll_heart_piece(&self, game_frame: i64) -> bool {
        self.candidates.iter().all(|candidate| {
            let mut walk = StateWalk::new(candidate.blinks());
            rolls_heart_piece(walk.state_for_roll(game_frame + BOX_GONE_TO_ROLL - candidate.offset))
        })
    }
}

/// Game frames that went by unseen, from the time since the last new one. A game frame is 50 ms,
/// and a 60 fps feed of a 59.94 fps console stretches some to 67 ms, so it takes more than that.
fn skipped_frames(gap_seconds: f64) -> i64 {
    if gap_seconds < 0.085 {
        return 0;
    }
    (gap_seconds * DEFAULT_GAME_FPS).round() as i64 - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_follows_the_measured_rate_not_the_default() {
        let mut clock = GameClock::default();
        for frame in 0..200 {
            clock.add(frame, 3.0 + frame as f64 / 18.0);
        }
        assert!((clock.fps() - 18.0).abs() < 1e-6, "{}", clock.fps());
        assert!((clock.seconds_at(400) - (3.0 + 400.0 / 18.0)).abs() < 1e-6);
    }

    #[test]
    fn only_long_gaps_are_skipped_frames() {
        assert_eq!(skipped_frames(0.050), 0);
        assert_eq!(skipped_frames(0.067), 0);
        assert_eq!(skipped_frames(0.100), 1);
        assert_eq!(skipped_frames(0.150), 2);
    }
}
