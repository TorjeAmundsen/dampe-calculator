//! `dampe live`: reads a video device and tells you when to press A. It reads video and makes
//! sounds, nothing else.

use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use dampe_core::picture::Luma;
use dampe_core::session::{Event, Session};
use dampe_core::tracker::Status;

use crate::capture::{Capture, list_devices};
use crate::gui::{self, Gui, Tone};
use crate::sound::{self, Sound};
use crate::timing::{Platform, Timing};

const DEFAULT_DEVICE: &str = "OBS Virtual Camera";
/// How far from the countdown's zero the box can leave and still have been following it.
const CUE_FOLLOWED: f64 = 1.0;
/// The countdown's short tones come this many seconds before the long one.
const COUNTDOWN: [u64; 3] = [3, 2, 1];
const NO_FRAMES_WARNING: Duration = Duration::from_secs(2);

/// How long without a flutter before saying why.
const SILENCE_REPORT: Duration = Duration::from_secs(10);

/// Where a line of output goes: the terminal, and the window if there is one.
#[derive(Clone)]
struct Log {
    started: Instant,
    gui: Option<Gui>,
}

impl Log {
    /// A line with the time since the program started.
    fn line(&self, text: impl std::fmt::Display) {
        let elapsed = self.started.elapsed().as_secs_f64();
        let line = format!("[{:02}:{:04.1}] {text}", (elapsed / 60.0) as u64, elapsed % 60.0);
        println!("{line}");
        if let Some(gui) = &self.gui {
            gui.add_log(line);
        }
    }
}

/// Plays a countdown ending at `at`, if the sound is on, unless told to stop first.
fn start_countdown(at: Instant, sound: Arc<Sound>) -> Sender<()> {
    let (cancel, cancelled) = channel();
    std::thread::spawn(move || {
        for seconds in COUNTDOWN {
            let Some(when) = at.checked_sub(Duration::from_secs(seconds)) else { continue };
            let wait = when.saturating_duration_since(Instant::now());
            if !wait.is_zero() && cancelled.recv_timeout(wait).is_ok() {
                return;
            }
            sound.cue(sound::TICK);
        }
        let wait = at.saturating_duration_since(Instant::now());
        if wait.is_zero() || cancelled.recv_timeout(wait).is_err() {
            sound.cue(sound::GO);
        }
    });
    cancel
}

fn describe(status: &Status) -> String {
    match status {
        Status::Collecting(n) => format!("{n} flutters kept"),
        Status::Narrowing(n) => format!("{n} states still fit"),
        Status::Confirming(n) => format!("found a state, confirming: {n} predicted"),
        Status::Locked => "locked".to_owned(),
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut device = DEFAULT_DEVICE.to_owned();
    let timing = Arc::new(Timing::load());
    let mut window = true;
    let mut no_sound = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--device" => device = args.next().ok_or("--device needs a name")?.clone(),
            "--no-window" => window = false,
            "--no-sound" => no_sound = true,
            "--platform" => {
                let platform = args.next().and_then(|value| Platform::parse(value));
                timing.set_platform(platform.ok_or("--platform needs n64 or vc")?);
            }
            "--feed-delay-ms" => {
                let delay = args.next().and_then(|value| value.parse().ok());
                timing.fix_feed_delay(delay.ok_or("--feed-delay-ms needs a number")?);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }

    let devices = list_devices()?;
    if !devices.contains(&device) {
        return Err(format!("no device named '{device}'. Devices: {devices:?}"));
    }
    let capture = Capture::open(&device, true).map_err(|e| format!("couldn't open '{device}': {e}"))?;
    let description = format!("{device}: {}x{}", capture.size.0, capture.size.1);
    println!("{description}");
    println!("Open the Yes/No box, wait for Navi to settle, then don't touch anything until the cue.");
    println!("Press Enter here or Start over in the window after a new talk, or a new camera angle.");

    let sound = Arc::new(Sound::load());
    if !window {
        // Without a window, sound is the only cue there is
        if !no_sound {
            sound.force_on();
        }
        return read(capture, &device, timing, None, Arc::default(), sound);
    }
    let window_sound = sound.clone();
    let window_timing = timing.clone();
    gui::run(description, window_sound, window_timing, move |gui, start_over| {
        if let Err(error) = read(capture, &device, timing, Some(gui), start_over, sound) {
            eprintln!("{error}");
            let _ = slint::quit_event_loop();
        }
    })
}

/// Reads until the device fails.
fn read(
    capture: Capture,
    device: &str,
    timing: Arc<Timing>,
    gui: Option<Gui>,
    start_over: Arc<AtomicBool>,
    sound: Arc<Sound>,
) -> Result<(), String> {
    let (restart_sender, restart) = channel();
    std::thread::spawn(move || {
        for _ in std::io::stdin().lock().lines() {
            if restart_sender.send(()).is_err() {
                return;
            }
        }
    });

    let started = Instant::now();
    let log = Log { started, gui: gui.clone() };
    let mut session = Session::new(false, true);
    let mut shown_frames = 0;
    let mut events = Vec::new();
    let mut countdown: Option<Sender<()>> = None;
    let mut flutters = 0;
    let mut cue_at: Option<f64> = None;
    let mut warned_placeholder = false;
    let mut last_dropped = 0;
    let mut last_frame = Instant::now();
    let mut last_report = Instant::now();

    loop {
        if restart.try_recv().is_ok() || start_over.swap(false, Ordering::Relaxed) {
            if let Some(cancel) = countdown.take() {
                let _ = cancel.send(());
            }
            cue_at = None;
            flutters = 0;
            session.restart();
            if let Some(gui) = &gui {
                gui.set_cue(None);
                gui.set_status("Starting over".to_owned(), Tone::Reading);
                gui.set_latest(String::new());
            }
            log.line(format_args!("Starting over."));
        }
        let frame = match capture.frames.recv_timeout(Duration::from_millis(250)) {
            Ok(frame) => frame,
            Err(_) => {
                if capture.state.failed.load(Ordering::Relaxed) {
                    return Err(format!("'{device}' stopped working"));
                }
                if capture.state.placeholder.swap(false, Ordering::Relaxed) && !warned_placeholder {
                    warned_placeholder = true;
                    log.line(format_args!(
                        "'{device}' is connected but not started. In OBS, click Start Virtual Camera."
                    ));
                } else if last_frame.elapsed() > NO_FRAMES_WARNING {
                    log.line(format_args!("No frames from '{device}'."));
                    last_frame = Instant::now();
                }
                continue;
            }
        };
        last_frame = Instant::now();
        let dropped = capture.state.dropped.load(Ordering::Relaxed);
        if dropped != last_dropped {
            log.line(format_args!(
                "Warning: {} frames dropped, this computer is falling behind.",
                dropped - last_dropped
            ));
            last_dropped = dropped;
        }

        let seconds = frame.arrival.duration_since(started).as_secs_f64();
        let luma = Luma { data: &frame.luma, width: frame.width, height: frame.height, stride: frame.width };
        session.push(luma, &frame.chroma, seconds, &mut events);
        if let Some(gui) = &gui
            && session.frames_seen() != shown_frames
        {
            shown_frames = session.frames_seen();
            if let Some(area) = session.game_area() {
                gui.show_frame(&frame, area, session.blobs().collect(), session.expected_eye());
            }
        }

        if last_report.elapsed() >= SILENCE_REPORT {
            last_report = Instant::now();
            let health = session.take_health();
            log.line(format_args!(
                "no flutter for {} s: {} game frames, {} with a bad reference, {} with blobs, {} tracks rejected, {} \
                 tracks open",
                SILENCE_REPORT.as_secs(),
                health.game_frames,
                health.bad_reference,
                health.with_blobs,
                health.rejected_tracks,
                health.tracks,
            ));
        }
        for event in events.drain(..) {
            match event {
                Event::Flutter(flutter) => {
                    if let Some(gui) = &gui {
                        gui.set_latest(format!(
                            "Flutter {}: {} blinks, fit {:.2}",
                            flutters + 1,
                            flutter.count,
                            flutter.fit
                        ));
                    }
                    last_report = Instant::now();
                    session.take_health();
                    flutters += 1;
                    log.line(format_args!("flutter {flutters}: {} blinks, fit {:.2}", flutter.count, flutter.fit));
                }
                // Each flutter has its own line, and this count stops rising when old ones age out
                Event::Skipped { frames, gap } => {
                    log.line(format_args!("Counted {frames} skipped game frames from a {:.0} ms gap.", gap * 1000.0));
                }
                Event::BoxOpened => {
                    log.line("The Yes/No box is up, reading.");
                    flutters = 0;
                    last_report = Instant::now();
                    if let Some(gui) = &gui {
                        gui.set_status("Reading flutters".to_owned(), Tone::Reading);
                        gui.set_latest(String::new());
                    }
                }
                Event::Status(Status::Collecting(_)) => {}
                Event::Status(status) => {
                    log.line(format_args!("{}", describe(&status)));
                    if let Some(gui) = &gui {
                        let tone = if status == Status::Locked { Tone::Locked } else { Tone::Reading };
                        gui.set_status(describe(&status), tone);
                    }
                }
                Event::Cue { seconds: box_seconds, window, .. } => {
                    let at_seconds = box_seconds - timing.lead_ms() / 1000.0;
                    let at = started + Duration::from_secs_f64(at_seconds.max(0.0));
                    let frames = window.last_roll - window.first_roll + 1;
                    let margin = f64::from(frames as i32) / session.fps() / 2.0 * 1000.0;
                    log.line(format_args!(
                        "Press A when the countdown reaches zero, in {:.1} s. Safe within +-{margin:.0} ms.",
                        at_seconds - seconds
                    ));
                    if let Some(cancel) = countdown.replace(start_countdown(at, sound.clone())) {
                        let _ = cancel.send(());
                    }
                    cue_at = Some(at_seconds);
                    if let Some(gui) = &gui {
                        gui.set_cue(Some(at));
                    }
                }
                Event::CueCancelled => {
                    if let Some(gui) = &gui {
                        gui.set_cue(None);
                        gui.set_status("Cancelled: don't press yet".to_owned(), Tone::Warning);
                    }
                    if let Some(cancel) = countdown.take() {
                        let _ = cancel.send(());
                    }
                    cue_at = None;
                    log.line(format_args!("Cancelled: the video stopped matching. Don't press yet."));
                }
                Event::BoxGone { seconds: gone, heart_piece, .. } => {
                    if let Some(gui) = &gui {
                        gui.set_cue(None);
                        gui.set_status("Waiting for the Yes/No box".to_owned(), Tone::Reading);
                    }
                    if let Some(cancel) = countdown.take() {
                        let _ = cancel.send(());
                    }
                    if let Some(cue) = cue_at.take() {
                        let after_zero = gone - cue;
                        if after_zero.abs() < CUE_FOLLOWED && timing.learn(after_zero * 1000.0) {
                            log.line(format_args!(
                                "The box left {:.0} ms after zero. {}",
                                after_zero * 1000.0,
                                timing.describe()
                            ));
                            if let Some(gui) = &gui {
                                gui.set_timing(timing.describe());
                            }
                        } else {
                            log.line(format_args!(
                                "The box left {:.0} ms after zero, too far off to measure from.",
                                after_zero * 1000.0
                            ));
                        }
                    }
                    match heart_piece {
                        Some(true) => log.line(format_args!("That press should give the Heart Piece.")),
                        Some(false) => log.line(format_args!("That press shouldn't give the Heart Piece.")),
                        None => log.line(format_args!("The box left before a state was found.")),
                    }
                    flutters = 0;
                    log.line("Waiting for the next Yes/No box.");
                }
            }
        }
    }
}
