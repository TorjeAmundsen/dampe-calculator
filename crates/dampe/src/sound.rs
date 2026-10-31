//! The optional sound cue: short generated tones, at a volume that can be set, remembered between
//! runs. Off by default, because the window's cue is enough.

use std::path::PathBuf;
#[cfg(not(windows))]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[cfg(windows)]
use windows::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_SYNC};
#[cfg(windows)]
use windows::core::PCWSTR;

const SAMPLE_RATE: u32 = 44100;
const DEFAULT_VOLUME: u8 = 50;
/// A short fade at each end, so a tone doesn't start or stop with a click.
const FADE_SAMPLES: usize = 220;

pub const TICK: (u32, u32) = (880, 120);
pub const GO: (u32, u32) = (1760, 400);

/// A mono 16-bit WAV file of a sine tone. Volume is 0 to 100, and sounds about even across that.
fn tone(frequency: u32, milliseconds: u32, volume: u8) -> Vec<u8> {
    let samples = (SAMPLE_RATE * milliseconds / 1000) as usize;
    let data_len = samples as u32 * 2;
    let mut wav = Vec::with_capacity(44 + samples * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    let share = f32::from(volume.min(100)) / 100.0;
    let amplitude = share * share * f32::from(i16::MAX);
    for i in 0..samples {
        let fade = (i.min(samples - 1 - i) as f32 / FADE_SAMPLES as f32).min(1.0);
        let phase = i as f32 * frequency as f32 / SAMPLE_RATE as f32 * std::f32::consts::TAU;
        wav.extend_from_slice(&((phase.sin() * amplitude * fade) as i16).to_le_bytes());
    }
    wav
}

/// Plays a tone, returning when it ended.
#[cfg(windows)]
pub fn play((frequency, milliseconds): (u32, u32), volume: u8) {
    let wav = tone(frequency, milliseconds, volume);
    let _ = unsafe { PlaySoundW(PCWSTR(wav.as_ptr().cast()), None, SND_MEMORY | SND_SYNC | SND_NODEFAULT) };
}

/// Plays a tone with whichever player the system has, returning when it ended.
#[cfg(not(windows))]
pub fn play((frequency, milliseconds): (u32, u32), volume: u8) {
    static PLAYS: AtomicU32 = AtomicU32::new(0);
    let number = PLAYS.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("dampe-tone-{}-{number}.wav", std::process::id()));
    if std::fs::write(&path, tone(frequency, milliseconds, volume)).is_err() {
        return;
    }
    for player in ["paplay", "pw-play", "aplay"] {
        let run = std::process::Command::new(player).arg(&path).stderr(std::process::Stdio::null()).status();
        if run.is_ok_and(|status| status.success()) {
            break;
        }
    }
    let _ = std::fs::remove_file(path);
}

fn settings_path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("settings.txt"))
}

fn parse(text: &str) -> (bool, u8) {
    let mut settings = (false, DEFAULT_VOLUME);
    for line in text.lines() {
        match line.split_once('=') {
            Some(("sound", value)) => settings.0 = value.trim() == "on",
            Some(("volume", value)) => {
                settings.1 = value.trim().parse::<u32>().map_or(DEFAULT_VOLUME, |volume| volume.min(100) as u8)
            }
            _ => {}
        }
    }
    settings
}

/// Whether the cue makes a sound, and how loud.
pub struct Sound {
    enabled: AtomicBool,
    volume: AtomicU8,
}

impl Sound {
    /// As saved last time, or off.
    pub fn load() -> Self {
        let text = settings_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
        let (enabled, volume) = parse(&text);
        Self { enabled: AtomicBool::new(enabled), volume: AtomicU8::new(volume) }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn volume(&self) -> u8 {
        self.volume.load(Ordering::Relaxed)
    }

    /// Turns it on or off, and remembers that.
    pub fn set(&self, enabled: bool, volume: u8) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.volume.store(volume.min(100), Ordering::Relaxed);
        self.save();
    }

    /// Turns it on for this run only.
    pub fn force_on(&self) {
        self.enabled.store(true, Ordering::Relaxed);
    }

    fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let text = format!("sound={}\nvolume={}\n", if self.enabled() { "on" } else { "off" }, self.volume());
        let _ = std::fs::write(path, text);
    }

    /// Plays a tone if the sound is on.
    pub fn cue(&self, tone: (u32, u32)) {
        if self.enabled() {
            play(tone, self.volume());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(wav: &[u8]) -> i32 {
        wav[44..].chunks_exact(2).map(|s| i32::from(i16::from_le_bytes([s[0], s[1]])).abs()).max().unwrap_or(0)
    }

    #[test]
    fn a_tone_is_a_wav_of_the_right_length() {
        let wav = tone(880, 100, 100);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 4410 * 2);
        assert_eq!(u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]) as usize, wav.len() - 8);
    }

    #[test]
    fn volume_scales_it_and_zero_is_silent() {
        assert_eq!(peak(&tone(880, 100, 0)), 0);
        let (half, full) = (peak(&tone(880, 100, 50)), peak(&tone(880, 100, 100)));
        assert!(full > 30000, "{full}");
        assert!(half > full / 5 && half < full / 3, "{half} against {full}");
    }

    #[test]
    fn off_by_default_and_saved_values_are_read() {
        assert_eq!(parse(""), (false, DEFAULT_VOLUME));
        assert_eq!(parse("sound=on\nvolume=30\n"), (true, 30));
        assert_eq!(parse("sound=off\nvolume=900\n"), (false, 100));
    }
}
