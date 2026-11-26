//! Applies a viewer's mouse and keyboard to the host.
//!
//! Limits (Windows rules, not ours): input can't reach windows running as
//! administrator unless Gamennetto also runs as administrator, and it can't
//! reach secure screens (Ctrl+Alt+Del, UAC prompts, the login screen).

use gamennetto_net::{InputEvent, MouseButton};

/// Put in the "extra info" of every key and mouse event the host types, so
/// a viewer on the same PC can tell them apart from real typing and never
/// sends them back (which would loop). Other programs' typed keys (keyboard
/// tools, remote sessions) don't carry it and are treated as real.
pub const INPUT_TAG: usize = 0x4741_4D45; // "GAME"

/// The shared monitor's rectangle on the host desktop, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl ScreenRect {
    /// Maps a 0..1 position to a desktop pixel.
    pub fn to_pixel(&self, x: f32, y: f32) -> (i32, i32) {
        let x = x.clamp(0.0, 1.0);
        let y = y.clamp(0.0, 1.0);
        (
            self.left + (x * (self.width.saturating_sub(1)) as f32).round() as i32,
            self.top + (y * (self.height.saturating_sub(1)) as f32).round() as i32,
        )
    }
}

/// Keys and mouse buttons that are currently pressed, so they can all be
/// released if the viewer goes away mid-press (otherwise e.g. Alt would stay
/// "stuck" down on the host).
#[derive(Debug, Default)]
pub struct Held {
    keys: Vec<u32>,
    buttons: Vec<MouseButton>,
}

impl Held {
    pub fn track(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Key { scancode, down } => {
                self.keys.retain(|&k| k != scancode);
                if down {
                    self.keys.push(scancode);
                }
            }
            InputEvent::MouseButton { button, down } => {
                self.buttons.retain(|&b| b != button);
                if down {
                    self.buttons.push(button);
                }
            }
            _ => {}
        }
    }

    /// "Release" events for everything still pressed, newest first; clears the list.
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let keys = self.keys.drain(..).rev().map(|scancode| InputEvent::Key { scancode, down: false });
        let buttons = self.buttons.drain(..).rev().map(|button| InputEvent::MouseButton { button, down: false });
        keys.chain(buttons).collect()
    }
}

pub struct Injector {
    screen: ScreenRect,
    held: std::sync::Mutex<Held>,
}

impl Injector {
    pub fn new(screen: ScreenRect) -> Self {
        Self { screen, held: Default::default() }
    }

    pub fn inject(&self, ev: InputEvent) {
        self.held.lock().unwrap().track(&ev);
        #[cfg(windows)]
        win::inject(&self.screen, ev);
    }

    /// Releases every key and button the viewer left pressed.
    pub fn release_all(&self) {
        let events = self.held.lock().unwrap().release_all();
        for _ev in events {
            #[cfg(windows)]
            win::inject(&self.screen, _ev);
        }
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[cfg(windows)]
mod win {
    use super::ScreenRect;
    use gamennetto_net::{InputEvent, MouseButton};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    const WHEEL_DELTA: f32 = 120.0;

    fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: super::INPUT_TAG,
                },
            },
        }
    }

    fn send(inputs: &[INPUT]) {
        unsafe {
            SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }

    pub fn inject(screen: &ScreenRect, ev: InputEvent) {
        match ev {
            InputEvent::MouseMove { x, y } => {
                let (px, py) = screen.to_pixel(x, y);
                // Absolute coordinates are 0..65535 across the whole virtual desktop.
                let (vx, vy, vw, vh) = unsafe {
                    (
                        GetSystemMetrics(SM_XVIRTUALSCREEN),
                        GetSystemMetrics(SM_YVIRTUALSCREEN),
                        GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                        GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
                    )
                };
                let dx = ((px - vx) as i64 * 65535 / (vw - 1) as i64) as i32;
                let dy = ((py - vy) as i64 * 65535 / (vh - 1) as i64) as i32;
                send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, dx, dy, 0)]);
            }
            InputEvent::MouseButton { button, down } => {
                let flags = match (button, down) {
                    (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                    (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                    (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                    (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                    (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                    (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
                };
                send(&[mouse(flags, 0, 0, 0)]);
            }
            InputEvent::Wheel { x, y } => {
                if y != 0.0 {
                    send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, (y * WHEEL_DELTA) as i32)]);
                }
                if x != 0.0 {
                    send(&[mouse(MOUSEEVENTF_HWHEEL, 0, 0, (x * WHEEL_DELTA) as i32)]);
                }
            }
            InputEvent::Key { scancode, down } => {
                // Pause has no one-number scan code: press it by key name.
                let by_name = scancode == gamennetto_net::SCAN_PAUSE;
                let mut flags = if by_name { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_SCANCODE };
                if !by_name && scancode & 0xFF00 == 0xE000 {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                if !down {
                    flags |= KEYEVENTF_KEYUP;
                }
                send(&[INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: if by_name { VK_PAUSE } else { VIRTUAL_KEY(0) },
                            wScan: if by_name { 0 } else { (scancode & 0xFF) as u16 },
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: super::INPUT_TAG,
                        },
                    },
                }]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_corners_to_monitor_edges() {
        // A second monitor to the left of the main one.
        let r = ScreenRect { left: -1920, top: 0, width: 1920, height: 1080 };
        assert_eq!(r.to_pixel(0.0, 0.0), (-1920, 0));
        assert_eq!(r.to_pixel(1.0, 1.0), (-1, 1079));
        assert_eq!(r.to_pixel(2.0, -1.0), (-1, 0)); // clamped
    }

    #[test]
    fn releases_what_is_still_pressed() {
        let mut h = Held::default();
        h.track(&InputEvent::Key { scancode: 0x38, down: true }); // Alt
        h.track(&InputEvent::Key { scancode: 0x0F, down: true }); // Tab
        h.track(&InputEvent::Key { scancode: 0x0F, down: false });
        h.track(&InputEvent::MouseButton { button: MouseButton::Left, down: true });
        h.track(&InputEvent::Key { scancode: 0x38, down: true }); // key repeat
        assert_eq!(
            h.release_all(),
            vec![
                InputEvent::Key { scancode: 0x38, down: false },
                InputEvent::MouseButton { button: MouseButton::Left, down: false },
            ]
        );
        assert!(h.release_all().is_empty());
    }
}
