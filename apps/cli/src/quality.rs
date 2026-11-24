//! Quality presets, each explained in plain words for the user.

use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Quality {
    Fastest,
    Balanced,
    Sharpest,
}

pub struct Settings {
    /// Scale the picture down to at most this height (None = full resolution).
    pub max_height: Option<u32>,
    pub fps: u32,
    pub mbps: f64,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Fastest, Quality::Balanced, Quality::Sharpest];

    pub fn settings(self) -> Settings {
        match self {
            Quality::Fastest => Settings { max_height: Some(720), fps: 60, mbps: 8.0 },
            Quality::Balanced => Settings { max_height: Some(1080), fps: 60, mbps: 15.0 },
            Quality::Sharpest => Settings { max_height: None, fps: 60, mbps: 30.0 },
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Quality::Fastest => "Fastest   - lowest delay, softer image",
            Quality::Balanced => "Balanced  - good for most things (recommended)",
            Quality::Sharpest => "Sharpest  - crisp text, needs a strong connection",
        }
    }

    pub fn explain(self) -> &'static str {
        match self {
            Quality::Fastest => {
                "Sends a smaller picture (up to 720p) so each frame is quick to compress, \
                 send and show. Best for mouse, touch, drawing and games. Small text looks soft."
            }
            Quality::Balanced => {
                "Up to 1080p at 60 frames per second. Text is readable and the delay stays low \
                 on normal 5 GHz Wi-Fi."
            }
            Quality::Sharpest => {
                "Full resolution with lots of data, so text is as sharp as on the real screen. \
                 Each frame is bigger, so the delay grows a little, and weak or 2.4 GHz Wi-Fi \
                 may stutter. Best for reading documents, code and spreadsheets."
            }
        }
    }
}

/// Output size for a `w`x`h` screen with a height limit, keeping the shape.
pub fn output_size(w: u32, h: u32, max_height: Option<u32>) -> (u32, u32) {
    let (w, h) = match max_height {
        Some(m) if h > m => (w * m / h, m),
        _ => (w, h),
    };
    (gamennetto_encode::even(w), gamennetto_encode::even(h))
}

#[cfg(test)]
mod tests {
    use super::output_size;

    #[test]
    fn scales_down_keeping_shape() {
        assert_eq!(output_size(2560, 1440, Some(1080)), (1920, 1080));
        assert_eq!(output_size(2560, 1440, Some(720)), (1280, 720));
        assert_eq!(output_size(1366, 768, Some(1080)), (1366, 768)); // already small
        assert_eq!(output_size(2560, 1440, None), (2560, 1440));
    }
}
