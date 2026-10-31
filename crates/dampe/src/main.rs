//! Reads Dampe's blinks from video and says when to press A for the Heart Piece. Video in, a cue
//! out: it never sends any input to the game.

#![cfg_attr(feature = "windowed", windows_subsystem = "windows")]

#[cfg(windows)]
mod capture;
#[cfg(not(windows))]
#[path = "capture_v4l2.rs"]
mod capture;
mod diag;
mod gui;
mod hotkey;
mod live;
mod logfile;
mod replay;
mod sound;
mod timing;

const USAGE: &str = "usage:
  dampe                                          the same as dampe live
  dampe live [--device <name>] [--platform n64|vc] [--feed-delay-ms <ms>] [--no-window] [--no-sound] [--navi-key <F1-F24>]
                                                 read a device and cue the A press
  dampe diag [--device <name>] [--seconds <n>]   measure what the feed delivers
  dampe devices                                  list video devices
  dampe replay <video> [--truth <file>]          test on a recording, scored against RNG watch values
  dampe replay <video> --session [--navi-gone-at <video frame>]   run a recording the way live runs it";

/// The folder for settings and the log.
fn config_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let config = std::path::PathBuf::from(std::env::var_os("APPDATA")?);
    #[cfg(not(windows))]
    let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
        Some(path) => std::path::PathBuf::from(path),
        None => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(config.join("dampe-rng"))
}

/// It was started from a terminal, so errors can go there.
#[cfg(all(windows, feature = "windowed"))]
static IN_TERMINAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Shows an error to someone with no console to read it in. `false` if there's a console for it.
fn show_error(error: &str) -> bool {
    #[cfg(all(windows, feature = "windowed"))]
    if !IN_TERMINAL.load(std::sync::atomic::Ordering::Relaxed) {
        use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        use windows::core::{HSTRING, w};
        unsafe { MessageBoxW(None, &HSTRING::from(error), w!("Dampe RNG"), MB_OK | MB_ICONERROR) };
        return true;
    }
    let _ = error;
    false
}

fn main() {
    // With no console of its own, it can still print to the terminal it was started from
    #[cfg(all(windows, feature = "windowed"))]
    {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok();
        IN_TERMINAL.store(attached, std::sync::atomic::Ordering::Relaxed);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let double_clicked = args.is_empty();
    let result = match args.first().map(String::as_str) {
        None => live::run(&[]),
        Some("replay") => replay::run(&args[1..]),
        Some("live") => live::run(&args[1..]),
        Some("diag") => diag::run(&args[1..]),
        Some("devices") => capture::list_devices().map(|devices| devices.iter().for_each(|d| println!("{d}"))),
        _ => Err(USAGE.to_owned()),
    };
    if let Err(error) = result {
        {
            logfile::to_file(&error);
            if show_error(&error) {
                std::process::exit(1);
            }
        }
        eprintln!("{error}");
        // Started from Explorer, the console closes with the program, and the error with it
        if double_clicked && cfg!(all(windows, not(feature = "windowed"))) {
            eprintln!("Press Enter to close.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        std::process::exit(1);
    }
}
