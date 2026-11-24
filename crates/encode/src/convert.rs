use anyhow::{Context, Result};
use std::mem::ManuallyDrop;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Converts BGRA screen textures to NV12 (the format video encoders take),
/// scaling if needed. Runs on the GPU's fixed-function video hardware.
pub struct Converter {
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    pub out_width: u32,
    pub out_height: u32,
}

impl Converter {
    pub fn new(device: &ID3D11Device, in_width: u32, in_height: u32, out_width: u32, out_height: u32) -> Result<Self> {
        unsafe {
            let video_device: ID3D11VideoDevice = device.cast().context("this GPU has no D3D11 video support")?;
            let video_context: ID3D11VideoContext = device.GetImmediateContext()?.cast()?;

            let rate = DXGI_RATIONAL { Numerator: 60, Denominator: 1 };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: in_width,
                InputHeight: in_height,
                OutputFrameRate: rate,
                OutputWidth: out_width,
                OutputHeight: out_height,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;

            // Output texture. Some drivers want the VIDEO_ENCODER bind flag on
            // encoder input, others reject it, so try with it first.
            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: out_width,
                Height: out_height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_VIDEO_ENCODER.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut output = None;
            if device.CreateTexture2D(&desc, None, Some(&mut output)).is_err() {
                desc.BindFlags = D3D11_BIND_RENDER_TARGET.0 as u32;
                device.CreateTexture2D(&desc, None, Some(&mut output)).context("creating NV12 texture")?;
            }
            let output = output.context("no NV12 texture")?;

            let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut output_view = None;
            video_device.CreateVideoProcessorOutputView(&output, &enumerator, &view_desc, Some(&mut output_view))?;
            let output_view = output_view.context("no output view")?;

            video_context.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            // No denoise/sharpen etc.: they cost time and change the image.
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            // Screen pixels are full-range RGB; video is limited-range BT.709.
            if let Ok(ctx1) = video_context.cast::<ID3D11VideoContext1>() {
                ctx1.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                ctx1.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            }

            Ok(Self { video_device, video_context, enumerator, processor, output, output_view, out_width, out_height })
        }
    }

    /// The NV12 texture holding the last converted picture.
    pub fn convert_output(&self) -> &ID3D11Texture2D {
        &self.output
    }

    /// Converts `input` (BGRA) into the internal NV12 texture and returns it.
    /// The returned texture is overwritten by the next call.
    pub fn convert(&mut self, input: &ID3D11Texture2D) -> Result<&ID3D11Texture2D> {
        unsafe {
            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut in_view = None;
            self.video_device.CreateVideoProcessorInputView(input, &self.enumerator, &in_desc, Some(&mut in_view))?;

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(in_view),
                ..Default::default()
            };
            let mut streams = [stream];
            let result = self.video_context.VideoProcessorBlt(&self.processor, &self.output_view, 0, &streams);
            // Release the input view we put in the ManuallyDrop field.
            ManuallyDrop::drop(&mut streams[0].pInputSurface);
            result.context("GPU colour conversion failed")?;
        }
        Ok(&self.output)
    }
}
