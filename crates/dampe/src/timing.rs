//! How early the countdown has to reach zero. A press reaches the screen after the game's input
//! latency, and frames reach this program after the feed's delay through the capture card, OBS and
//! the virtual camera, so:
//!
//!     countdown zero = when the frame should arrive here - input latency - feed delay
//!
//! Input latency is known per platform. Feed delay differs per setup, so it's measured: each press
//! shows how long after zero the box really left. A person pressing to a countdown anticipates it,
//! so reaction time isn't part of this, and any habit of pressing early or late corrects itself.

use std::path::PathBuf;
use std::sync::Mutex;

/// Game frames are 50 ms.
const GAME_FRAME_MS: f64 = 50.0;
/// The feed delay until a press has measured it, in milliseconds: from the first presses on a Wii
/// through OBS Virtual Camera.
const DEFAULT_FEED_DELAY_MS: f64 = 100.0;
/// Presses kept for the median, so one mistimed press can't move it far.
#[cfg(any(feature = "video", test))]
const MEASUREMENTS_KEPT: usize = 9;
/// A measured feed delay outside this is a press that wasn't following the countdown.
#[cfg(any(feature = "video", test))]
const PLAUSIBLE_FEED_DELAY_MS: std::ops::RangeInclusive<f64> = 0.0..=500.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    N64,
    WiiVc,
}

impl Platform {
    pub fn input_latency_ms(self) -> f64 {
        GAME_FRAME_MS
            * match self {
                Platform::N64 => 2.0,
                Platform::WiiVc => 3.0,
            }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "n64" => Some(Platform::N64),
            "vc" | "wii" | "wiivc" | "wii-vc" => Some(Platform::WiiVc),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Platform::N64 => "n64",
            Platform::WiiVc => "vc",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct State {
    platform: Platform,
    measurements: Vec<f64>,
    /// A delay given on the command line, instead of what's been measured.
    fixed_feed_delay: Option<f64>,
}

fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[middle]),
        _ => Some((sorted[middle - 1] + sorted[middle]) / 2.0),
    }
}

impl State {
    fn feed_delay_ms(&self) -> f64 {
        self.fixed_feed_delay.or_else(|| median(&self.measurements)).unwrap_or(DEFAULT_FEED_DELAY_MS)
    }

    /// Adds a press that left the box `after_zero_ms` after the countdown's zero. Whether it counted.
    #[cfg(any(feature = "video", test))]
    fn learn(&mut self, after_zero_ms: f64) -> bool {
        let measured = after_zero_ms - self.platform.input_latency_ms();
        if self.fixed_feed_delay.is_some() || !PLAUSIBLE_FEED_DELAY_MS.contains(&measured) {
            return false;
        }
        self.measurements.push(measured);
        let excess = self.measurements.len().saturating_sub(MEASUREMENTS_KEPT);
        self.measurements.drain(..excess);
        true
    }

    fn parse(text: &str) -> Self {
        let mut state = Self { platform: Platform::WiiVc, measurements: Vec::new(), fixed_feed_delay: None };
        for line in text.lines() {
            match line.split_once('=') {
                Some(("platform", value)) => state.platform = Platform::parse(value.trim()).unwrap_or(state.platform),
                Some(("feed_delays", value)) => {
                    state.measurements = value.split(',').filter_map(|delay| delay.trim().parse().ok()).collect();
                }
                _ => {}
            }
        }
        state
    }

    fn to_text(&self) -> String {
        let delays: Vec<String> = self.measurements.iter().map(|delay| format!("{delay:.0}")).collect();
        format!("platform={}\nfeed_delays={}\n", self.platform.name(), delays.join(","))
    }
}

fn path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("timing.txt"))
}

/// The platform and feed delay, remembered between runs.
pub struct Timing {
    state: Mutex<State>,
    /// Going by a lag given here, with no feed to measure a delay from.
    manual: std::sync::atomic::AtomicBool,
}

impl Timing {
    pub fn load() -> Self {
        let text = path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
        Self { state: Mutex::new(State::parse(&text)), manual: Default::default() }
    }

    fn save(state: &State) {
        let Some(path) = path() else { return };
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(path, state.to_text());
    }

    pub fn platform(&self) -> Platform {
        self.state.lock().unwrap().platform
    }

    pub fn set_platform(&self, platform: Platform) {
        let mut state = self.state.lock().unwrap();
        state.platform = platform;
        Self::save(&state);
    }

    /// For this run only, and nothing gets measured.
    pub fn fix_feed_delay(&self, milliseconds: f64) {
        self.state.lock().unwrap().fixed_feed_delay = Some(milliseconds);
    }

    /// With no video, how late presses are takes the place of the feed delay, for this run only.
    pub fn use_manual_lag(&self, milliseconds: f64) {
        self.fix_feed_delay(milliseconds);
        self.manual.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// How long before a frame should arrive the press has to happen.
    pub fn lead_ms(&self) -> f64 {
        let state = self.state.lock().unwrap();
        state.platform.input_latency_ms() + state.feed_delay_ms()
    }

    /// Learns from a press. Whether it counted.
    #[cfg(feature = "video")]
    pub fn learn(&self, after_zero_ms: f64) -> bool {
        let mut state = self.state.lock().unwrap();
        let counted = state.learn(after_zero_ms);
        if counted {
            Self::save(&state);
        }
        counted
    }

    pub fn describe(&self) -> String {
        let state = self.state.lock().unwrap();
        if self.manual.load(std::sync::atomic::Ordering::Relaxed) {
            return format!("Input {:.0} ms + lag {:.0} ms", state.platform.input_latency_ms(), state.feed_delay_ms());
        }
        let source = match (state.fixed_feed_delay, state.measurements.len()) {
            (Some(_), _) => "set by hand".to_owned(),
            (None, 0) => "not measured yet".to_owned(),
            (None, 1) => "from 1 press".to_owned(),
            (None, presses) => format!("from {presses} presses"),
        };
        format!("Input {:.0} ms + feed {:.0} ms ({source})", state.platform.input_latency_ms(), state.feed_delay_ms())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(platform: Platform) -> State {
        State { platform, measurements: Vec::new(), fixed_feed_delay: None }
    }

    #[test]
    fn input_latency_is_per_platform() {
        assert_eq!(Platform::N64.input_latency_ms(), 100.0);
        assert_eq!(Platform::WiiVc.input_latency_ms(), 150.0);
    }

    /// The box leaving 254 ms after zero on Wii VC leaves 104 ms for the feed.
    #[test]
    fn a_press_measures_what_input_latency_does_not_explain() {
        let mut timing = state(Platform::WiiVc);
        assert_eq!(timing.feed_delay_ms(), DEFAULT_FEED_DELAY_MS);
        assert!(timing.learn(254.0));
        assert_eq!(timing.feed_delay_ms(), 104.0);
    }

    #[test]
    fn one_mistimed_press_barely_moves_it() {
        let mut timing = state(Platform::WiiVc);
        for after_zero in [240.0, 250.0, 260.0, 600.0] {
            timing.learn(after_zero);
        }
        // 600 ms is 450 ms after input latency: plausible enough to count, but the median holds
        assert!((timing.feed_delay_ms() - 105.0).abs() < 1.0, "{}", timing.feed_delay_ms());
    }

    #[test]
    fn presses_that_did_not_follow_the_countdown_do_not_count() {
        let mut timing = state(Platform::WiiVc);
        assert!(timing.learn(200.0), "the control");
        assert!(!timing.learn(100.0), "before the input could have arrived");
        assert!(!timing.learn(1500.0), "long after");
        assert_eq!(timing.measurements.len(), 1);
    }

    #[test]
    fn a_fixed_delay_wins_and_nothing_is_measured() {
        let mut timing = state(Platform::N64);
        timing.fixed_feed_delay = Some(40.0);
        assert!(!timing.learn(200.0));
        assert_eq!(timing.feed_delay_ms(), 40.0);
    }

    #[test]
    fn saved_settings_come_back() {
        let mut timing = state(Platform::N64);
        timing.learn(180.0);
        timing.learn(190.0);
        assert_eq!(State::parse(&timing.to_text()), timing);
        assert_eq!(State::parse("").platform, Platform::WiiVc);
    }
}
