//! Screen capture.
//!
//! On Windows this uses Windows Graphics Capture: frames stay on the GPU as
//! D3D11 textures, so the encoder can read them without a CPU copy. It also
//! works for virtual monitors, which extend mode will need.

#[cfg(windows)]
mod wgc;
#[cfg(windows)]
pub use wgc::{monitors, now, Capturer, Frame, Monitor};
