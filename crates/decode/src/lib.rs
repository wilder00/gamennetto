//! Viewer side: decode H.264 on the GPU and show it in a window.

#[cfg(windows)]
mod decoder;
#[cfg(windows)]
mod present;

#[cfg(windows)]
pub use decoder::{create_device, H264Decoder};
#[cfg(windows)]
pub use present::Presenter;

/// Where a `src_w`x`src_h` picture goes inside a `dst_w`x`dst_h` window,
/// keeping its shape (black bars on the sides or top/bottom).
/// Returns (left, top, width, height).
pub fn letterbox(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (u32, u32, u32, u32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w, dst_h);
    }
    let scale = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
    let w = ((src_w as f64 * scale).round() as u32).min(dst_w);
    let h = ((src_h as f64 * scale).round() as u32).min(dst_h);
    ((dst_w - w) / 2, (dst_h - h) / 2, w, h)
}

#[cfg(test)]
mod tests {
    use super::letterbox;

    #[test]
    fn letterbox_keeps_aspect() {
        // 16:9 video in a 4:3 window: bars on top and bottom.
        assert_eq!(letterbox(1920, 1080, 1024, 768), (0, 96, 1024, 576));
        // 16:9 video in a 21:9 window: bars on the sides.
        assert_eq!(letterbox(1920, 1080, 2560, 1080), (320, 0, 1920, 1080));
        assert_eq!(letterbox(1920, 1080, 1920, 1080), (0, 0, 1920, 1080));
    }
}
