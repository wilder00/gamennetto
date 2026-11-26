//! Keyboard capture for the viewer, the way virtual machine windows do it.
//!
//! While the viewer has "grabbed" the keyboard, a low-level keyboard hook
//! takes every key before Windows acts on it, so Alt+Tab, the Windows key,
//! Alt+F4 and so on go to the host instead of this PC. Ctrl+Alt (pressed
//! and released on their own) gives the keyboard back. Ctrl+Alt+Del can't be
//! taken: Windows always keeps it for itself.

use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC_EX};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, LLKHF_UP, MSG, WH_KEYBOARD_LL, WM_QUIT,
};

pub const SC_CTRL: u32 = 0x1D;
pub const SC_RCTRL: u32 = 0xE01D;
pub const SC_ALT: u32 = 0x38;
pub const SC_RALT: u32 = 0xE038;
pub const SC_Q: u32 = 0x10;
pub const SC_S: u32 = 0x1F;
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
    /// Ctrl+Alt+S: next picture size (fit, fill, actual size).
    CycleScale,
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
    /// Like `on_key`, but `altgr_ctrl` marks the Ctrl Windows adds for AltGr:
    /// it is sent to the host (which needs it to type AltGr characters) but
    /// never counts as Ctrl for our Ctrl+Alt shortcuts, or AltGr would hand
    /// control back and AltGr+Q (@ on Latin American keyboards) would close
    /// the viewer.
    pub fn handle(&mut self, scancode: u32, down: bool, altgr_ctrl: bool) -> Vec<GrabEvent> {
        if altgr_ctrl {
            return vec![GrabEvent::Key { scancode, down }];
        }
        self.on_key(scancode, down)
    }

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
            SC_S if self.ctrl && self.alt => {
                if down {
                    vec![GrabEvent::CycleScale]
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
///
/// It runs on a thread of its own that only waits for Windows' messages.
/// Windows calls a low-level hook through the message queue of the thread
/// that installed it, and skips the hook (or removes it for good) if that
/// thread is slow to answer, so it must never share a thread with drawing
/// or decoding video.
pub struct KeyboardHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyboardHook {
    pub fn install(sink: impl Fn(GrabEvent) + Send + 'static) -> Result<Self> {
        *STATE.lock().unwrap() = Some((Shortcuts::default(), Box::new(sink)));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("keyboard-hook".into()).spawn(move || unsafe {
            let hook = GetModuleHandleW(None)
                .and_then(|module| SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(module.into()), 0));
            let hook = match hook {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(anyhow!(e).context("installing the keyboard hook")));
                    return;
                }
            };
            let _ = tx.send(Ok(GetCurrentThreadId()));
            // Waiting here is what lets Windows call the hook. WM_QUIT ends it.
            let mut msg = MSG::default();
            let ended = loop {
                let r = GetMessageW(&mut msg, None, 0, 0).0;
                if r == 0 || r == -1 {
                    break r;
                }
            };
            if DEBUG.load(Ordering::Relaxed) {
                eprintln!("[keys] hook thread ended (GetMessage returned {ended})");
            }
            let _ = UnhookWindowsHookEx(hook);
        })?;
        let thread_id = rx.recv().context("the keyboard hook thread stopped")??;
        Ok(Self { thread_id, thread: Some(thread) })
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
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        *STATE.lock().unwrap() = None;
    }
}

/// Keys whose scan code needs the 0xE0 "extended" prefix. Real keyboards
/// mark them, but some tools, remote sessions and on-screen keyboards don't;
/// without the prefix the host would press a different key (the Windows
/// key's 0x5B alone isn't the Windows key).
fn always_extended(vk: u32) -> bool {
    matches!(
        vk,
        0x5B | 0x5C | 0x5D // left/right Windows, menu key
        | 0x21
            ..=0x28      // Page Up/Down, End, Home, arrows
        | 0x2D | 0x2E      // Insert, Delete
        | 0xA3 | 0xA5      // right Ctrl, right Alt
        | 0x6F // numpad /
    )
}

const VK_PAUSE: u32 = 0x13;
const VK_SNAPSHOT: u32 = 0x2C;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_NUMLOCK: u32 = 0x90;

/// The Ctrl that Windows adds when AltGr is pressed (Spanish, German...
/// keyboards) has this scan code in the hook.
const ALTGR_CTRL_SCAN: u32 = 0x21D;

/// The scan code the host must press for a key seen by the hook, checked
/// against Windows' own table (MapVirtualKey):
/// - right Shift and Num Lock are marked "extended" in the hook, but are
///   plain 0x36 and 0x45; with 0xE0 they are no key at all (right Shift + 1
///   typed "1" instead of "!");
/// - Pause shows up as 0x45, which is Num Lock; its real code is E1 1D
///   (sent as `gamennetto_net::SCAN_PAUSE`);
/// - Alt + Print Screen is plain 0x54; Print Screen alone is E0 37.
fn host_scancode(vk: u32, scan: u32, extended_flag: bool) -> u32 {
    let scan = scan & 0xFF;
    match vk {
        VK_PAUSE => gamennetto_net::SCAN_PAUSE,
        VK_RSHIFT | VK_NUMLOCK => scan,
        VK_SNAPSHOT if scan == 0x54 => 0x54,
        VK_SNAPSHOT => 0xE037,
        _ if extended_flag || always_extended(vk) => scan | 0xE000,
        _ => scan,
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let active = ACTIVE.load(Ordering::SeqCst);
    if code >= 0 && DEBUG.load(Ordering::Relaxed) {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        eprintln!(
            "[keys] hook saw vk 0x{:02x} scan 0x{:02x} flags 0x{:x} extra 0x{:x} (grabbed: {active})",
            k.vkCode, k.scanCode, k.flags.0, k.dwExtraInfo
        );
    }
    if code >= 0 && active {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Keys typed by our own host (on the same PC) are left alone, so they
        // can't loop back. Keys from other programs that type (keyboard
        // tools, remote sessions) are taken like real ones.
        let ours = k.dwExtraInfo == gamennetto_input::INPUT_TAG;
        if !ours {
            let mut scan = k.scanCode;
            if scan == 0 {
                scan = MapVirtualKeyW(k.vkCode, MAPVK_VK_TO_VSC_EX) & 0xFF;
            }
            let sc = host_scancode(k.vkCode, scan, k.flags.0 & LLKHF_EXTENDED.0 != 0);
            // AltGr's added Ctrl goes to the host (it needs it for AltGr),
            // but isn't Ctrl for our own Ctrl+Alt shortcuts.
            let altgr_ctrl = k.vkCode == VK_LCONTROL && k.scanCode == ALTGR_CTRL_SCAN;
            let down = k.flags.0 & LLKHF_UP.0 == 0;
            if let Ok(mut guard) = STATE.try_lock() {
                if let Some((shortcuts, sink)) = guard.as_mut() {
                    for ev in shortcuts.handle(sc, down, altgr_ctrl) {
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

    /// Types the Windows key and Alt+Tab for real and checks the hook takes
    /// them (so the Start menu must NOT open). Run by hand:
    /// `cargo test -p gamennetto hook_takes -- --ignored`
    #[test]
    #[ignore = "presses real keys on this PC"]
    fn hook_takes_windows_key_and_alt_tab() {
        use std::sync::Arc;
        use windows::Win32::UI::Input::KeyboardAndMouse::*;

        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let hook = KeyboardHook::install(move |e| g.lock().unwrap().push(e)).unwrap();
        hook.set_active(true);

        let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let keys = [
            key(VK_LWIN, false),
            key(VK_LWIN, true),
            key(VK_MENU, false),
            key(VK_TAB, false),
            key(VK_TAB, true),
            key(VK_MENU, true),
            key(VK_RSHIFT, false),
            key(VK_RSHIFT, true),
        ];
        unsafe { SendInput(&keys, std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(std::time::Duration::from_millis(500));
        hook.set_active(false);
        drop(hook);

        let got = got.lock().unwrap().clone();
        let downs: Vec<u32> = got
            .iter()
            .filter_map(|e| if let Key { scancode, down: true } = e { Some(*scancode) } else { None })
            .collect();
        assert!(downs.contains(&0xE05B), "Windows key not taken: {got:?}");
        assert!(downs.contains(&SC_ALT) && downs.contains(&0x0F), "Alt+Tab not taken: {got:?}");
        assert!(downs.contains(&0x36), "right Shift not sent as 0x36: {got:?}");
    }

    #[test]
    fn scan_codes_sent_to_the_host() {
        // Right Shift: Windows says "extended", but it is plain 0x36.
        assert_eq!(host_scancode(0xA1, 0x36, true), 0x36);
        assert_eq!(host_scancode(0xA0, 0x2A, false), 0x2A); // left Shift
        assert_eq!(host_scancode(0x90, 0x45, true), 0x45); // Num Lock
        assert_eq!(host_scancode(0x13, 0x45, false), gamennetto_net::SCAN_PAUSE); // Pause, not Num Lock
        assert_eq!(host_scancode(0x2C, 0x37, true), 0xE037); // Print Screen
        assert_eq!(host_scancode(0x2C, 0x54, false), 0x54); // Alt + Print Screen
        assert_eq!(host_scancode(0x0D, 0x1C, true), 0xE01C); // numpad Enter
        assert_eq!(host_scancode(0x67, 0x47, false), 0x47); // numpad 7
        assert_eq!(host_scancode(0xA3, 0x1D, true), 0xE01D); // right Ctrl
        assert_eq!(host_scancode(0x5B, 0x5B, false), 0xE05B); // Windows key, even unmarked
        assert_eq!(host_scancode(0x26, 0x48, true), 0xE048); // arrow up
        assert_eq!(host_scancode(0x31, 0x02, false), 0x02); // "1"
    }

    #[test]
    fn altgr_is_not_ctrl_alt() {
        let mut s = Shortcuts::default();
        // AltGr down: Windows' added Ctrl, then right Alt.
        assert_eq!(s.handle(SC_CTRL, true, true), vec![Key { scancode: SC_CTRL, down: true }]);
        assert_eq!(s.handle(SC_RALT, true, false), vec![Key { scancode: SC_RALT, down: true }]);
        // AltGr+Q types @ on the host instead of closing the viewer.
        assert_eq!(s.handle(SC_Q, true, false), vec![Key { scancode: SC_Q, down: true }]);
        assert_eq!(s.handle(SC_Q, false, false), vec![Key { scancode: SC_Q, down: false }]);
        // Letting go of AltGr doesn't hand control back.
        assert_eq!(s.handle(SC_CTRL, false, true), vec![Key { scancode: SC_CTRL, down: false }]);
        assert_eq!(s.handle(SC_RALT, false, false), vec![Key { scancode: SC_RALT, down: false }]);
        // A real Ctrl+Alt still does.
        s.handle(SC_CTRL, true, false);
        s.handle(SC_ALT, true, false);
        assert_eq!(s.handle(SC_ALT, false, false), vec![Key { scancode: SC_ALT, down: false }, Release]);
    }

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
        assert_eq!(s.on_key(SC_S, true), vec![CycleScale]);
        assert_eq!(s.on_key(SC_S, false), vec![]);
        let mut s = Shortcuts::default();
        assert_eq!(s.on_key(SC_F11, true), vec![ToggleFullscreen]);
        assert_eq!(s.on_key(SC_F11, false), vec![]);
    }
}
