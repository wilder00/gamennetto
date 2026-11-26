//! Viewer side: decode H.264 on the GPU and show it in a window.

#[cfg(windows)]
mod decoder;
#[cfg(windows)]
mod present;

#[cfg(windows)]
pub use decoder::{create_device, H264Decoder};
#[cfg(windows)]
pub use present::Presenter;

/// How the picture fills the window. None of them distorts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleMode {
    /// As big as fits; black bars only where the shapes differ.
    #[default]
    Fit,
    /// Covers the whole window, trimming the edges that don't fit.
    Fill,
    /// One picture pixel per screen pixel (made smaller only if it doesn't fit).
    Actual,
}

impl ScaleMode {
    pub fn next(self) -> Self {
        match self {
            Self::Fit => Self::Fill,
            Self::Fill => Self::Actual,
            Self::Actual => Self::Fit,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Fit => "fit: as big as possible, whole picture",
            Self::Fill => "fill: whole window, edges trimmed",
            Self::Actual => "actual size: pixel for pixel",
        }
    }
}

/// Where a `src_w`x`src_h` picture goes in a `dst_w`x`dst_h` window.
/// Returns (part of the picture shown, where it goes in the window), both
/// as (left, top, width, height) in pixels.
pub fn place(mode: ScaleMode, src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (Rect, Rect) {
    let whole_src = (0, 0, src_w, src_h);
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (whole_src, (0, 0, dst_w, dst_h));
    }
    let fit = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
    match mode {
        ScaleMode::Fit | ScaleMode::Actual => {
            let scale = if mode == ScaleMode::Actual { fit.min(1.0) } else { fit };
            let w = ((src_w as f64 * scale).round() as u32).min(dst_w);
            let h = ((src_h as f64 * scale).round() as u32).min(dst_h);
            (whole_src, ((dst_w - w) / 2, (dst_h - h) / 2, w, h))
        }
        ScaleMode::Fill => {
            let scale = (dst_w as f64 / src_w as f64).max(dst_h as f64 / src_h as f64);
            // The part of the picture that covers the window, centred.
            let sw = ((dst_w as f64 / scale).round() as u32).clamp(1, src_w);
            let sh = ((dst_h as f64 / scale).round() as u32).clamp(1, src_h);
            (((src_w - sw) / 2, (src_h - sh) / 2, sw, sh), (0, 0, dst_w, dst_h))
        }
    }
}

pub type Rect = (u32, u32, u32, u32);

/// Window pixel -> position on the shared screen (0.0..=1.0 each way), or
/// None outside the picture.
pub fn window_to_screen(x: f64, y: f64, src: Rect, dst: Rect, src_w: u32, src_h: u32) -> Option<(f32, f32)> {
    let (sl, st, sw, sh) = src;
    let (dl, dt, dw, dh) = dst;
    if dw == 0 || dh == 0 || src_w == 0 || src_h == 0 {
        return None;
    }
    let fx = (x - dl as f64) / dw as f64;
    let fy = (y - dt as f64) / dh as f64;
    if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
        return None;
    }
    let px = (sl as f64 + fx * sw as f64) / src_w as f64;
    let py = (st as f64 + fy * sh as f64) / src_h as f64;
    Some((px.clamp(0.0, 1.0) as f32, py.clamp(0.0, 1.0) as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_grows() {
        // 16:10 host on a 16:9 screen: as tall as the screen, bars at the sides.
        assert_eq!(place(ScaleMode::Fit, 1728, 1080, 2560, 1440).1, (128, 0, 2304, 1440));
        // A small picture grows to fill.
        assert_eq!(place(ScaleMode::Fit, 1280, 720, 2560, 1440).1, (0, 0, 2560, 1440));
        assert_eq!(place(ScaleMode::Fit, 1920, 1080, 1024, 768).1, (0, 96, 1024, 576));
    }

    #[test]
    fn fill_covers_the_window_by_trimming() {
        let (src, dst) = place(ScaleMode::Fill, 1728, 1080, 2560, 1440);
        assert_eq!(dst, (0, 0, 2560, 1440));
        // 16:10 -> 16:9: a little is cut from the top and bottom.
        assert_eq!(src, (0, 54, 1728, 972));
    }

    #[test]
    fn actual_size_is_pixel_for_pixel_unless_too_big() {
        assert_eq!(place(ScaleMode::Actual, 1280, 720, 2560, 1440).1, (640, 360, 1280, 720));
        assert_eq!(place(ScaleMode::Actual, 3840, 2160, 2560, 1440).1, (0, 0, 2560, 1440));
    }

    #[test]
    fn mouse_maps_through_the_trimmed_part() {
        let (src, dst) = place(ScaleMode::Fill, 1728, 1080, 2560, 1440);
        // Window centre is the picture centre.
        let (x, y) = window_to_screen(1280.0, 720.0, src, dst, 1728, 1080).unwrap();
        assert!((x - 0.5).abs() < 0.01 && (y - 0.5).abs() < 0.01);
        // The window's top edge is a bit below the picture's top (trimmed).
        let (_, y) = window_to_screen(1280.0, 0.0, src, dst, 1728, 1080).unwrap();
        assert!((y - 54.0 / 1080.0).abs() < 0.001, "{y}");
        // Outside the picture (in a fit bar): nothing.
        let (src, dst) = place(ScaleMode::Fit, 1728, 1080, 2560, 1440);
        assert!(window_to_screen(10.0, 10.0, src, dst, 1728, 1080).is_none());
    }
}
