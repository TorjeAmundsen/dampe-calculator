//! Global hotkeys for saying Navi has gone or starting over, and for the counts of flutters, since the
//! game has the focus during a run, not this window. It only listens to the PC's keyboard, and nothing is ever sent to the game. On Linux the
//! key only works with the window focused, since Wayland has no global hotkeys to register.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(windows)]
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::time::Instant;

#[cfg(windows)]
use windows::Win32::Foundation::{LPARAM, WPARAM};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_HOTKEY,
};

/// Whether `listen` does anything here.
pub const AVAILABLE: bool = cfg!(windows);

const VK_F1: u32 = 0x70;

/// The key code for a name like `F8`.
pub fn parse_key(name: &str) -> Option<u32> {
    let number: u32 = name.strip_prefix(['F', 'f'])?.parse().ok()?;
    (1..=24).contains(&number).then(|| VK_F1 + number - 1)
}

/// What the window sees for a key named like `F8`, for when it has to listen itself.
pub fn window_key(name: &str) -> Option<char> {
    let number: u32 = name.strip_prefix(['F', 'f'])?.parse().ok()?;
    if (1..=24).contains(&number) { char::from_u32(0xF704 + number - 1) } else { None }
}

/// Sets `pressed` whenever `key` is pressed, from any program. Reports back through `failed` if
/// another program already has the key.
#[cfg(windows)]
pub fn listen(key: u32, pressed: Arc<AtomicBool>, failed: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        // A hotkey belongs to the thread that registers it, which then has to wait for its messages
        let modifiers = HOT_KEY_MODIFIERS(MOD_NOREPEAT.0);
        if unsafe { RegisterHotKey(None, 1, modifiers, key) }.is_err() {
            failed();
            return;
        }
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            if message.message == WM_HOTKEY {
                pressed.store(true, Ordering::Relaxed);
            }
        }
    });
}

#[cfg(not(windows))]
pub fn listen(_key: u32, _pressed: Arc<AtomicBool>, _failed: impl FnOnce() + Send + 'static) {}

/// The number keys for a flutter's count, heard from any program while on. While they're on, other
/// programs can't get those keys, which is why they can be turned off.
pub struct CountKeys {
    #[cfg(windows)]
    thread: u32,
}

/// Whether the number keys are heard from any program, from what's saved. On unless turned off.
fn count_keys_on(text: &str) -> bool {
    !text.lines().any(|line| line.trim() == "count_keys=off")
}

fn count_keys_path() -> Option<std::path::PathBuf> {
    Some(crate::config_dir()?.join("keys.txt"))
}

/// Whether the number keys were left on, last time they were changed.
pub fn load_count_keys_on() -> bool {
    count_keys_on(&count_keys_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default())
}

pub fn save_count_keys_on(on: bool) {
    let Some(path) = count_keys_path() else { return };
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let _ = std::fs::write(path, format!("count_keys={}\n", if on { "on" } else { "off" }));
}

/// The counts, and their keys' codes, which are their characters.
#[cfg(windows)]
const COUNT_KEYS: [(u8, u32); 2] = [(3, 0x33), (4, 0x34)];
#[cfg(windows)]
const SET_COUNT_KEYS: u32 = WM_APP + 1;

#[cfg(windows)]
impl CountKeys {
    /// Starts off. Presses go to `presses`, stamped as soon as they're known. `failed` hears about it
    /// when another program has one of the keys.
    pub fn new(presses: Sender<(u8, Instant)>, failed: impl Fn() + Send + 'static) -> Self {
        let (thread_sender, thread) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut message = MSG::default();
            // A thread only gets a message queue once it asks for messages, and it can't be sent any before
            let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
            let _ = thread_sender.send(unsafe { GetCurrentThreadId() });
            let unregister = || {
                for id in 0..COUNT_KEYS.len() {
                    let _ = unsafe { UnregisterHotKey(None, id as i32) };
                }
            };
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
                if message.message == WM_HOTKEY {
                    let Some(&(count, _)) = COUNT_KEYS.get(message.wParam.0) else { continue };
                    if presses.send((count, Instant::now())).is_err() {
                        return;
                    }
                } else if message.message == SET_COUNT_KEYS {
                    unregister();
                    if message.wParam.0 == 1 {
                        let modifiers = HOT_KEY_MODIFIERS(MOD_NOREPEAT.0);
                        let registered = COUNT_KEYS
                            .iter()
                            .enumerate()
                            .all(|(id, &(_, key))| unsafe { RegisterHotKey(None, id as i32, modifiers, key) }.is_ok());
                        if !registered {
                            unregister();
                            failed();
                        }
                    }
                }
            }
        });
        Self { thread: thread.recv().expect("the hotkey thread stopped") }
    }

    pub fn set_on(&self, on: bool) {
        let _ = unsafe { PostThreadMessageW(self.thread, SET_COUNT_KEYS, WPARAM(usize::from(on)), LPARAM(0)) };
    }
}

#[cfg(not(windows))]
impl CountKeys {
    pub fn new(_presses: Sender<(u8, Instant)>, _failed: impl Fn() + Send + 'static) -> Self {
        Self {}
    }

    pub fn set_on(&self, _on: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keys_are_the_ones_slint_sends() {
        assert_eq!(window_key("F1"), Some(char::from(slint::platform::Key::F1)));
        assert_eq!(window_key("F8"), Some(char::from(slint::platform::Key::F8)));
        assert_eq!(window_key("F24"), Some(char::from(slint::platform::Key::F24)));
        assert_eq!(window_key("F25"), None);
    }

    #[test]
    fn the_number_keys_stay_off_only_when_saved_off() {
        assert!(count_keys_on(""));
        assert!(count_keys_on("count_keys=on\n"));
        assert!(!count_keys_on("count_keys=off\n"));
    }

    #[test]
    fn keys_are_function_keys_only() {
        assert_eq!(parse_key("F1"), Some(0x70));
        assert_eq!(parse_key("f8"), Some(0x77));
        assert_eq!(parse_key("F24"), Some(0x87));
        assert_eq!(parse_key("F25"), None);
        assert_eq!(parse_key("F0"), None);
        assert_eq!(parse_key("A"), None);
    }
}
