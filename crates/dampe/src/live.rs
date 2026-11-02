//! `dampe live`: reads a video device and tells you when to press A. It reads video and makes
//! sounds, nothing else.

use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use dampe_core::picture::Luma;
use dampe_core::reading::{Reading, Readings, Status};
use dampe_core::session::{Event, Session};

use crate::capture::{Capture, list_devices};
use crate::cues::{Log, start_countdown};
use crate::gui::{self, Gui, Tone};
use crate::hotkey;
use crate::logfile;
use crate::manuallive;
use crate::presslog::PressLog;
use crate::sound::Sound;
use crate::timing::{Platform, Timing};

/// How often the times in the list of readings count down. Fast enough to press by.
const READINGS_REFRESH: Duration = Duration::from_millis(100);
const DEFAULT_DEVICE: &str = "OBS Virtual Camera";
const DEFAULT_NAVI_KEY: &str = "F8";
/// How far from the countdown's zero the box can leave and still have been following it.
const CUE_FOLLOWED: f64 = 1.0;
const NO_FRAMES_WARNING: Duration = Duration::from_secs(2);

/// How long without a flutter before saying why.
const SILENCE_REPORT: Duration = Duration::from_secs(10);

fn describe(status: &Status) -> String {
    match status {
        Status::Collecting(n) => format!("{n} flutters kept"),
        Status::Narrowing(n) => format!("{n} states still fit"),
        Status::Confirming(n) => format!("found a state, confirming: {n} predicted"),
        Status::Locked => "locked".to_owned(),
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "--manual") {
        return manuallive::run(args);
    }
    logfile::start();
    let mut device = DEFAULT_DEVICE.to_owned();
    let timing = Arc::new(Timing::load());
    let mut window = true;
    let mut no_sound = false;
    let mut navi_key = DEFAULT_NAVI_KEY.to_owned();
    let mut press_log_path = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--device" => device = args.next().ok_or("--device needs a name")?.clone(),
            "--no-window" => window = false,
            "--no-sound" => no_sound = true,
            "--navi-key" => navi_key = args.next().ok_or("--navi-key needs a key")?.to_ascii_uppercase(),
            "--log-presses" => press_log_path = Some(args.next().ok_or("--log-presses needs a file")?.clone()),
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
    logfile::line(&description);
    logfile::line("Talk to Dampe: it starts reading from his first text box and waits for Navi by itself.");
    if cfg!(not(feature = "windowed")) {
        logfile::line("Press Enter here or Start over in the window after a new talk, or a new camera angle.");
    }

    let key = hotkey::parse_key(&navi_key).ok_or("--navi-key needs a function key, F1 to F24")?;
    let navi_gone = Arc::new(AtomicBool::new(false));
    let failed_key = navi_key.clone();
    hotkey::listen(key, navi_gone.clone(), move || {
        let error = format!("Couldn't use {failed_key} as the Navi key: another program has it. Try --navi-key.");
        logfile::line(&error);
        crate::show_error(&error);
    });
    logfile::line(format_args!(
        "If the box opened right away because you've talked to Dampe before, it waits for Navi by itself. {navi_key} forces a start, and is only for when it doesn't start or doesn't see Navi finish."
    ));
    let (press_sender, press_log) = match press_log_path {
        Some(_) if !window => return Err("--log-presses takes the window, which hears the keys".to_owned()),
        Some(path) => {
            let (sender, presses) = channel();
            let press_log = PressLog::open(&path, presses, &timing.describe())?;
            logfile::line(format_args!(
                "Logging presses to {path}: keys 3 and 4 with the window focused. Pressing either again right away flips the count."
            ));
            (Some(sender), Some(press_log))
        }
        None => (None, None),
    };
    let sound = Arc::new(Sound::load());
    if !window {
        // Without a window, sound is the only cue there is
        if !no_sound {
            sound.force_on();
        }
        return read(capture, &device, timing, None, Arc::default(), navi_gone, sound, press_log);
    }
    let window_sound = sound.clone();
    let window_timing = timing.clone();
    let window_navi_gone = navi_gone.clone();
    let options = gui::Options {
        source: description,
        sound: window_sound,
        timing: window_timing,
        hotkey_pressed: window_navi_gone,
        hotkey_name: navi_key,
        presses: press_sender,
        manual: None,
    };
    gui::run(options, move |gui, start_over| {
        if let Err(error) = read(capture, &device, timing, Some(gui), start_over, navi_gone, sound, press_log) {
            logfile::to_file(&error);
            if !crate::show_error(&error) {
                eprintln!("{error}");
            }
            let _ = slint::quit_event_loop();
        }
    })
}

/// Reads until the device fails.
#[allow(clippy::too_many_arguments)]
fn read(
    capture: Capture,
    device: &str,
    timing: Arc<Timing>,
    gui: Option<Gui>,
    start_over: Arc<AtomicBool>,
    navi_gone: Arc<AtomicBool>,
    sound: Arc<Sound>,
    mut press_log: Option<PressLog>,
) -> Result<(), String> {
    let (restart_sender, restart) = channel();
    // Without a console of its own, the terminal's input belongs to the shell it was started from
    if cfg!(not(feature = "windowed")) {
        std::thread::spawn(move || {
            for _ in std::io::stdin().lock().lines() {
                if restart_sender.send(()).is_err() {
                    return;
                }
            }
        });
    }

    let started = Instant::now();
    let log = Log { started, gui: gui.clone() };
    let mut session = Session::new(false, true);
    let mut readings_shown = Readings::default();
    let mut last_readings = Instant::now();
    let mut shown_frames = 0;
    let mut events = Vec::new();
    let mut countdown: Option<Sender<()>> = None;
    let mut flutters = 0;
    let mut cue_at: Option<f64> = None;
    let mut warned_placeholder = false;
    let mut last_dropped = 0;
    let mut last_frame = Instant::now();
    let mut last_report = Instant::now();
    let mut eye_found = false;
    let mut shown_percent = 0;

    loop {
        if restart.try_recv().is_ok() || start_over.swap(false, Ordering::Relaxed) {
            if let Some(cancel) = countdown.take() {
                let _ = cancel.send(());
            }
            cue_at = None;
            flutters = 0;
            eye_found = false;
            session.restart();
            if let Some(press_log) = &mut press_log {
                press_log.read_started();
            }
            if let Some(gui) = &gui {
                gui.set_cue(None);
                gui.set_likely(None);
                gui.set_status("Starting over".to_owned(), Tone::Reading);
                gui.set_latest(String::new());
            }
            log.line(format_args!("Starting over."));
        }
        if navi_gone.swap(false, Ordering::Relaxed) {
            if let Some(cancel) = countdown.take() {
                let _ = cancel.send(());
            }
            cue_at = None;
            flutters = 0;
            session.start_now();
            if let Some(press_log) = &mut press_log {
                press_log.read_started();
            }
            if let Some(gui) = &gui {
                gui.set_cue(None);
                gui.set_likely(None);
                gui.set_latest(String::new());
                let status = if session.started_by_hand() {
                    "Forced start with no text box up: reading whatever is on screen"
                } else {
                    "Forced start: counting flutters from now"
                };
                gui.set_status(status.to_owned(), Tone::Reading);
            }
            if session.started_by_hand() {
                log.line("Forced start with no text box up. It reads whatever is on screen until a text box opens.");
            } else {
                log.line("Forced start: only flutters from now on count.");
            }
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
        // Before this frame, which might start a new read with new frame numbers
        if let Some(press_log) = &mut press_log {
            for said in press_log.take_presses(started, |at| session.frame_at(at)) {
                log.line(said);
            }
        }
        let luma = Luma { data: &frame.luma, width: frame.width, height: frame.height, stride: frame.width };
        session.push(luma, &frame.chroma, seconds, &mut events);
        let new_readings = session.take_readings();
        let changed = new_readings.is_some();
        if let Some(readings) = new_readings {
            if let (Some(press_log), Some(top)) = (&mut press_log, readings.top.first()) {
                press_log.top_reading(top);
            }
            readings_shown = readings;
        }
        if let Some(gui) = &gui
            && (changed || (!readings_shown.top.is_empty() && last_readings.elapsed() >= READINGS_REFRESH))
        {
            last_readings = Instant::now();
            // Until the moment to press, like the countdown
            let lead = timing.lead_ms() / 1000.0;
            let left = |reading: &Reading| reading.next_window.map(|w| session.window_seconds(&w) - lead - seconds);
            gui.set_readings(&readings_shown, left);
        }
        if let Some(gui) = &gui
            && session.frames_seen() != shown_frames
        {
            shown_frames = session.frames_seen();
            if let Some(area) = session.game_area() {
                gui.show_frame(&frame, area, session.blobs().collect(), session.expected_eye());
            }
        }

        if !eye_found && session.expected_eye().is_some() {
            eye_found = true;
            log.line(format_args!("Found the eye, {flutters} flutters so far."));
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
            if let Some(press_log) = &mut press_log {
                press_log.event(&event);
            }
            match event {
                Event::NaviDone { held, .. } => {
                    log.line(format_args!("Navi looks done. Counting from then: {held} flutters so far."));
                    if let Some(gui) = &gui {
                        gui.set_status("Reading flutters".to_owned(), Tone::Reading);
                    }
                }
                Event::ReadAgain { flutters: found, .. } => {
                    log.line(format_args!("Read the early frames again knowing where the eye is: {found} flutters."));
                }
                Event::FoundLookingBack { flutters: found, seconds } => {
                    if session.soft_feed() {
                        log.line("The feed looks soft, so smaller changes count as blinks.");
                    }
                    log.line(format_args!(
                        "Ready after {seconds:.1} s of the box. Found {found} flutters from before that, looking back."
                    ));
                }
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
                Event::NaviSeen => {}
                Event::TextOpened => {
                    eye_found = false;
                    log.line("A text box is up, reading.");
                    flutters = 0;
                    last_report = Instant::now();
                    if let Some(gui) = &gui {
                        gui.set_status("Reading flutters".to_owned(), Tone::Reading);
                        gui.set_latest(String::new());
                    }
                }
                Event::TextGone => {
                    flutters = 0;
                    log.line("The text ended with no Yes/No box. Waiting for the next text box.");
                    if let Some(gui) = &gui {
                        gui.set_cue(None);
                        gui.set_likely(None);
                        gui.set_status(
                            "Talk to Dampe: reading starts by itself at his first text box".to_owned(),
                            Tone::Reading,
                        );
                    }
                    if let Some(cancel) = countdown.take() {
                        let _ = cancel.send(());
                    }
                }
                Event::BoxOpened => {
                    eye_found = false;
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
                        gui.set_likely(None);
                    }
                }
                Event::LikelyCue { seconds: box_seconds, chance, .. } => {
                    let at_seconds = box_seconds - timing.lead_ms() / 1000.0;
                    let at = started + Duration::from_secs_f64(at_seconds.max(0.0));
                    let percent = ((chance * 100.0).round() as u32).min(99);
                    log.line(format_args!(
                        "{percent}% chance: press A at zero, in {:.1} s. No state is proven yet.",
                        at_seconds - seconds
                    ));
                    if let Some(cancel) = countdown.replace(start_countdown(at, sound.clone())) {
                        let _ = cancel.send(());
                    }
                    cue_at = Some(at_seconds);
                    shown_percent = percent;
                    if let Some(gui) = &gui {
                        gui.set_cue(Some(at));
                        gui.set_likely(Some(percent));
                        gui.set_status(format!("{percent}% chance"), Tone::Warning);
                    }
                }
                Event::LikelyChance { chance } => {
                    let percent = ((chance * 100.0).round() as u32).min(99);
                    if percent != shown_percent {
                        shown_percent = percent;
                        log.line(format_args!("That press is now a {percent}% chance."));
                        if let Some(gui) = &gui {
                            gui.set_likely(Some(percent));
                            gui.set_status(format!("{percent}% chance"), Tone::Warning);
                        }
                    }
                }
                Event::LikelyCancelled => {
                    if let Some(cancel) = countdown.take() {
                        let _ = cancel.send(());
                    }
                    cue_at = None;
                    log.line("That likely press isn't likely any more. Don't press yet.");
                    if let Some(gui) = &gui {
                        gui.set_cue(None);
                        gui.set_likely(None);
                        gui.set_status("Reading flutters".to_owned(), Tone::Reading);
                    }
                }
                Event::CueCancelled => {
                    if let Some(gui) = &gui {
                        gui.set_cue(None);
                        gui.set_likely(None);
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
                        gui.set_likely(None);
                        gui.set_status(
                            "Talk to Dampe: reading starts by itself at his first text box".to_owned(),
                            Tone::Reading,
                        );
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
                    log.line("Waiting for the next text box.");
                }
            }
        }
    }
}
