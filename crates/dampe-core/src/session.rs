//! A whole read, from pictures to when to press A. Used by the live app and by replays, so replays
//! test what the live app does. It only ever reads video.

use std::sync::mpsc::{Receiver, Sender, channel};

use crate::detector::{Cadence, Detector, Flutter, Health, Rect};
use crate::navi::NaviWatch;
use crate::picture::{GameArea, HEIGHT, Luma, Normalizer, WIDTH};
use crate::rng::{BOX_GONE_TO_ROLL, StateWalk, rolls_heart_piece};
use crate::solver::{Candidate, Observation};
use crate::textbox::{Change, FRAMES_TO_CLOSE, TextWatch, TextboxWatch};
use crate::tracker::{LIKELY_MIN_CONFIDENCE, Readings, Status, Tracker, Window};

/// Game frames between now and the earliest cue, so the countdown can be followed.
const CUE_LEAD_FRAMES: i64 = 100;
/// The same for a likely cue. Its point is being early, so it's shorter: just its countdown.
const LIKELY_LEAD_FRAMES: i64 = 70;
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

    /// Where `seconds` falls in game frames, once there are any.
    pub fn frame_at(&self, seconds: f64) -> Option<f64> {
        let &(last_frame, _) = self.samples.back()?;
        Some(last_frame as f64 + (seconds - self.seconds_at(last_frame)) * self.fps())
    }
}

/// What a read has to say.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Flutter(Flutter),
    /// The early frames were read again knowing where the eye is, up to `through`.
    ReadAgain {
        through: i64,
        flutters: usize,
    },
    /// The reference is ready, after this many seconds of the box, and this many flutters from
    /// before that were found looking back. They follow as usual.
    FoundLookingBack {
        flutters: usize,
        seconds: f64,
    },
    Status(Status),
    /// Game frames were counted as skipped, from a gap of this many seconds.
    Skipped {
        frames: i64,
        gap: f64,
    },
    /// The Yes/No box opened: everything starts from here.
    BoxOpened,
    /// His text started, before any Yes/No box. Reading starts here.
    TextOpened,
    /// Her light is showing, so the read started over and begins on the next frame.
    NaviSeen,
    /// His text ended with no Yes/No box, so that read is over.
    TextGone,
    /// Press A so the text box leaves on `box_frame`, which happens at `seconds`.
    Cue {
        box_frame: i64,
        seconds: f64,
        window: Window,
    },
    /// Probably press A so the text box leaves on `box_frame`: no state is proven yet.
    LikelyCue {
        box_frame: i64,
        seconds: f64,
        window: Window,
        chance: f64,
    },
    /// The chance of the running likely cue changed.
    LikelyChance {
        chance: f64,
    },
    /// A likely cue isn't likely any more.
    LikelyCancelled,
    /// Navi looks done: flutters from `from` on count, and `held` of those came while waiting.
    NaviDone {
        from: i64,
        held: usize,
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

/// Readings sent to be shown, more than fit a window of the usual size.
const SHOWN_READINGS: usize = 16;

struct Update {
    status: Status,
    readings: Readings,
    candidates: Vec<Candidate>,
    window: Option<Window>,
    likely: Option<(Window, f64)>,
}

enum Work {
    Observe(Observation, bool, i64),
    Replace(i64, Vec<(Observation, bool)>, i64),
    Reset,
}

/// How far from a likely window's target the frames have to stay likely, for it to stay.
const LIKELY_TARGET_FRAMES: i64 = 4;
/// A chance this high is every reading agreeing.
const CERTAIN: f64 = 0.999;

/// Whether a press can still make a window. A cue that's nearly due is the last one to drop.
fn still_ahead(window: &Window, now: i64) -> bool {
    window.box_gone().1 >= now
}

/// Searches can take seconds, and video doesn't wait.
fn tracker_thread(work: Receiver<Work>, updates: Sender<Update>) {
    let mut tracker = Tracker::new();
    let mut likely: Option<Window> = None;
    while let Ok(job) = work.recv() {
        let mut now = None;
        let mut jobs = vec![job];
        // Whatever came in during the last search gets judged together
        jobs.extend(work.try_iter());
        for job in jobs {
            match job {
                Work::Reset => {
                    tracker.reset();
                    likely = None;
                    now = None;
                }
                Work::Replace(through, flutters, at) => {
                    tracker.replace_through(through, &flutters, at);
                    now = Some(at);
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
        // A likely window stays until it's passed or disproven, also once a state is locked: the
        // certain window can be much later. If it turns out certain, it simply becomes the cue.
        // The frames around its target are what matter: edges coming and going shouldn't move a countdown
        likely = likely.filter(|window| {
            let target = (window.first_roll + window.last_roll) / 2;
            let around = Window { first_roll: target - LIKELY_TARGET_FRAMES, last_roll: target + LIKELY_TARGET_FRAMES };
            still_ahead(window, now) && tracker.window_confidence(around) >= LIKELY_MIN_CONFIDENCE
        });
        let proven = likely.filter(|&window| locked && tracker.window_confidence(window) >= CERTAIN);
        if proven.is_some() {
            likely = None;
        }
        let window = proven.or_else(|| {
            (locked && likely.is_none())
                .then(|| tracker.next_window(now + CUE_LEAD_FRAMES, MIN_WINDOW_FRAMES))
                .flatten()
        });
        let mut likely_update = likely.map(|window| (window, tracker.chance_at(window.target())));
        if likely.is_none() && window.is_none() {
            let found = tracker.likely_window(now + LIKELY_LEAD_FRAMES, MIN_WINDOW_FRAMES, LIKELY_MIN_CONFIDENCE);
            likely = found.map(|found| found.window);
            likely_update = found.map(|found| (found.window, found.chance));
        }
        let update = Update {
            status: tracker.status(),
            readings: tracker.readings(SHOWN_READINGS, now, MIN_WINDOW_FRAMES),
            candidates: tracker.candidates().to_vec(),
            window,
            likely: likely_update,
        };
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
    text: TextWatch,
    /// Reading started from a text box, and keeps going when the Yes/No box comes.
    reading_text: bool,
    /// Reading was started by hand, so it doesn't wait for the box, and the box opening changes nothing.
    started_by_hand: bool,
    /// Whether to wait for the Yes/No box, which takes color. Recordings without it read everything.
    needs_box: bool,
    game_frame: i64,
    navi: NaviWatch,
    frames_seen: u64,
    last_new_frame: Option<f64>,
    work: Sender<Work>,
    updates: Receiver<Update>,
    status: Status,
    candidates: Vec<Candidate>,
    /// New readings to show, if they changed since they were last taken.
    new_readings: Option<Readings>,
    cue: Option<Window>,
    likely_cue: Option<Window>,
    /// Wait for each search instead of continuing, so a replay isn't ahead of its own searches.
    wait_for_searches: bool,
}

impl Session {
    pub fn new(wait_for_searches: bool, needs_box: bool) -> Self {
        let mut navi = NaviWatch::default();
        if !needs_box {
            navi.box_opened();
        }
        let (work, work_receiver) = channel();
        let (update_sender, updates) = channel();
        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || tracker_thread(work_receiver, update_sender))
            .expect("couldn't start the search thread");
        Self {
            normalizer: Normalizer::new(),
            cadence: Cadence::new(),
            detector: if wait_for_searches { Detector::new() } else { Detector::in_background() },
            clock: GameClock::default(),
            picture: vec![0; WIDTH * HEIGHT],
            textbox: TextboxWatch::default(),
            text: TextWatch::default(),
            reading_text: false,
            started_by_hand: false,
            needs_box,
            game_frame: -1,
            navi,
            frames_seen: 0,
            last_new_frame: None,
            work,
            updates,
            status: Status::Collecting(0),
            candidates: Vec::new(),
            new_readings: None,
            cue: None,
            likely_cue: None,
            wait_for_searches,
        }
    }

    pub fn fps(&self) -> f64 {
        self.clock.fps()
    }

    /// Where `seconds` falls in the read's game frames, if it's reading.
    pub fn frame_at(&self, seconds: f64) -> Option<f64> {
        self.clock.frame_at(seconds)
    }

    /// Replays wait for everything, so they come out the same every time. Live, video doesn't wait.
    fn new_detector(&self) -> Detector {
        let mut detector = if self.wait_for_searches { Detector::new() } else { Detector::in_background() };
        if self.reading_text || self.started_by_hand {
            detector.ignore_text_box();
        }
        detector
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

    /// Someone said flutters count from now: Navi has gone, or she isn't coming at all. Her draws break
    /// the blink model, so nothing before this counts. It doesn't take the Yes/No box: from some angles
    /// she never shows up, and his blinks are good during all of his text, once a text box has settled the camera. If the box is up, what it
    /// learned about the picture stays.
    pub fn start_now(&mut self) {
        if self.needs_box && !self.textbox.is_open() && !self.reading_text {
            self.started_by_hand = true;
            self.start_read();
            return;
        }
        let _ = self.work.send(Work::Reset);
        while self.updates.try_recv().is_ok() {}
        self.navi.mark_gone(self.game_frame + 1);
        self.status = Status::Collecting(0);
        self.candidates.clear();
        self.new_readings = Some(Readings::default());
        self.cue = None;
        self.likely_cue = None;
    }

    /// Seconds into the read that a text box leaving for `window` should leave, going by the clock.
    pub fn window_seconds(&self, window: &Window) -> f64 {
        let (first, last) = window.box_gone();
        self.clock.seconds_at((first + last) / 2)
    }

    /// Whether the feed turned out to be soft, so smaller changes count.
    pub fn soft_feed(&self) -> bool {
        self.detector.soft_feed()
    }

    /// The readings, if they changed since the last call.
    pub fn take_readings(&mut self) -> Option<Readings> {
        self.new_readings.take()
    }

    /// Reading was forced with no text box up, so it's reading whatever is on screen.
    pub fn started_by_hand(&self) -> bool {
        self.started_by_hand
    }

    /// Starts over: new talk, new camera angle.
    pub fn restart(&mut self) {
        self.started_by_hand = false;
        self.reading_text = self.text.is_open();
        let _ = self.work.send(Work::Reset);
        while self.updates.try_recv().is_ok() {}
        self.cadence = Cadence::new();
        self.detector = self.new_detector();
        self.status = Status::Collecting(0);
        self.candidates.clear();
        self.new_readings = Some(Readings::default());
        self.cue = None;
        self.likely_cue = None;
        self.game_frame = -1;
        self.navi.end_read();
        self.last_new_frame = None;
        self.clock = GameClock::default();
    }

    /// Starts over for a new box: nothing from before it counts.
    fn start_read(&mut self) {
        let _ = self.work.send(Work::Reset);
        while self.updates.try_recv().is_ok() {}
        self.detector = self.new_detector();
        self.status = Status::Collecting(0);
        self.candidates.clear();
        self.new_readings = Some(Readings::default());
        self.cue = None;
        self.likely_cue = None;
        self.game_frame = -1;
        self.navi.end_read();
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
        self.navi.frame(&self.picture);
        if self.needs_box
            && let Some(area) = self.normalizer.game_area()
        {
            let text = self.text.observe(frame, area);
            match self.textbox.observe(frame, chroma, area) {
                Some(Change::Opened) if self.started_by_hand || self.reading_text => {}
                Some(Change::Opened) => {
                    self.start_read();
                    self.navi.box_opened();
                    events.push(Event::BoxOpened);
                }
                Some(Change::Closed) => {
                    // It left on the first frame without it
                    let game_frame = self.game_frame + 1 - i64::from(FRAMES_TO_CLOSE - 1);
                    let heart_piece = (!self.candidates.is_empty()).then(|| self.would_roll_heart_piece(game_frame));
                    let seconds = self.clock.seconds_at(game_frame);
                    events.push(Event::BoxGone { game_frame, seconds, heart_piece });
                    self.started_by_hand = false;
                    self.reading_text = false;
                    self.start_read();
                    return;
                }
                None => {}
            }
            match text {
                // A real text box replaces a read forced before it: there's a talk to read now, and what
                // came before it could be anything, like a loading zone or walking up to him
                Some(Change::Opened) if !self.textbox.is_open() => {
                    self.started_by_hand = false;
                    self.reading_text = true;
                    self.start_read();
                    self.navi.box_opened();
                    events.push(Event::TextOpened);
                }
                Some(Change::Closed) if self.reading_text && !self.textbox.is_open() => {
                    self.reading_text = false;
                    self.start_read();
                    events.push(Event::TextGone);
                }
                _ => {}
            }
            if !self.textbox.is_open() && !self.started_by_hand && !self.reading_text {
                return;
            }
            // Nothing from while she's here is any use, including as a reference for the picture
            if self.navi.light_while_waiting() {
                self.start_read();
                self.navi.box_opened();
                events.push(Event::NaviSeen);
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

        let (info, flutters) = self.detector.push(&self.picture);
        if let Some(reread) = self.detector.take_reread()
            && let Some(counted) = self.navi.admit_reread(reread.through, &reread.flutters)
        {
            events.push(Event::ReadAgain { through: reread.through, flutters: counted.len() });
            let observations =
                counted.iter().map(|f| (Observation { start: f.start, count: f.count }, f.is_clean())).collect();
            let _ = self.work.send(Work::Replace(reread.through, observations, self.game_frame));
            self.wait_for_update(events);
        }
        if info.finished_learning {
            let seconds = seconds - self.clock.seconds_at(0);
            events.push(Event::FoundLookingBack { flutters: flutters.len(), seconds });
        }
        for flutter in flutters {
            events.push(Event::Flutter(flutter));
            if let Some(flutter) = self.navi.admit(flutter) {
                self.observe(flutter, events);
            }
        }
        if let Some((from, held)) = self.navi.advance(self.game_frame) {
            events.push(Event::NaviDone { from, held: held.len() });
            for flutter in held {
                self.observe(flutter, events);
            }
        }
        if let Some(window) = self.cue
            && window.box_gone().1 < self.game_frame
        {
            // That one went by: the next read of the tracker brings another
            self.cue = None;
        }
    }

    fn observe(&mut self, flutter: Flutter, events: &mut Vec<Event>) {
        let observation = Observation { start: flutter.start, count: flutter.count };
        let _ = self.work.send(Work::Observe(observation, flutter.is_clean(), self.game_frame));
        self.wait_for_update(events);
    }

    fn wait_for_update(&mut self, events: &mut Vec<Event>) {
        if self.wait_for_searches
            && let Ok(update) = self.updates.recv()
        {
            self.apply(update, events);
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
            let doubted = self.likely_cue.is_some_and(|window| still_ahead(&window, self.game_frame));
            if self.likely_cue.take().is_some() && doubted && (new_likely.is_some() || update.window.is_none()) {
                events.push(Event::LikelyCancelled);
            }
            if let Some((window, chance)) = update_likely {
                let (first, last) = window.box_gone();
                let box_frame = (first + last) / 2;
                self.likely_cue = Some(window);
                events.push(Event::LikelyCue { box_frame, seconds: self.clock.seconds_at(box_frame), window, chance });
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

    /// A different picture every time, with no colors in it, so never a Yes/No box.
    fn push_frame(session: &mut Session, frame: usize, events: &mut Vec<Event>) {
        let (width, height) = (640, 480);
        let mut luma = vec![60u8; width * height];
        for row in 100..200 {
            let x = 100 + frame * 3 % 300;
            luma[row * width + x..row * width + x + 50].fill(180);
        }
        let chroma = vec![128u8; width * height / 2];
        let picture = Luma { data: &luma, width, height, stride: width };
        session.push(picture, &chroma, frame as f64 / 20.0, events);
    }

    /// The same frame with a text box's darkening over it, and the Yes/No box's colors if asked for.
    fn push_text(session: &mut Session, frame: usize, yes_no: bool, events: &mut Vec<Event>) {
        let (width, height) = (640, 480);
        let mut luma = vec![60u8; width * height];
        for row in 100..200 {
            let x = 100 + frame * 3 % 300;
            luma[row * width + x..row * width + x + 50].fill(180);
        }
        for row in 287..422 {
            luma[row * width + 58..row * width + 590].fill(25);
        }
        let mut chroma = vec![128u8; width * height / 2];
        if yes_no {
            for (i, (y, u, v)) in [(60u8, 220u8, 110u8), (150, 70, 60), (70, 110, 220)].into_iter().enumerate() {
                let left = 80 + i * 150;
                for row in 320..360 {
                    luma[row * width + left..row * width + left + 100].fill(y);
                    for x in (left..left + 100).step_by(2) {
                        chroma[row / 2 * width + x] = u;
                        chroma[row / 2 * width + x + 1] = v;
                    }
                }
            }
        }
        let picture = Luma { data: &luma, width, height, stride: width };
        session.push(picture, &chroma, frame as f64 / 20.0, events);
    }

    /// Flutters from his first words on are what this is for, so the Yes/No box must not start over.
    #[test]
    fn reading_starts_from_his_first_text_and_goes_on_through_the_yes_no_box() {
        let mut session = Session::new(true, true);
        let mut events = Vec::new();
        for frame in 0..60 {
            push_frame(&mut session, frame, &mut events);
        }
        assert_eq!(session.game_frame, -1, "the control: no box, so nothing is read");
        for frame in 60..80 {
            push_text(&mut session, frame, false, &mut events);
        }
        assert!(matches!(events.iter().filter(|e| matches!(e, Event::TextOpened)).count(), 1));
        let before_yes_no = session.game_frame;
        assert!(before_yes_no >= 15, "{before_yes_no}");
        for frame in 80..90 {
            push_text(&mut session, frame, true, &mut events);
        }
        assert_eq!(session.game_frame, before_yes_no + 10, "the Yes/No box started it over");
        assert!(!events.iter().any(|e| matches!(e, Event::BoxOpened)));
    }

    /// Forcing a start in a loading zone or while walking up to him can't spoil the talk that follows.
    #[test]
    fn a_text_box_replaces_a_read_forced_before_it() {
        let mut session = Session::new(true, true);
        let mut events = Vec::new();
        for frame in 0..60 {
            push_frame(&mut session, frame, &mut events);
        }
        session.start_now();
        for frame in 60..90 {
            push_frame(&mut session, frame, &mut events);
        }
        assert!(session.started_by_hand());
        assert_eq!(session.game_frame, 29, "the control: forcing it reads with no box up");
        for frame in 90..100 {
            push_text(&mut session, frame, false, &mut events);
        }
        assert!(!session.started_by_hand());
        assert!(events.iter().any(|e| matches!(e, Event::TextOpened)));
        assert!(session.game_frame < 10, "it went on from before the box: {}", session.game_frame);
    }

    /// Her light starts the read over, and text keeps scrolling after that.
    #[test]
    fn the_text_box_stays_ignored_when_navi_starts_the_read_over() {
        let mut session = Session::new(true, true);
        let mut events = Vec::new();
        for frame in 0..60 {
            push_frame(&mut session, frame, &mut events);
        }
        for frame in 60..70 {
            push_text(&mut session, frame, false, &mut events);
        }
        assert!(session.detector.ignores_text_box(), "the control: reading from a text box");
        session.start_read();
        assert!(session.detector.ignores_text_box());
    }

    /// Talking to someone else, or leaving, can't leave a read going for the next talk.
    #[test]
    fn text_ending_with_no_yes_no_box_ends_the_read() {
        let mut session = Session::new(true, true);
        let mut events = Vec::new();
        for frame in 0..20 {
            push_frame(&mut session, frame, &mut events);
        }
        for frame in 20..40 {
            push_text(&mut session, frame, false, &mut events);
        }
        assert!(session.game_frame > 0, "the control: it was reading");
        // The edge flickering while the box is up isn't the text ending, and keeps the read
        let reading = session.game_frame;
        for frame in 40..50 {
            push_frame(&mut session, frame, &mut events);
        }
        assert_eq!(session.game_frame, reading + 10, "a flicker threw the read away");
        for frame in 50..100 {
            push_frame(&mut session, frame, &mut events);
        }
        assert_eq!(session.game_frame, -1);
        assert!(events.iter().any(|e| matches!(e, Event::TextGone)));
    }

    /// From some angles Navi never shows up, and his blinks count during all of his text.
    #[test]
    fn starting_by_hand_does_not_wait_for_the_box() {
        let mut session = Session::new(true, true);
        let mut events = Vec::new();
        for frame in 0..60 {
            push_frame(&mut session, frame, &mut events);
        }
        assert_eq!(session.game_frame, -1, "the control: no box, so nothing is read");
        session.start_now();
        for frame in 60..70 {
            push_frame(&mut session, frame, &mut events);
        }
        assert_eq!(session.game_frame, 9);
        session.restart();
        for frame in 70..80 {
            push_frame(&mut session, frame, &mut events);
        }
        assert_eq!(session.game_frame, -1, "starting over waits for the box again");
    }

    /// Its first frame being close is when a cue matters most. Dropping it then cancelled good cues
    /// in their last second.
    #[test]
    fn a_likely_window_stays_until_its_last_frame_has_gone() {
        let window = Window { first_roll: 200 + BOX_GONE_TO_ROLL, last_roll: 240 + BOX_GONE_TO_ROLL };
        assert!(still_ahead(&window, 150));
        assert!(still_ahead(&window, 199), "just before its first frame");
        assert!(still_ahead(&window, 220), "inside it");
        assert!(still_ahead(&window, 240), "on its last frame");
        assert!(!still_ahead(&window, 241));
    }

    #[test]
    fn clock_follows_the_measured_rate_not_the_default() {
        let mut clock = GameClock::default();
        for frame in 0..200 {
            clock.add(frame, 3.0 + frame as f64 / 18.0);
        }
        assert!((clock.fps() - 18.0).abs() < 1e-6, "{}", clock.fps());
        assert!((clock.seconds_at(400) - (3.0 + 400.0 / 18.0)).abs() < 1e-6);
        let frame = clock.frame_at(3.0 + 150.25 / 18.0).expect("it has frames");
        assert!((frame - 150.25).abs() < 1e-6, "{frame}");
        assert_eq!(GameClock::default().frame_at(1.0), None);
    }

    #[test]
    fn only_long_gaps_are_skipped_frames() {
        assert_eq!(skipped_frames(0.050), 0);
        assert_eq!(skipped_frames(0.067), 0);
        assert_eq!(skipped_frames(0.100), 1);
        assert_eq!(skipped_frames(0.150), 2);
    }
}
