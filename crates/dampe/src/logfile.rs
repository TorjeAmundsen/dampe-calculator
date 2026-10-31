//! The run's log, in a file, since the published program has no console. Each line also goes to the
//! terminal, if it was started from one. The run before this one is kept, for reporting a problem
//! after restarting.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<File>> = OnceLock::new();

/// Where the log goes.
pub fn path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("dampe.log"))
}

/// Starts the log for this run, and has panics end up in it.
pub fn start() {
    let Some(path) = path() else { return };
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let _ = std::fs::rename(&path, path.with_file_name("dampe.previous.log"));
    if let Ok(file) = File::create(&path) {
        let _ = FILE.set(Mutex::new(file));
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        to_file(format_args!("Crashed: {info}"));
        default(info);
    }));
}

/// Writes a line to the terminal, if there is one, and the file.
pub fn line(text: impl std::fmt::Display) {
    println!("{text}");
    to_file(text);
}

/// Writes a line to the file, straight away so it's there if the program dies.
pub fn to_file(text: impl std::fmt::Display) {
    if let Some(file) = FILE.get()
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "{text}");
        let _ = file.flush();
    }
}
