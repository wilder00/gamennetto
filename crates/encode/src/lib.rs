//! Turns captured screen frames into a compressed H.264 stream.
//!
//! Pipeline on Windows, all on the GPU:
//!   BGRA texture --(convert: D3D11 video processor)--> NV12 texture
//!               --(H264Encoder: Media Foundation hardware MFT)--> H.264 bytes

#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod mf;

#[cfg(windows)]
pub use convert::Converter;
#[cfg(windows)]
pub use mf::{EncodedFrame, EncoderConfig, H264Encoder};

/// Rounds down to an even number. NV12 and H.264 need even sizes.
pub fn even(v: u32) -> u32 {
    v & !1
}

#[cfg(test)]
mod tests {
    #[test]
    fn even_rounds_down() {
        assert_eq!(super::even(1081), 1080);
        assert_eq!(super::even(1920), 1920);
    }
}
