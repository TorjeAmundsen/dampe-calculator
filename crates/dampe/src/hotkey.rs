//! A global hotkey for saying Navi has gone, since the game has the focus during a run, not this
//! window. It only listens to the PC's keyboard, and nothing is ever sent to the game. On Linux the
//! key only works with the window focused, since Wayland has no global hotkeys to register.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(windows)]
use std::sync::atomic::Ordering;

#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

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
    fn keys_are_function_keys_only() {
        assert_eq!(parse_key("F1"), Some(0x70));
        assert_eq!(parse_key("f8"), Some(0x77));
        assert_eq!(parse_key("F24"), Some(0x87));
        assert_eq!(parse_key("F25"), None);
        assert_eq!(parse_key("F0"), None);
        assert_eq!(parse_key("A"), None);
    }
}
