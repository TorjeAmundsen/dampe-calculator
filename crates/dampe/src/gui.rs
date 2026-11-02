//! The window, in Slint with its software renderer like AutoSplit-rs. It shows the feed in color and
//! what the read sees on top. The read itself only uses brightness.
//!
//! It's only for looking at: closing it ends the program, but a slow window never delays the read.
//! While the last picture hasn't been shown yet, new ones are skipped.

use std::cell::Cell;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[cfg(feature = "video")]
use dampe_core::detector::Rect;
#[cfg(feature = "video")]
use dampe_core::picture::{GAME_HEIGHT, GAME_WIDTH, GameArea, MARGIN_X, MARGIN_Y};
use dampe_core::tracker::{Reading, Readings};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel, Weak};
#[cfg(feature = "video")]
use slint::{Rgba8Pixel, SharedPixelBuffer};

#[cfg(feature = "video")]
use crate::capture::CapturedFrame;
use crate::hotkey;
use crate::logfile;
use crate::sound::{self, Sound};
use crate::timing::{Platform, Timing};

slint::include_modules!();

const LOG_LINES: usize = 12;
/// The bar fills over this much of the countdown.
const BAR_SECONDS: f64 = 5.0;
/// Fast, so the moment to press shows when it is. Nothing is redrawn unless it changed.
const COUNTDOWN_REFRESH: Duration = Duration::from_millis(16);
/// How long each of the last seconds flashes, and how long the moment to press stays up.
const PULSE_SECONDS: f64 = 0.12;
const PRESS_SECONDS: f64 = 0.6;
/// The flashes come on these many seconds before the moment to press.
const PULSE_AT: [f64; 3] = [3.0, 2.0, 1.0];
/// The zoomed view: this part of the game's picture around the eye, and its size in pixels.
#[cfg(feature = "video")]
const ZOOM_GAME_WIDTH: usize = 80;
#[cfg(feature = "video")]
const ZOOM_GAME_HEIGHT: usize = 60;
#[cfg(feature = "video")]
const ZOOM_WIDTH: u32 = 160;
#[cfg(feature = "video")]
const ZOOM_HEIGHT: u32 = 120;

const READING: Color = Color::from_rgb_u8(255, 255, 255);
const LOCKED: Color = Color::from_rgb_u8(108, 214, 136);
const WARNING: Color = Color::from_rgb_u8(224, 160, 64);

/// How a status should look.
#[derive(Clone, Copy)]
pub enum Tone {
    Reading,
    Locked,
    Warning,
}

/// One rectangle of the game's picture from the frame, in color.
#[cfg(feature = "video")]
fn pixels(
    frame: &CapturedFrame,
    area: GameArea,
    (game_x, game_y): (usize, usize),
    (game_width, game_height): (usize, usize),
    (width, height): (u32, u32),
) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let columns: Vec<usize> = (0..width as usize)
        .map(|x| {
            let game = game_x + x * game_width / width as usize;
            (area.left + game * area.width / GAME_WIDTH).min(frame.width - 1)
        })
        .collect();
    let chroma_stride = frame.width.next_multiple_of(2);
    let (out, row_width) = (buffer.make_mut_slice(), width as usize);
    for (y, row) in out.chunks_exact_mut(row_width).enumerate() {
        let game = game_y + y * game_height / height as usize;
        let source_y = (area.top + game * area.height / GAME_HEIGHT).min(frame.height - 1);
        let luma = &frame.luma[source_y * frame.width..];
        let chroma = (source_y / 2) * chroma_stride;
        for (pixel, &source_x) in row.iter_mut().zip(&columns) {
            let i = chroma + source_x / 2 * 2;
            let (y, u, v) = match (frame.chroma.get(i), frame.chroma.get(i + 1)) {
                (Some(&u), Some(&v)) => (luma[source_x], u, v),
                _ => (luma[source_x], 128, 128),
            };
            *pixel = bt709(y, u, v);
        }
    }
    buffer
}

/// Video range BT.709, what OBS Virtual Camera sends.
#[cfg(feature = "video")]
#[inline]
fn bt709(y: u8, u: u8, v: u8) -> Rgba8Pixel {
    let (y, u, v) = (298 * (i32::from(y) - 16), i32::from(u) - 128, i32::from(v) - 128);
    let channel = |value: i32| ((value + 128) >> 8).clamp(0, 255) as u8;
    Rgba8Pixel { r: channel(y + 459 * v), g: channel(y - 55 * u - 136 * v), b: channel(y + 541 * u), a: 255 }
}

#[cfg(feature = "video")]
fn game_rect(rect: Rect) -> GameRect {
    GameRect {
        x: (usize::from(rect.left) + MARGIN_X) as f32,
        y: (usize::from(rect.top) + MARGIN_Y) as f32,
        width: f32::from(rect.right - rect.left + 1),
        height: f32::from(rect.bottom - rect.top + 1),
    }
}

/// A chance for reading at a glance, as a percentage.
fn percent(share: f64) -> String {
    match share * 100.0 {
        percent if percent >= 99.95 => ">99.9%".to_owned(),
        percent if percent > 0.0 && percent < 0.1 => "<0.1%".to_owned(),
        percent => format!("{percent:.1}%"),
    }
}

/// The rows of the list of readings, and what's said over them. `left` is the seconds until a
/// reading's next Heart Piece.
fn reading_rows(readings: &Readings, left: impl Fn(&Reading) -> Option<f64>) -> (Vec<ReadingRow>, String) {
    let rows = readings
        .top
        .iter()
        .enumerate()
        .map(|(rank, reading)| ReadingRow {
            rank: rank as i32 + 1,
            share: reading.share as f32,
            chance: percent(reading.share).into(),
            seed: format!("{:06x}", reading.candidate.seed).into(),
            link: format!("Link {}", reading.candidate.link_first).into(),
            hits: reading.hits as i32,
            misses: reading.misses as i32,
            strays: reading.extras as i32,
            predicted: reading.confirmed as i32,
            heart_piece: match left(reading) {
                Some(seconds) if seconds >= 0.0 => format!("{seconds:.1} s").into(),
                Some(_) => "passed".into(),
                None => "over 60 s".into(),
            },
        })
        .collect();
    let summary = match readings.total {
        0 => "It takes a few flutters to start looking".to_owned(),
        // Presses have accepted a state once locked, so nothing's left for the real one being missing
        total if readings.not_found == 0.0 => format!("{total} candidates"),
        1 => format!("1 candidate. Chance it's not this: {}", percent(readings.not_found)),
        total => format!("{total} candidates. Chance it's none of them: {}", percent(readings.not_found)),
    };
    (rows, summary)
}

/// Repaints the whole window with the next change, if it isn't showing video. See the rectangle this
/// flips for why. Video's frames are too frequent and too big to add this to, and hide the problem.
fn repaint_all(window: &MainWindow) {
    if window.get_manual() {
        window.set_repaint_all(!window.get_repaint_all());
    }
}

/// Talks to the window from the read's thread.
#[derive(Clone)]
pub struct Gui {
    window: Weak<MainWindow>,
    #[cfg(feature = "video")]
    picture_pending: Arc<AtomicBool>,
    cue: Arc<std::sync::Mutex<Option<Instant>>>,
}

impl Gui {
    fn on_ui(&self, change: impl FnOnce(&MainWindow) + Send + 'static) {
        let window = self.window.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window.upgrade() {
                change(&window);
                repaint_all(&window);
            }
        });
    }

    /// Shows a new game frame, unless the window hasn't caught up with the last one.
    #[cfg(feature = "video")]
    pub fn show_frame(&self, frame: &CapturedFrame, area: GameArea, blobs: Vec<Rect>, eye: Option<Rect>) {
        if self.picture_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let feed = pixels(frame, area, (0, 0), (GAME_WIDTH, GAME_HEIGHT), (GAME_WIDTH as u32, GAME_HEIGHT as u32));
        let zoom = eye.map(|eye| {
            let (x, y) = eye.center();
            let left =
                (usize::from(x) + MARGIN_X).saturating_sub(ZOOM_GAME_WIDTH / 2).min(GAME_WIDTH - ZOOM_GAME_WIDTH);
            let top =
                (usize::from(y) + MARGIN_Y).saturating_sub(ZOOM_GAME_HEIGHT / 2).min(GAME_HEIGHT - ZOOM_GAME_HEIGHT);
            pixels(frame, area, (left, top), (ZOOM_GAME_WIDTH, ZOOM_GAME_HEIGHT), (ZOOM_WIDTH, ZOOM_HEIGHT))
        });
        let blinking = eye.is_some_and(|eye| blobs.iter().any(|blob| blob.overlaps(&eye)));
        let pending = self.picture_pending.clone();
        self.on_ui(move |window| {
            pending.store(false, Ordering::Release);
            window.set_feed(slint::Image::from_rgba8(feed));
            let marks: Vec<GameRect> = blobs.into_iter().map(game_rect).collect();
            window.set_blobs(ModelRc::new(VecModel::from(marks)));
            window.set_has_eye(eye.is_some());
            window.set_blinking(blinking);
            if let Some(eye) = eye {
                window.set_eye(game_rect(eye));
            }
            if let Some(zoom) = zoom {
                window.set_eye_zoom(slint::Image::from_rgba8(zoom));
            }
        });
    }

    /// Shows the window for reading presses instead of video, with `count_keys` hearing about its
    /// setting for hearing them from any program.
    pub fn set_manual(&self, count_keys: impl Fn(bool) + Send + 'static) {
        self.on_ui(move |window| {
            window.set_manual(true);
            window.set_global_keys_available(hotkey::AVAILABLE);
            window.on_global_keys_changed(move |on| count_keys(on));
        });
    }

    /// The keys couldn't be had from any program, so they're only heard in the window.
    pub fn set_global_keys_off(&self) {
        self.on_ui(|window| window.set_global_keys(false));
    }

    pub fn set_status(&self, status: String, tone: Tone) {
        self.on_ui(move |window| {
            window.set_status(status.into());
            window.set_status_color(match tone {
                Tone::Reading => READING,
                Tone::Locked => LOCKED,
                Tone::Warning => WARNING,
            });
        });
    }

    #[cfg(feature = "video")]
    pub fn set_timing(&self, timing: String) {
        self.on_ui(move |window| window.set_timing(timing.into()));
    }

    pub fn set_latest(&self, latest: String) {
        self.on_ui(move |window| window.set_latest(latest.into()));
    }

    pub fn add_log(&self, line: String) {
        self.on_ui(move |window| {
            let model = window.get_log();
            let mut lines: Vec<SharedString> =
                (0..slint::Model::row_count(&model)).filter_map(|i| slint::Model::row_data(&model, i)).collect();
            lines.push(line.into());
            let excess = lines.len().saturating_sub(LOG_LINES);
            lines.drain(..excess);
            window.set_log(ModelRc::new(VecModel::from(lines)));
        });
    }

    /// Shows the most likely readings.
    pub fn set_readings(&self, readings: &Readings, left: impl Fn(&Reading) -> Option<f64>) {
        let (rows, summary) = reading_rows(readings, left);
        self.on_ui(move |window| {
            window.set_readings(ModelRc::new(VecModel::from(rows)));
            window.set_readings_summary(summary.into());
        });
    }

    /// Marks the running countdown as only likely, at this percentage.
    pub fn set_likely(&self, percent: Option<u32>) {
        self.on_ui(move |window| {
            window.set_likely(percent.is_some());
            window
                .set_likely_text(percent.map_or_else(SharedString::new, |percent| format!("{percent}% chance").into()));
        });
    }

    /// When the countdown reaches zero, if one's running.
    pub fn set_cue(&self, cue: Option<Instant>) {
        *self.cue.lock().unwrap() = cue;
    }
}

/// Opens the window, runs `read` on its own thread, and returns when the window closes.
/// Shows the log in the file manager.
#[cfg(windows)]
fn open_log_folder(path: &std::path::Path) {
    // Explorer doesn't take the whole argument in quotes, which is what a path with a space would get
    let select = format!("/select,\"{}\"", path.display());
    let _ = std::process::Command::new("explorer").raw_arg(select).spawn();
}

/// Shows the log's folder in the file manager.
#[cfg(not(windows))]
fn open_log_folder(path: &std::path::Path) {
    if let Some(folder) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(folder).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chances_stay_readable_at_the_extremes() {
        assert_eq!(percent(0.0), "0.0%");
        assert_eq!(percent(0.00001), "<0.1%");
        assert_eq!(percent(0.5), "50.0%");
        assert_eq!(percent(0.99999), ">99.9%");
    }
}

pub fn run(
    device: String,
    sound: Arc<Sound>,
    timing: Arc<Timing>,
    navi_gone: Arc<AtomicBool>,
    navi_key: String,
    presses: Option<Sender<(u8, Instant)>>,
    read: impl FnOnce(Gui, Arc<AtomicBool>) + Send + 'static,
) -> Result<(), String> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
        .map_err(|e| e.to_string())?;
    let window = MainWindow::new().map_err(|e| e.to_string())?;
    window.set_device(device.into());
    window.set_platform_index(i32::from(timing.platform() == Platform::WiiVc));
    window.set_timing(timing.describe().into());
    {
        let weak = window.as_weak();
        window.on_platform_changed(move || {
            let Some(window) = weak.upgrade() else { return };
            timing.set_platform(if window.get_platform_index() == 0 { Platform::N64 } else { Platform::WiiVc });
            window.set_timing(timing.describe().into());
        });
    }
    window.set_sound_on(sound.enabled());
    window.set_volume(f32::from(sound.volume()));

    // Hearing it when it's changed is how to set the volume, on a thread since playing waits
    let apply = {
        let (weak, sound) = (window.as_weak(), sound);
        move || {
            let Some(window) = weak.upgrade() else { return };
            let volume = window.get_volume().round() as u8;
            sound.set(window.get_sound_on(), volume);
            if window.get_sound_on() {
                std::thread::spawn(move || sound::play(sound::TICK, volume));
            }
        }
    };
    window.on_sound_changed(apply.clone());
    window.on_volume_released(apply);

    let start_over = Arc::new(AtomicBool::new(false));
    let flag = start_over.clone();
    window.on_start_over(move || flag.store(true, Ordering::Relaxed));
    if !hotkey::AVAILABLE {
        window.set_navi_key_text(hotkey::window_key(&navi_key).map(String::from).unwrap_or_default().into());
    }
    window.set_navi_key(navi_key.into());
    window.on_navi_gone(move || navi_gone.store(true, Ordering::Relaxed));
    if let Some(presses) = presses {
        window.on_flutter_pressed(move |count| {
            let _ = presses.send((count as u8, Instant::now()));
        });
    }
    window.on_open_log(|| {
        if let Some(path) = logfile::path() {
            open_log_folder(&path);
        }
    });

    let gui = Gui {
        window: window.as_weak(),
        #[cfg(feature = "video")]
        picture_pending: Arc::new(AtomicBool::new(false)),
        cue: Arc::default(),
    };

    let cue = gui.cue.clone();
    let shown = Rc::new(Cell::new(false));
    let refresh = slint::Timer::default();
    let weak = window.as_weak();
    refresh.start(slint::TimerMode::Repeated, COUNTDOWN_REFRESH, move || {
        let Some(window) = weak.upgrade() else { return };
        let cue_at = *cue.lock().unwrap();
        let now = Instant::now();
        let press_over = cue_at.is_some_and(|at| now.saturating_duration_since(at).as_secs_f64() > PRESS_SECONDS);
        let Some(at) = cue_at.filter(|_| !press_over) else {
            if shown.replace(false) {
                window.set_countdown(SharedString::new());
                window.set_countdown_share(0.0);
                window.set_pulse(false);
                window.set_press(false);
            }
            return;
        };
        shown.set(true);
        let left = at.saturating_duration_since(now).as_secs_f64();
        let pressing = at <= now;
        window.set_press(pressing);
        window.set_pulse(PULSE_AT.iter().any(|&second| (0.0..PULSE_SECONDS).contains(&(second - left))));
        let text: SharedString = if pressing { "PRESS A".into() } else { format!("{left:.1} s").into() };
        if window.get_countdown() != text {
            window.set_countdown(text);
        }
        window.set_countdown_share((1.0 - left / BAR_SECONDS).clamp(0.0, 1.0) as f32);
        repaint_all(&window);
    });

    let thread_gui = gui.clone();
    let thread_flag = start_over.clone();
    std::thread::Builder::new()
        .name("read".into())
        .spawn(move || read(thread_gui, thread_flag))
        .map_err(|e| e.to_string())?;

    window.run().map_err(|e| e.to_string())
}
