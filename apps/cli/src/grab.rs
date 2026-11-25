//! Keyboard capture for the viewer, the way virtual machine windows do it.
//!
//! While the viewer has "grabbed" the keyboard, a low-level keyboard hook
//! takes every key before Windows acts on it, so Alt+Tab, the Windows key,
//! Alt+F4 and so on go to the host instead of this PC. Ctrl+Alt (pressed
//! and released on their own) gives the keyboard back. Ctrl+Alt+Del can't be
//! taken: Windows always keeps it for itself.

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC_EX};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED,
    LLKHF_UP, WH_KEYBOARD_LL,
};

pub const SC_CTRL: u32 = 0x1D;
pub const SC_RCTRL: u32 = 0xE01D;
pub const SC_ALT: u32 = 0x38;
pub const SC_RALT: u32 = 0xE038;
pub const SC_Q: u32 = 0x10;
pub const SC_F11: u32 = 0x57;

/// What the hook tells the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabEvent {
    /// Send this key to the host.
    Key { scancode: u32, down: bool },
    /// Ctrl+Alt: give the keyboard and mouse back to this PC.
    Release,
    /// Ctrl+Alt+Q: close the viewer.
    Quit,
    /// F11: full screen on/off.
    ToggleFullscreen,
}

/// Decides what each key means while grabbed. Kept apart from the hook so
/// it can be tested.
#[derive(Debug, Default)]
pub struct Shortcuts {
    ctrl: bool,
    alt: bool,
    /// Ctrl and Alt went down together and nothing else was pressed since.
    only_ctrl_alt: bool,
}

impl Shortcuts {
    /// Returns the events for one key press/release, in order.
    pub fn on_key(&mut self, scancode: u32, down: bool) -> Vec<GrabEvent> {
        let is_ctrl = scancode == SC_CTRL || scancode == SC_RCTRL;
        let is_alt = scancode == SC_ALT || scancode == SC_RALT;
        let key = GrabEvent::Key { scancode, down };
        if is_ctrl || is_alt {
            if is_ctrl {
                self.ctrl = down;
            } else {
                self.alt = down;
            }
            if down && self.ctrl && self.alt {
                self.only_ctrl_alt = true;
            }
            if !down && std::mem::take(&mut self.only_ctrl_alt) {
                return vec![key, GrabEvent::Release];
            }
            return vec![key];
        }
        if down {
            self.only_ctrl_alt = false;
        }
        match scancode {
            SC_F11 => {
                if down {
                    vec![GrabEvent::ToggleFullscreen]
                } else {
                    vec![]
                }
            }
            SC_Q if self.ctrl && self.alt => {
                if down {
                    vec![GrabEvent::Quit]
                } else {
                    vec![]
                }
            }
            _ => vec![key],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

type Sink = Box<dyn Fn(GrabEvent) + Send>;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static DEBUG: AtomicBool = AtomicBool::new(false);

/// Print what the hook does with each key (for `GAMENNETTO_DEBUG=1`).
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}
static STATE: Mutex<Option<(Shortcuts, Sink)>> = Mutex::new(None);

/// The installed hook. It does nothing until `set_active(true)`.
pub struct KeyboardHook(HHOOK);

impl KeyboardHook {
    /// Must be called on the thread that runs the window's message loop.
    pub fn install(sink: impl Fn(GrabEvent) + Send + 'static) -> Result<Self> {
        *STATE.lock().unwrap() = Some((Shortcuts::default(), Box::new(sink)));
        let module = unsafe { GetModuleHandleW(None)? };
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(module.into()), 0) }
            .context("installing the keyboard hook")?;
        Ok(Self(hook))
    }

    pub fn set_active(&self, on: bool) {
        ACTIVE.store(on, Ordering::SeqCst);
        if let Some((s, _)) = STATE.lock().unwrap().as_mut() {
            s.reset();
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::SeqCst);
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
        *STATE.lock().unwrap() = None;
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ACTIVE.load(Ordering::SeqCst) {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let injected = k.flags.0 & LLKHF_INJECTED.0 != 0;
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!(
                "[keys] hook saw vk 0x{:02x} scan 0x{:02x} flags 0x{:x}{}",
                k.vkCode,
                k.scanCode,
                k.flags.0,
                if injected { " (typed by a program: ignored)" } else { "" }
            );
        }
        // Keys typed by programs (including our own host on the same PC)
        // are left alone, so they can't loop back.
        if !injected {
            let mut sc = k.scanCode;
            if sc == 0 {
                sc = MapVirtualKeyW(k.vkCode, MAPVK_VK_TO_VSC_EX) & 0xFF;
            }
            if k.flags.0 & LLKHF_EXTENDED.0 != 0 {
                sc |= 0xE000;
            }
            let down = k.flags.0 & LLKHF_UP.0 == 0;
            if let Ok(mut guard) = STATE.try_lock() {
                if let Some((shortcuts, sink)) = guard.as_mut() {
                    for ev in shortcuts.on_key(sc, down) {
                        sink(ev);
                    }
                    // Swallow the key: this PC must not act on it.
                    return LRESULT(1);
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use GrabEvent::*;

    #[test]
    fn ctrl_alt_alone_releases() {
        let mut s = Shortcuts::default();
        assert_eq!(s.on_key(SC_CTRL, true), vec![Key { scancode: SC_CTRL, down: true }]);
        assert_eq!(s.on_key(SC_ALT, true), vec![Key { scancode: SC_ALT, down: true }]);
        assert_eq!(s.on_key(SC_ALT, false), vec![Key { scancode: SC_ALT, down: false }, Release]);
        // The other key's release is still sent, so nothing stays pressed.
        assert_eq!(s.on_key(SC_CTRL, false), vec![Key { scancode: SC_CTRL, down: false }]);
    }

    #[test]
    fn ctrl_alt_with_another_key_goes_to_the_host() {
        let mut s = Shortcuts::default();
        s.on_key(SC_CTRL, true);
        s.on_key(SC_ALT, true);
        assert_eq!(s.on_key(0x14, true), vec![Key { scancode: 0x14, down: true }]); // T
        s.on_key(0x14, false);
        assert_eq!(s.on_key(SC_ALT, false), vec![Key { scancode: SC_ALT, down: false }]);
    }

    #[test]
    fn alt_tab_goes_to_the_host() {
        let mut s = Shortcuts::default();
        assert_eq!(s.on_key(SC_ALT, true), vec![Key { scancode: SC_ALT, down: true }]);
        assert_eq!(s.on_key(0x0F, true), vec![Key { scancode: 0x0F, down: true }]);
        assert_eq!(s.on_key(0x0F, false), vec![Key { scancode: 0x0F, down: false }]);
        assert_eq!(s.on_key(SC_ALT, false), vec![Key { scancode: SC_ALT, down: false }]);
    }

    #[test]
    fn ctrl_alt_q_quits_and_f11_is_local() {
        let mut s = Shortcuts::default();
        s.on_key(SC_RCTRL, true);
        s.on_key(SC_ALT, true);
        assert_eq!(s.on_key(SC_Q, true), vec![Quit]);
        assert_eq!(s.on_key(SC_Q, false), vec![]);
        let mut s = Shortcuts::default();
        assert_eq!(s.on_key(SC_F11, true), vec![ToggleFullscreen]);
        assert_eq!(s.on_key(SC_F11, false), vec![]);
    }
}
