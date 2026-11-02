//! Reading flutters from presses: `dampe live`, or `dampe live --manual` in a build with video. No video at all. Flutters are pressed by hand in the window, and it says when
//! to press A. Nothing here reads a device, and nothing is ever sent to the game.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use dampe_core::manual::{DEFAULT_LAG_MS, Event, GAME_FPS, ManualSession};
use dampe_core::reading::{Reading, Readings, Status, Window};

use crate::cues::{Log, start_countdown};
use crate::gui::{self, Gui, Tone};
use crate::hotkey::{self, CountKeys};
use crate::logfile;
use crate::sound::Sound;
use crate::timing::{Platform, Timing};

const DEFAULT_START_OVER_KEY: &str = "F8";
/// How often to look for what's new while waiting for a press.
const TICK: Duration = Duration::from_millis(20);
/// How often the times in the list of readings count down. Fast enough to press by.
const READINGS_REFRESH: Duration = Duration::from_millis(100);

/// What `--lag-ms` takes: earlier than the flutter's end is possible, by someone anticipating it.
const LAG_RANGE_MS: std::ops::RangeInclusive<f64> = -500.0..=2000.0;

const WAITING: &str = "Talk to Dampe, then press once Navi is done";
/// Counts shown of the last presses, enough to catch a wrong one.
const RECENT_PRESSES: usize = 6;

fn tone(status: &Status) -> Tone {
    if *status == Status::Locked { Tone::Locked } else { Tone::Reading }
}

/// A chance as shown: nothing's certain until it's locked, so never 100%.
fn shown_percent_of(chance: f64) -> u32 {
    ((chance * 100.0).round() as u32).min(99)
}

fn describe(status: &Status) -> String {
    match status {
        Status::Collecting(1) => "1 flutter so far".to_owned(),
        Status::Collecting(n) => format!("{n} flutters so far"),
        Status::Narrowing(n) => format!("{n} states still fit"),
        Status::Confirming(_) | Status::Locked => "Locked: waiting for a good moment".to_owned(),
    }
}

/// The last counts pressed, oldest first.
fn recent_text(counts: &[u8]) -> String {
    let shown = &counts[counts.len().saturating_sub(RECENT_PRESSES)..];
    shown.iter().map(u8::to_string).collect::<Vec<_>>().join(" ")
}

/// How far off the moment to press can be for `window`, either way, in milliseconds.
fn margin_ms(window: &Window) -> f64 {
    (window.last_roll - window.first_roll + 1) as f64 / GAME_FPS / 2.0 * 1000.0
}

/// The start over key, for hearing it once there's a window to say if that fails.
struct StartOverKey {
    code: u32,
    name: String,
    pressed: Arc<AtomicBool>,
}

/// A lag worth using. Text like `inf` also parses as a number, and no time could be worked out from it.
fn parse_lag(text: &str) -> Option<f64> {
    text.parse().ok().filter(|lag| LAG_RANGE_MS.contains(lag))
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
            // Its old name, from when it was only for Navi
            "--start-over-key" | "--navi-key" => {
                key_name = args.next().ok_or("--start-over-key needs a key")?.to_ascii_uppercase();
            }
            "--platform" => {
                let platform = args.next().and_then(|value| Platform::parse(value));
                timing.set_platform(platform.ok_or("--platform needs n64 or vc")?);
            }
            "--lag-ms" => {
                lag = args
                    .next()
                    .and_then(|value| parse_lag(value))
                    .ok_or("--lag-ms needs a number of milliseconds, from -500 to 2000")?;
            }
            other => return Err(format!("Unknown option {other}\n\n{}", crate::USAGE)),
        }
    }
    timing.use_manual_lag(lag);
    logfile::line(format_args!("Manual: no video. {}", timing.describe()));

    let code = hotkey::parse_key(&key_name).ok_or("--start-over-key needs a function key, F1 to F24")?;
    let start_over_key = StartOverKey { code, name: key_name.clone(), pressed: Arc::new(AtomicBool::new(false)) };

    let (press_sender, presses) = channel();
    let sound = Arc::new(Sound::load());
    let window_sound = sound.clone();
    let window_timing = timing.clone();
    let window_key = start_over_key.pressed.clone();
    let options = gui::Options {
        source: String::new(),
        sound: window_sound,
        timing: window_timing,
        hotkey_pressed: window_key,
        hotkey_name: key_name,
        presses: Some(press_sender.clone()),
        manual: Some(gui::ManualStart { status: WAITING.to_owned(), global_keys: hotkey::load_count_keys_on() }),
    };
    gui::run(options, move |gui, start_over| {
        read(presses, press_sender, timing, gui, start_over, start_over_key, sound)
    })
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
    start_over_key: StartOverKey,
    sound: Arc<Sound>,
) {
    let started = Instant::now();
    // Only to the file: the window has no room for a log
    let log = Log { started, gui: None };
    let key_gui = gui.clone();
    let key_name = start_over_key.name;
    hotkey::listen(start_over_key.code, start_over_key.pressed.clone(), move || {
        logfile::line(format_args!(
            "Couldn't use {key_name} to start over: another program has it. Try --start-over-key."
        ));
        key_gui.set_warning(format!("Another program has {key_name}: use the button"));
        key_gui.set_start_over_key_off();
    });
    let start_over_key = start_over_key.pressed;
    let failed_gui = gui.clone();
    let count_keys = CountKeys::new(press_sender, move || {
        logfile::line("Couldn't use keys 3 and 4 from any program: another program has one. They work in this window.");
        failed_gui.set_warning("Another program has key 3 or 4: keep this window focused".to_owned());
        failed_gui.set_global_keys_off();
    });
    let global_keys = hotkey::load_count_keys_on();
    count_keys.set_on(global_keys);
    gui.on_global_keys_changed(move |on| {
        hotkey::save_count_keys_on(on);
        count_keys.set_on(on);
    });
    let mut session = ManualSession::new(timing.lead_ms() / 1000.0);
    let mut events = Vec::new();
    let mut countdown: Option<Sender<()>> = None;
    let mut readings_shown = Readings::default();
    let mut last_readings = Instant::now();
    let mut shown_percent = 0;
    // When to press for the box to leave at `box_seconds`, on the clock since `started` and as an instant
    let moment_to_press = |box_seconds: f64| {
        let at_seconds = box_seconds - timing.lead_ms() / 1000.0;
        (at_seconds, started + Duration::from_secs_f64(at_seconds.max(0.0)))
    };
    let mut status = Status::Collecting(0);
    let mut counts: Vec<u8> = Vec::new();
    // The moment to press of the running countdown, until it's gone by
    let mut cue_at: Option<Instant> = None;

    loop {
        // Presses are stamped when they're made, so waiting here doesn't change their time
        let press = presses.recv_timeout(TICK);
        let seconds = started.elapsed().as_secs_f64();
        if start_over.swap(false, Ordering::Relaxed) | start_over_key.swap(false, Ordering::Relaxed) {
            cancel(&mut countdown);
            session.restart();
            status = Status::Collecting(0);
            cue_at = None;
            gui.clear_cue();
            counts.clear();
            gui.set_recent(String::new());
            gui.set_latest(String::new());
            gui.set_status(WAITING.to_owned(), Tone::Reading);
            log.line("Starting over.");
        }
        if let Ok((count, at)) = press {
            let at = at.saturating_duration_since(started).as_secs_f64();
            logfile::to_file(format_args!("press {count} {at:.3}"));
            if session.press(count, at) {
                if let Some(last) = counts.last_mut() {
                    *last = 7 - *last;
                    gui.set_latest(format!("Flutter {}: now {last}", session.presses()));
                }
            } else {
                counts.push(count);
                gui.set_latest(format!("Flutter {}: pressed {count}", session.presses()));
                if session.presses() == 1 {
                    gui.set_status(describe(&Status::Collecting(1)), Tone::Reading);
                }
            }
            gui.set_recent(recent_text(&counts));
        }

        // Nothing here can see a dig, and it ends what's been read
        if cue_at.is_some_and(|at| at <= Instant::now()) {
            cue_at = None;
            gui.set_latest("Dug? Start over before talking again".to_owned());
            if status == Status::Locked {
                gui.set_status(describe(&status), tone(&status));
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
                Event::Status(new) => {
                    status = new;
                    log.line(describe(&status));
                    // For bug reports, since the window doesn't show them
                    if let (Status::Locked, Some(top)) = (&status, readings_shown.top.first()) {
                        let candidate = top.candidate;
                        log.line(format_args!("Locked on {:06x}, Link {}", candidate.seed, candidate.link_first));
                    }
                    gui.set_status(describe(&status), tone(&status));
                }
                Event::Cue { seconds: box_seconds, window } => {
                    let (at_seconds, at) = moment_to_press(box_seconds);
                    let margin = margin_ms(&window);
                    log.line(format_args!("Press A at zero, in {:.1} s. +-{margin:.0} ms.", at_seconds - seconds));
                    logfile::to_file(format_args!("cue {at_seconds:.3}"));
                    cancel(&mut countdown);
                    countdown = Some(start_countdown(at, sound.clone()));
                    cue_at = Some(at);
                    gui.set_cue(Some(at));
                    gui.set_likely(None);
                    gui.set_margin(margin);
                    gui.set_status("Locked: follow the countdown".to_owned(), Tone::Locked);
                }
                Event::LikelyCue { seconds: box_seconds, window, chance } => {
                    let (at_seconds, at) = moment_to_press(box_seconds);
                    let percent = shown_percent_of(chance);
                    log.line(format_args!("{percent}%: press A at zero, in {:.1} s.", at_seconds - seconds));
                    logfile::to_file(format_args!("likely {at_seconds:.3} {chance:.3}"));
                    cancel(&mut countdown);
                    countdown = Some(start_countdown(at, sound.clone()));
                    shown_percent = percent;
                    cue_at = Some(at);
                    gui.set_cue(Some(at));
                    gui.set_likely(Some(percent));
                    gui.set_margin(margin_ms(&window));
                    gui.set_status(format!("{percent}% chance"), Tone::Warning);
                }
                Event::LikelyChance { chance } => {
                    let percent = shown_percent_of(chance);
                    if percent != shown_percent {
                        shown_percent = percent;
                        gui.set_likely(Some(percent));
                        gui.set_status(format!("{percent}% chance"), Tone::Warning);
                    }
                }
                Event::LikelyCancelled => {
                    cancel(&mut countdown);
                    cue_at = None;
                    log.line("Not likely any more. Don't press yet.");
                    gui.clear_cue();
                    gui.set_status("Keep pressing".to_owned(), Tone::Reading);
                }
                // The window takes the countdown down by itself, right after the moment to press
                Event::LikelyEnded => {
                    gui.set_likely(None);
                    gui.set_status(describe(&status), tone(&status));
                }
                Event::CueCancelled => {
                    cancel(&mut countdown);
                    cue_at = None;
                    log.line("Presses don't match. Don't press yet.");
                    gui.clear_cue();
                    gui.set_status("Cancelled: don't press yet".to_owned(), Tone::Warning);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lag_has_to_be_a_time_that_can_be_worked_with() {
        assert_eq!(parse_lag("250"), Some(250.0));
        assert_eq!(parse_lag("-100"), Some(-100.0));
        for bad in ["inf", "-inf", "nan", "-1e300", "2001", "soon"] {
            assert_eq!(parse_lag(bad), None, "{bad}");
        }
    }

    #[test]
    fn recent_presses_show_the_last_few_in_order() {
        assert_eq!(recent_text(&[]), "");
        assert_eq!(recent_text(&[3, 4]), "3 4");
        assert_eq!(recent_text(&[3, 3, 4, 3, 4, 4, 3, 4]), "4 3 4 4 3 4");
    }

    #[test]
    fn one_flutter_is_not_flutters() {
        assert_eq!(describe(&Status::Collecting(1)), "1 flutter so far");
        assert_eq!(describe(&Status::Collecting(2)), "2 flutters so far");
    }
}
