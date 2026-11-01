//! `dampe live --manual`: no video at all. Flutters are pressed by hand in the window, and it says when
//! to press A. Nothing here reads a device, and nothing is ever sent to the game.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use dampe_core::manual::{DEFAULT_LAG_MS, GAME_FPS, ManualSession};
use dampe_core::session::Event;
use dampe_core::tracker::{Reading, Readings, Status};

use crate::gui::{self, Gui, Tone};
use crate::hotkey::{self, CountKeys};
use crate::live::{Log, start_countdown};
use crate::logfile;
use crate::sound::Sound;
use crate::timing::{Platform, Timing};

const DEFAULT_START_OVER_KEY: &str = "F8";
/// How often to look for what's new while waiting for a press.
const TICK: Duration = Duration::from_millis(20);
/// How often the times in the list of readings count down. Fast enough to press by.
const READINGS_REFRESH: Duration = Duration::from_millis(100);

const WAITING: &str = "Talk to Dampe, and start pressing once Navi is done";

fn describe(status: &Status) -> String {
    match status {
        Status::Collecting(n) => format!("{n} flutters so far"),
        Status::Narrowing(n) => format!("{n} states still fit"),
        Status::Confirming(_) | Status::Locked => "Locked: follow the countdown".to_owned(),
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    logfile::start();
    let timing = Arc::new(Timing::load());
    let mut lag = DEFAULT_LAG_MS;
    let mut key_name = DEFAULT_START_OVER_KEY.to_owned();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manual" => {}
            "--navi-key" => key_name = args.next().ok_or("--navi-key needs a key")?.to_ascii_uppercase(),
            "--platform" => {
                let platform = args.next().and_then(|value| Platform::parse(value));
                timing.set_platform(platform.ok_or("--platform needs n64 or vc")?);
            }
            "--lag-ms" => lag = args.next().and_then(|value| value.parse().ok()).ok_or("--lag-ms needs a number")?,
            other => return Err(format!("{other} doesn't go with --manual")),
        }
    }
    timing.use_manual_lag(lag);
    logfile::line(format_args!("Manual: no video. {}", timing.describe()));

    let key = hotkey::parse_key(&key_name).ok_or("--navi-key needs a function key, F1 to F24")?;
    let start_over_key = Arc::new(AtomicBool::new(false));
    let failed_key = key_name.clone();
    hotkey::listen(key, start_over_key.clone(), move || {
        let error = format!("Couldn't use {failed_key} to start over: another program has it. Try --navi-key.");
        logfile::line(&error);
        crate::show_error(&error);
    });

    let (press_sender, presses) = channel();
    let sound = Arc::new(Sound::load());
    let window_sound = sound.clone();
    let window_timing = timing.clone();
    let window_key = start_over_key.clone();
    gui::run(
        "Manual: no video".to_owned(),
        window_sound,
        window_timing,
        window_key,
        key_name,
        Some(press_sender.clone()),
        move |gui, start_over| read(presses, press_sender, timing, gui, start_over, start_over_key, sound),
    )
}

/// Stops a countdown, if one is running.
fn cancel(countdown: &mut Option<Sender<()>>) {
    if let Some(cancel) = countdown.take() {
        let _ = cancel.send(());
    }
}

fn read(
    presses: Receiver<(u8, Instant)>,
    press_sender: Sender<(u8, Instant)>,
    timing: Arc<Timing>,
    gui: Gui,
    start_over: Arc<AtomicBool>,
    start_over_key: Arc<AtomicBool>,
    sound: Arc<Sound>,
) {
    let started = Instant::now();
    let log = Log { started, gui: Some(gui.clone()) };
    let failed_gui = gui.clone();
    let count_keys = CountKeys::new(press_sender, move || {
        let error = "Couldn't use keys 3 and 4 from any program: another program has one. They work in this window.";
        logfile::line(error);
        failed_gui.add_log(error.to_owned());
        failed_gui.set_global_keys_off();
    });
    count_keys.set_on(true);
    gui.set_manual(move |on| count_keys.set_on(on));
    gui.set_status(WAITING.to_owned(), Tone::Reading);
    let mut session = ManualSession::new();
    let mut events = Vec::new();
    let mut countdown: Option<Sender<()>> = None;
    let mut readings_shown = Readings::default();
    let mut last_readings = Instant::now();
    let mut shown_percent = 0;

    loop {
        // Presses are stamped when they're made, so waiting here doesn't change their time
        let press = presses.recv_timeout(TICK);
        let seconds = started.elapsed().as_secs_f64();
        if start_over.swap(false, Ordering::Relaxed) | start_over_key.swap(false, Ordering::Relaxed) {
            cancel(&mut countdown);
            session.restart();
            gui.set_cue(None);
            gui.set_likely(None);
            gui.set_latest(String::new());
            gui.set_status(WAITING.to_owned(), Tone::Reading);
            log.line("Starting over.");
        }
        if let Ok((count, at)) = press {
            let at = at.saturating_duration_since(started).as_secs_f64();
            logfile::to_file(format_args!("press {count} {at:.3}"));
            if session.press(count, at) {
                gui.set_latest("Flipped the last press".to_owned());
            } else {
                gui.set_latest(format!("Flutter {}: pressed {count}", session.presses()));
                if session.presses() == 1 {
                    gui.set_status(describe(&Status::Collecting(1)), Tone::Reading);
                }
            }
        }

        session.tick(seconds, &mut events);
        let new_readings = session.take_readings();
        let changed = new_readings.is_some();
        if let Some(readings) = new_readings {
            readings_shown = readings;
        }
        if changed || (!readings_shown.top.is_empty() && last_readings.elapsed() >= READINGS_REFRESH) {
            last_readings = Instant::now();
            // Until the moment to press, like the countdown
            let lead = timing.lead_ms() / 1000.0;
            let left = |reading: &Reading| {
                reading.next_window.map(|window| ManualSession::window_seconds(&window) - lead - seconds)
            };
            gui.set_readings(&readings_shown, left);
        }

        for event in events.drain(..) {
            match event {
                Event::Status(status) => {
                    log.line(describe(&status));
                    let tone = if status == Status::Locked { Tone::Locked } else { Tone::Reading };
                    gui.set_status(describe(&status), tone);
                }
                Event::Cue { seconds: box_seconds, window, .. } => {
                    let at_seconds = box_seconds - timing.lead_ms() / 1000.0;
                    let at = started + Duration::from_secs_f64(at_seconds.max(0.0));
                    let frames = window.last_roll - window.first_roll + 1;
                    let margin = frames as f64 / GAME_FPS / 2.0 * 1000.0;
                    log.line(format_args!(
                        "Press A when the countdown reaches zero, in {:.1} s. Safe within +-{margin:.0} ms.",
                        at_seconds - seconds
                    ));
                    logfile::to_file(format_args!("cue {at_seconds:.3}"));
                    cancel(&mut countdown);
                    countdown = Some(start_countdown(at, sound.clone()));
                    gui.set_cue(Some(at));
                    gui.set_likely(None);
                }
                Event::LikelyCue { seconds: box_seconds, chance, .. } => {
                    let at_seconds = box_seconds - timing.lead_ms() / 1000.0;
                    let at = started + Duration::from_secs_f64(at_seconds.max(0.0));
                    let percent = ((chance * 100.0).round() as u32).min(99);
                    log.line(format_args!(
                        "{percent}% chance: press A at zero, in {:.1} s. No state is proven yet.",
                        at_seconds - seconds
                    ));
                    logfile::to_file(format_args!("likely {at_seconds:.3} {chance:.3}"));
                    cancel(&mut countdown);
                    countdown = Some(start_countdown(at, sound.clone()));
                    shown_percent = percent;
                    gui.set_cue(Some(at));
                    gui.set_likely(Some(percent));
                    gui.set_status(format!("{percent}% chance"), Tone::Warning);
                }
                Event::LikelyChance { chance } => {
                    let percent = ((chance * 100.0).round() as u32).min(99);
                    if percent != shown_percent {
                        shown_percent = percent;
                        gui.set_likely(Some(percent));
                        gui.set_status(format!("{percent}% chance"), Tone::Warning);
                    }
                }
                Event::LikelyCancelled => {
                    cancel(&mut countdown);
                    log.line("That likely press isn't likely any more. Don't press yet.");
                    gui.set_cue(None);
                    gui.set_likely(None);
                    gui.set_status("Keep pressing".to_owned(), Tone::Reading);
                }
                Event::CueCancelled => {
                    cancel(&mut countdown);
                    log.line("Cancelled: the presses stopped matching. Don't press yet.");
                    gui.set_cue(None);
                    gui.set_likely(None);
                    gui.set_status("Cancelled: don't press yet".to_owned(), Tone::Warning);
                }
                _ => {}
            }
        }
    }
}
