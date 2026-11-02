//! What reading from video and from presses share: the log, and the sound of a countdown.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
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
        let ticks = COUNTDOWN.iter().filter_map(|&seconds| at.checked_sub(Duration::from_secs(seconds)));
        let tones = ticks.map(|when| (when, sound::TICK)).chain([(at, sound::GO)]);
        play_on_time(tones, &cancelled, |tone| sound.cue(tone));
    });
    cancel
}

/// Plays each tone at its time, in order, until told to stop. A tone whose time has passed is skipped,
/// since it's the rhythm that's followed, and one played late would only mislead.
fn play_on_time<Tone>(
    tones: impl Iterator<Item = (Instant, Tone)>,
    cancelled: &Receiver<()>,
    mut play: impl FnMut(Tone),
) {
    for (when, tone) in tones {
        let wait = when.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            continue;
        }
        match cancelled.recv_timeout(wait) {
            Err(RecvTimeoutError::Timeout) => play(tone),
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOON: Duration = Duration::from_millis(30);

    /// What plays of tones at these offsets from now, with `cancel` sent first if it's true.
    fn played(past: &[u8], future: &[u8], cancel: bool) -> Vec<u8> {
        let now = Instant::now();
        let long_ago = now.checked_sub(Duration::from_secs(1)).expect("the clock goes back that far");
        let ahead = |i: usize| now + SOON * (i as u32 + 1);
        let tones = past
            .iter()
            .map(|&tone| (long_ago, tone))
            .chain(future.iter().enumerate().map(|(i, &tone)| (ahead(i), tone)));
        let (sender, cancelled) = channel();
        if cancel {
            sender.send(()).expect("it's listening");
        }
        let mut heard = Vec::new();
        play_on_time(tones, &cancelled, |tone| heard.push(tone));
        heard
    }

    #[test]
    fn tones_in_time_all_play_and_the_ones_gone_by_do_not() {
        assert_eq!(played(&[], &[1, 2], false), [1, 2]);
        assert_eq!(played(&[1, 2], &[3], false), [3]);
        assert_eq!(played(&[1, 2, 3], &[], false), [] as [u8; 0]);
    }

    #[test]
    fn being_told_to_stop_stops_the_rest() {
        assert_eq!(played(&[1], &[2, 3], true), [] as [u8; 0]);
    }
}
