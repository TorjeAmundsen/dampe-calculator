//! Runs a recording through the pipeline, and scores it against RNG watch values when given them.
//!
//! Frames come from an `ffmpeg` pipe, so the same code path sees what a capture device would give.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::process::{Command, Stdio};

use dampe_core::detector::{Cadence, Detector, Flutter};
use dampe_core::picture::{HEIGHT, Luma, Normalizer, WIDTH};
use dampe_core::rng::{self, StateWalk, flutter_len, rolls_heart_piece};
use dampe_core::session::{Event, Session};
use dampe_core::solver::Observation;
use dampe_core::textbox::{Change, TextboxWatch};
use dampe_core::tracker::Tracker;

/// The RNG watch's state per game frame, in the recording's own game frames.
struct Truth {
    /// `(video frame, state)`, one per game frame.
    frames: Vec<(u64, u32)>,
    by_video: HashMap<u64, usize>,
}

impl Truth {
    fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let mut frames = Vec::new();
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let (video, state) = line.split_once(' ').ok_or_else(|| format!("{path}: bad line {line}"))?;
            let video = video.parse().map_err(|e| format!("{path}: {e}"))?;
            let state = u32::from_str_radix(state, 16).map_err(|e| format!("{path}: {e}"))?;
            frames.push((video, state & (rng::STATES - 1)));
        }
        let by_video = frames.iter().enumerate().map(|(i, &(video, _))| (video, i)).collect();
        Ok(Self { frames, by_video })
    }

    /// Draws between two consecutive game frames, `None` past the end or over a burst.
    fn draws_on(&self, index: usize) -> Option<u32> {
        let previous = self.frames.get(index.checked_sub(1)?)?.1;
        let target = self.frames.get(index)?.1;
        let mut state = previous;
        for draws in 0..200 {
            if state == target {
                return Some(draws);
            }
            state = rng::next(state);
        }
        None
    }

    /// The flutters the watch shows: each Dampe renewal draws the wait and the count.
    fn flutters(&self) -> BTreeMap<usize, u8> {
        let mut flutters = BTreeMap::new();
        for index in 1..self.frames.len() {
            let Some(draws) = self.draws_on(index) else { continue };
            if draws != 2 && draws != 3 {
                continue;
            }
            let wait_state = {
                let mut state = self.frames[index - 1].1;
                for _ in 0..draws - 1 {
                    state = rng::next(state);
                }
                state
            };
            let count = if self.frames[index].1 as f32 / rng::STATES as f32 > 0.5 { 4 } else { 3 };
            let wait = (wait_state as f32 / rng::STATES as f32 * 30.0) as usize + 30;
            let start = index + wait;
            // A real flutter ends with the next renewal, which a burst of effects doesn't
            let next_renewal = start + flutter_len(count) as usize;
            if self.draws_on(next_renewal).is_some_and(|draws| draws == 2 || draws == 3) {
                flutters.insert(start, count);
            }
        }
        flutters
    }
}

#[derive(Default)]
struct Score {
    locks: usize,
    states_right: usize,
    states_wrong: usize,
    window_frames: u32,
    window_frames_right: u32,
    windows: usize,
    windows_right: usize,
    flutters_to_lock: Vec<usize>,
    /// The game frames locks came on, counted from when reading started.
    lock_frames: Vec<i64>,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let (mut video, mut truth_path) = (None, None);
    let mut session = false;
    let mut needs_box = true;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--truth" {
            truth_path = args.next().cloned();
        } else if arg == "--session" {
            session = true;
        } else if arg == "--no-box" {
            needs_box = false;
        } else {
            video = Some(arg.clone());
        }
    }
    let video = video.ok_or("no video given")?;
    let truth = truth_path.as_deref().map(Truth::load).transpose()?;
    let (width, height) = video_size(&video)?;

    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-i", &video, "-f", "rawvideo", "-pix_fmt", "nv12", "-"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start ffmpeg: {e}"))?;
    let mut pipe = child.stdout.take().ok_or("ffmpeg has no output")?;
    if session {
        run_session(&mut pipe, width, height, needs_box);
        let _ = child.wait();
        return Ok(());
    }

    let mut raw = vec![0u8; width * height * 3 / 2];
    let mut picture = vec![0u8; WIDTH * HEIGHT];
    let mut normalizer = Normalizer::new();
    let mut cadence = Cadence::new();
    let mut textbox = TextboxWatch::default();
    let mut detector = Detector::new();
    let mut tracker = Tracker::new();
    let mut seen = Vec::<(i64, Flutter)>::new();
    let mut score = Score::default();
    let mut flutters_since_reset = 0;
    // Game frame in the detector's count, to the truth's index
    let mut game_to_truth = Vec::<Option<usize>>::new();
    let mut video_frame = 0u64;
    let mut eye_errors = Vec::new();
    let mut eye_snaps: Vec<f32> = Vec::new();

    while pipe.read_exact(&mut raw).is_ok() {
        let (luma, chroma) = raw.split_at(width * height);
        let frame = Luma { data: luma, width, height, stride: width };
        let current = video_frame;
        video_frame += 1;
        if !cadence.is_new_frame(frame) || !normalizer.process(frame, &mut picture) {
            continue;
        }
        if needs_box && let Some(area) = normalizer.game_area() {
            match textbox.observe(frame, chroma, area) {
                Some(Change::Opened) => println!("box opened at video frame {current}"),
                Some(Change::Closed) => println!("box closed at video frame {current}"),
                None => {}
            }
            if !textbox.is_open() {
                continue;
            }
        }
        game_to_truth.push(truth.as_ref().and_then(|truth| truth.by_video.get(&current).copied()));
        let (_, flutters) = detector.push(&picture);
        if let Some(snap) = detector.last_eye_snap() {
            eye_snaps.push(snap);
        }
        for flutter in flutters {
            if flutter.is_clean()
                && let Some(error) = detector.last_eye_error()
            {
                eye_errors.push(error);
            }
            seen.push((current as i64, flutter));
            flutters_since_reset += 1;
            tracker.add(Observation { start: flutter.start, count: flutter.count }, flutter.is_clean());
            tracker.update(game_to_truth.len() as i64 - 1);
            if tracker.is_locked() {
                score.locks += 1;
                score.flutters_to_lock.push(flutters_since_reset);
                score.lock_frames.push(game_to_truth.len() as i64 - 1);
                if let Some(truth) = &truth {
                    check_lock(&tracker, truth, &game_to_truth, flutter.start, &mut score);
                }
                tracker.reset();
                flutters_since_reset = 0;
            }
        }
    }
    let _ = child.wait();

    println!("video frames {video_frame}, game frames {}, flutters read {}", game_to_truth.len(), seen.len());
    let health = detector.take_health();
    println!("detector: {health:?}, region {:?}", detector.region());
    eye_snaps.sort_by(f32::total_cmp);
    if let Some(worst) = eye_snaps.last() {
        println!("eye snap per update, px: median {:.1}, worst {worst:.1}", eye_snaps[eye_snaps.len() / 2]);
    }
    eye_errors.sort_by(f32::total_cmp);
    if let (Some(median), Some(worst)) = (eye_errors.get(eye_errors.len() / 2), eye_errors.last()) {
        let ninety = eye_errors[eye_errors.len() * 9 / 10];
        let close = eye_errors.iter().filter(|&&error| error <= 5.0).count();
        println!(
            "eye curve error before each clean flutter, px: median {median:.1}, 90% {ninety:.1}, worst {worst:.1}; {close} of {} within 5",
            eye_errors.len()
        );
    }
    if let Some(truth) = &truth {
        report_flutters(truth, &game_to_truth, &seen);
    }
    println!(
        "locks {}: state right {}, wrong {}; flutters per lock {:?}",
        score.locks, score.states_right, score.states_wrong, score.flutters_to_lock
    );
    let seconds: Vec<f64> =
        score.lock_frames.iter().map(|&frame| (frame as f64 / 19.98 * 10.0).round() / 10.0).collect();
    println!("seconds from the start of reading to each lock: {seconds:?}");
    if truth.is_some() {
        println!(
            "windows {}, every frame right {}; window frames right {}/{}",
            score.windows, score.windows_right, score.window_frames_right, score.window_frames
        );
    }
    Ok(())
}

/// Runs the recording the way the live app does, with video frames as its clock.
fn run_session(pipe: &mut impl Read, width: usize, height: usize, needs_box: bool) {
    let mut raw = vec![0u8; width * height * 3 / 2];
    let mut session = Session::new(true, needs_box);
    let mut events = Vec::new();
    let mut video_frame = 0u64;
    while pipe.read_exact(&mut raw).is_ok() {
        let (luma, chroma) = raw.split_at(width * height);
        let frame = Luma { data: luma, width, height, stride: width };
        let seen = session.frames_seen();
        session.push(frame, chroma, video_frame as f64 * 1001.0 / 60000.0, &mut events);
        if session.frames_seen() != seen
            && std::env::var_os("DAMPE_EYE_LOG").is_some()
            && let Some(eye) = session.expected_eye()
        {
            let blob = session.blobs().find(|blob| blob.overlaps(&eye));
            println!("eye {video_frame} {} {} {} {} {}", eye.left, eye.top, eye.right, eye.bottom, blob.is_some());
        }
        video_frame += 1;
        for event in events.drain(..) {
            if !matches!(event, Event::Flutter(_)) {
                println!("video frame {video_frame}: {event:?}");
            }
        }
    }
    println!("game frames per second: {:.3}", session.fps());
}

/// Checks a lock against the watch: the predicted state on every following frame, up to the next
/// burst of draws from something that isn't blinks, and then the Heart Piece window.
fn check_lock(tracker: &Tracker, truth: &Truth, game_to_truth: &[Option<usize>], last_start: i64, score: &mut Score) {
    let Some(&Some(anchor)) = game_to_truth.get(last_start as usize) else { return };
    let game_to_index = anchor as i64 - last_start;
    let now = game_to_truth.len() as i64 - 1;

    // The first frame the watch stops being blinks
    let stop = ((now + game_to_index + 1) as usize..truth.frames.len())
        .find(|&index| truth.draws_on(index).is_none_or(|draws| draws > 3))
        .unwrap_or(truth.frames.len());

    // Readings that differ only in Link's phase can stay until his next blink tells them apart, so
    // a lock is right when one of them matches the watch all the way
    let mut any_right = false;
    let mut compared = false;
    for candidate in tracker.candidates() {
        let mut walk = StateWalk::new(candidate.blinks());
        let frames = (now + game_to_index + 1) as usize..stop;
        compared |= !frames.is_empty();
        if frames.into_iter().all(|index| {
            let game = index as i64 - game_to_index;
            walk.state_after(game - candidate.offset) == truth.frames[index].1
        }) {
            any_right = true;
        }
    }
    if !compared {
        return;
    }
    if any_right {
        score.states_right += 1;
    } else {
        score.states_wrong += 1;
    }

    // A window is right when digging on each of its frames really would roll the Heart Piece,
    // judged only from the watch's values
    let last_checkable = stop as i64 - game_to_index;
    if let Some(window) = tracker.next_window(now + 40, 13) {
        if window.last_roll >= last_checkable {
            return;
        }
        score.windows += 1;
        let mut all = true;
        for frame in window.first_roll..=window.last_roll {
            let index = (frame + game_to_index) as usize;
            let mut state = truth.frames[index - 1].1;
            // Link draws before the dig on the same frame, Dampe after it
            let draws = truth.draws_on(index).unwrap_or(0);
            if draws == 1 || draws == 3 {
                state = rng::next(state);
            }
            score.window_frames += 1;
            if rolls_heart_piece(state) {
                score.window_frames_right += 1;
            } else {
                all = false;
            }
        }
        if all {
            score.windows_right += 1;
        }
    }
}

fn report_flutters(truth: &Truth, game_to_truth: &[Option<usize>], seen: &[(i64, Flutter)]) {
    let flutters = truth.flutters();
    let mut read = BTreeMap::new();
    for (_, flutter) in seen {
        if let Some(Some(index)) = game_to_truth.get(flutter.start as usize) {
            read.insert(*index, flutter.count);
        }
    }
    // Scored from the first flutter the detector could have seen to the last blink before the dig
    let first = read.keys().next().copied().unwrap_or(0);
    let scored: Vec<_> = flutters.iter().filter(|&(&start, _)| start >= first).collect();
    let exact = scored.iter().filter(|&&(start, count)| read.get(start) == Some(count)).count();
    let wrong_count = scored.iter().filter(|&&(start, count)| read.get(start).is_some_and(|c| c != count)).count();
    let shifted = scored
        .iter()
        .filter(|&&(start, _)| {
            !read.contains_key(start)
                && (1..=3).any(|d| read.contains_key(&(start - d)) || read.contains_key(&(start + d)))
        })
        .count();
    let missed = scored.len() - exact - wrong_count - shifted;
    let extra = read
        .keys()
        .filter(|&&start| start <= scored.last().map_or(0, |&(&s, _)| s))
        .filter(|&&start| (0..=3).all(|d| !flutters.contains_key(&(start + d)) && !flutters.contains_key(&(start - d))))
        .count();
    println!(
        "flutters in truth {}: exact {exact}, wrong count {wrong_count}, wrong start {shifted}, missed {missed}, extra {extra}",
        scored.len()
    );
}

fn video_size(video: &str) -> Result<(usize, usize), String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            video,
        ])
        .output()
        .map_err(|e| format!("couldn't start ffprobe: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (width, height) = text.trim().split_once(',').ok_or_else(|| format!("ffprobe said: {text}"))?;
    Ok((
        width.trim().parse().map_err(|e| format!("width: {e}"))?,
        height.trim().parse().map_err(|e| format!("height: {e}"))?,
    ))
}
