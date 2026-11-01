//! What reading from video and from presses share: the log, and the sound of a countdown.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use crate::gui::Gui;
use crate::logfile;
use crate::sound::{self, Sound};

/// The countdown's short tones come this many seconds before the long one.
const COUNTDOWN: [u64; 3] = [3, 2, 1];

/// Where a line of output goes: the terminal, and the window if there is one.
#[derive(Clone)]
pub struct Log {
    pub started: Instant,
    pub gui: Option<Gui>,
}

impl Log {
    /// A line with the time since the program started.
    pub fn line(&self, text: impl std::fmt::Display) {
        let elapsed = self.started.elapsed().as_secs_f64();
        let line = format!("[{:02}:{:04.1}] {text}", (elapsed / 60.0) as u64, elapsed % 60.0);
        logfile::line(&line);
        if let Some(gui) = &self.gui {
            gui.add_log(line);
        }
    }
}

/// Plays a countdown ending at `at`, if the sound is on, unless told to stop first.
pub fn start_countdown(at: Instant, sound: Arc<Sound>) -> Sender<()> {
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
