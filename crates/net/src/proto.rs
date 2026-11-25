//! Messages and how they are written on the wire.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Viewer -> host, on the control stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewerMsg {
    Hello {
        version: u32,
        name: String,
        /// The viewer's screen size in pixels. In extend mode the host makes
        /// its virtual monitor this size, so the picture fills the screen.
        #[serde(default)]
        screen: Option<(u32, u32)>,
        /// The viewer doesn't know this host yet and wants to pair, even if
        /// the host already knows the viewer.
        #[serde(default)]
        pair: bool,
    },
    /// The viewer lost data or just joined: send a full picture.
    RequestKeyframe,
    /// `viewer_us` is echoed back in `Pong`, used to sync clocks.
    Ping {
        viewer_us: u64,
    },
    Input(InputEvent),
    // Pairing (see pairing.rs).
    PairByApproval,
    PairByCode {
        spake: Vec<u8>,
    },
    PairConfirm {
        mac: Vec<u8>,
    },
}

/// Host -> viewer, on the control stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMsg {
    Welcome {
        version: u32,
        name: String,
        width: u32,
        height: u32,
        fps: u32,
        /// True if the host allows this viewer to use mouse and keyboard.
        control_allowed: bool,
        /// True if this is an extra (virtual) monitor rather than a copy.
        #[serde(default)]
        extend: bool,
    },
    Pong {
        viewer_us: u64,
        host_us: u64,
    },
    /// Connection refused (e.g. another viewer is already connected).
    Busy,
    /// The host could not start sharing; the message says why.
    Error {
        message: String,
    },
    // Pairing (see pairing.rs).
    /// This viewer isn't approved yet; these are the ways to approve it.
    PairingRequired {
        by_code: bool,
        by_approval: bool,
    },
    Paired,
    PairRefused {
        reason: String,
    },
    PairSpake {
        spake: Vec<u8>,
    },
    PairConfirm {
        mac: Vec<u8>,
    },
}

/// Input sent from the viewer. Positions are 0.0..=1.0 across the shared
/// screen, so they work whatever the two screen sizes are.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum InputEvent {
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Wheel steps; 1.0 = one notch. Positive y scrolls up, positive x right.
    Wheel {
        x: f32,
        y: f32,
    },
    /// Keyboard scan code as Windows reports it (0xE0xx for extended keys).
    Key {
        scancode: u32,
        down: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Control messages: 4-byte little-endian length, then JSON.
pub async fn write_msg<T: Serialize>(s: &mut quinn::SendStream, msg: &T) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    s.write_all(&(body.len() as u32).to_le_bytes()).await?;
    s.write_all(&body).await?;
    Ok(())
}

pub async fn read_msg<T: for<'de> Deserialize<'de>>(s: &mut quinn::RecvStream) -> Result<T> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await.context("connection closed")?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 64 * 1024 {
        bail!("control message too large ({len} bytes)");
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

/// One encoded video frame on the video stream.
#[derive(Debug, Clone)]
pub struct VideoPacket {
    pub seq: u32,
    /// Capture time on the host clock, microseconds.
    pub capture_us: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

pub const VIDEO_HEADER_LEN: usize = 4 + 8 + 1 + 4;
const MAX_FRAME: usize = 32 * 1024 * 1024;

impl VideoPacket {
    pub fn header(&self) -> [u8; VIDEO_HEADER_LEN] {
        let mut h = [0u8; VIDEO_HEADER_LEN];
        h[0..4].copy_from_slice(&self.seq.to_le_bytes());
        h[4..12].copy_from_slice(&self.capture_us.to_le_bytes());
        h[12] = self.keyframe as u8;
        h[13..17].copy_from_slice(&(self.data.len() as u32).to_le_bytes());
        h
    }

    /// Returns (seq, capture_us, keyframe, data length).
    pub fn parse_header(h: &[u8; VIDEO_HEADER_LEN]) -> Result<(u32, u64, bool, usize)> {
        let seq = u32::from_le_bytes(h[0..4].try_into()?);
        let capture_us = u64::from_le_bytes(h[4..12].try_into()?);
        let keyframe = h[12] != 0;
        let len = u32::from_le_bytes(h[13..17].try_into()?) as usize;
        if len > MAX_FRAME {
            bail!("video frame too large ({len} bytes)");
        }
        Ok((seq, capture_us, keyframe, len))
    }

    pub async fn write(&self, s: &mut quinn::SendStream) -> Result<()> {
        s.write_all(&self.header()).await?;
        s.write_all(&self.data).await?;
        Ok(())
    }

    pub async fn read(s: &mut quinn::RecvStream) -> Result<Self> {
        let mut h = [0u8; VIDEO_HEADER_LEN];
        s.read_exact(&mut h).await.context("video stream closed")?;
        let (seq, capture_us, keyframe, len) = Self::parse_header(&h)?;
        let mut data = vec![0u8; len];
        s.read_exact(&mut data).await?;
        Ok(Self { seq, capture_us, keyframe, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_header_roundtrip() {
        let p = VideoPacket { seq: 7, capture_us: 123_456_789, keyframe: true, data: vec![1, 2, 3] };
        let (seq, t, k, len) = VideoPacket::parse_header(&p.header()).unwrap();
        assert_eq!((seq, t, k, len), (7, 123_456_789, true, 3));
    }

    #[test]
    fn rejects_huge_frames() {
        let mut h = [0u8; VIDEO_HEADER_LEN];
        h[13..17].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(VideoPacket::parse_header(&h).is_err());
    }

    #[test]
    fn input_json_roundtrip() {
        let m = ViewerMsg::Input(InputEvent::Key { scancode: 0xE048, down: true });
        let s = serde_json::to_string(&m).unwrap();
        let back: ViewerMsg = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ViewerMsg::Input(InputEvent::Key { scancode: 0xE048, down: true })));
    }
}
