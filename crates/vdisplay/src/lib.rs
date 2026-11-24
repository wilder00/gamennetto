//! Virtual monitor for extend mode.
//!
//! Windows only extends the desktop onto monitors it believes exist, so extend
//! mode needs a display driver that pretends a monitor is plugged in. We use
//! the open-source Virtual Display Driver (MIT licence, signed by SignPath
//! Foundation): <https://github.com/VirtualDrivers/Virtual-Display-Driver>.
//!
//! The driver always shows at least one monitor while it is installed. When
//! Gamennetto isn't extending, that monitor is kept *detached* from the
//! desktop (like "Disconnect this display" in Windows Settings), and the
//! detached layout is saved in Windows' display database, so it survives
//! reboots and crashes.

pub mod settings;

#[cfg(windows)]
mod device;
#[cfg(windows)]
mod layout;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod system;

#[cfg(windows)]
pub use device::{find_device, install_driver, uninstall_driver, DeviceInfo};
#[cfg(windows)]
pub use layout::{attach, detach, known_monitors, virtual_display, VirtualDisplay};
#[cfg(windows)]
pub use pipe::{driver_running, send_command};
#[cfg(windows)]
pub use system::*;
