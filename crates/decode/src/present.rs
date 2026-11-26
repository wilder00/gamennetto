use anyhow::{Context, Result};
use std::mem::ManuallyDrop;

use windows::core::Interface;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::{place, Rect, ScaleMode};

/// A video processor and the (src_w, src_h, dst_w, dst_h) it was made for.
type CachedProcessor = (ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor, (u32, u32, u32, u32));

/// Draws NV12 pictures into a window, scaled by `mode` without distortion.
/// Colour conversion and scaling run on the GPU's video hardware.
pub struct Presenter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    swapchain: IDXGISwapChain1,
    processor: Option<CachedProcessor>,
    pub mode: ScaleMode,
    /// Last drawn: the part of the picture shown, and where in the window.
    pub shown: (Rect, Rect),
    /// Size of the last picture drawn.
    pub picture_size: (u32, u32),
}

impl Presenter {
    pub fn new(device: &ID3D11Device, hwnd: HWND) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory2 = device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 0, // take the window size
                Height: 0,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: 0,
            };
            let swapchain = factory
                .CreateSwapChainForHwnd(device, hwnd, &desc, None, None)
                .context("creating the window's swap chain")?;
            let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            Ok(Self {
                device: device.clone(),
                context: device.GetImmediateContext()?,
                video_device: device.cast()?,
                video_context: device.GetImmediateContext()?.cast()?,
                swapchain,
                processor: None,
                mode: ScaleMode::default(),
                shown: ((0, 0, 0, 0), (0, 0, 0, 0)),
                picture_size: (0, 0),
            })
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(()); // minimised
        }
        unsafe {
            self.swapchain.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        }
        Ok(())
    }

    /// Shows `frame` (NV12, of which `w`x`h` is visible), or black if None.
    pub fn present(&mut self, frame: Option<(&ID3D11Texture2D, u32, u32)>) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
            let mut bd = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut bd);

            match frame {
                None => {
                    let mut rtv = None;
                    self.device.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
                    if let Some(rtv) = rtv {
                        self.context.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);
                    }
                }
                Some((tex, w, h)) => self.blit(tex, w, h, &back, bd.Width, bd.Height)?,
            }
            // Sync interval 0: show immediately, don't wait for the screen refresh.
            self.swapchain.Present(0, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    unsafe fn blit(
        &mut self,
        tex: &ID3D11Texture2D,
        w: u32,
        h: u32,
        back: &ID3D11Texture2D,
        bw: u32,
        bh: u32,
    ) -> Result<()> {
        let key = (w, h, bw, bh);
        if self.processor.as_ref().map(|p| p.2) != Some(key) {
            let rate = DXGI_RATIONAL { Numerator: 60, Denominator: 1 };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: w,
                InputHeight: h,
                OutputFrameRate: rate,
                OutputWidth: bw,
                OutputHeight: bh,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let en =
                self.video_device.CreateVideoProcessorEnumerator(&content).context("video processor enumerator")?;
            let vp = self.video_device.CreateVideoProcessor(&en, 0).context("video processor")?;
            let vc = &self.video_context;
            vc.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            vc.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
            vc.VideoProcessorSetOutputBackgroundColor(&vp, false, &D3D11_VIDEO_COLOR::default());
            if let Ok(vc1) = vc.cast::<ID3D11VideoContext1>() {
                vc1.VideoProcessorSetStreamColorSpace1(&vp, 0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
                vc1.VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
            }
            self.processor = Some((en, vp, key));
        }
        let (en, vp, _) = self.processor.as_ref().unwrap();

        let ((sl, st, sw, sh), (l, t, pw, ph)) = place(self.mode, w, h, bw, bh);
        self.shown = ((sl, st, sw, sh), (l, t, pw, ph));
        self.picture_size = (w, h);
        let vc = &self.video_context;
        let src = RECT { left: sl as i32, top: st as i32, right: (sl + sw) as i32, bottom: (st + sh) as i32 };
        let dst = RECT { left: l as i32, top: t as i32, right: (l + pw) as i32, bottom: (t + ph) as i32 };
        let full = RECT { left: 0, top: 0, right: bw as i32, bottom: bh as i32 };
        vc.VideoProcessorSetStreamSourceRect(vp, 0, true, Some(&src));
        vc.VideoProcessorSetStreamDestRect(vp, 0, true, Some(&dst));
        vc.VideoProcessorSetOutputTargetRect(vp, true, Some(&full));

        let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
            },
        };
        let mut in_view = None;
        self.video_device
            .CreateVideoProcessorInputView(tex, en, &in_desc, Some(&mut in_view))
            .context("input view on decoded picture")?;
        let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
        };
        let mut out_view = None;
        self.video_device
            .CreateVideoProcessorOutputView(back, en, &out_desc, Some(&mut out_view))
            .context("output view on window")?;
        let out_view = out_view.context("no output view")?;

        let mut streams = [D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(in_view),
            ..Default::default()
        }];
        let r = vc.VideoProcessorBlt(vp, &out_view, 0, &streams);
        ManuallyDrop::drop(&mut streams[0].pInputSurface);
        r.context("drawing the picture failed")?;
        Ok(())
    }
}
