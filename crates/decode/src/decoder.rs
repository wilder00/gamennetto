use anyhow::{bail, Context, Result};
use std::mem::ManuallyDrop;
use std::time::Duration;

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VARIANT;

/// A D3D11 device on the main GPU, able to decode video.
pub fn create_device() -> Result<ID3D11Device> {
    unsafe {
        let mut device = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .context("creating Direct3D 11 device")?;
        let device = device.context("no D3D11 device")?;
        let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
        Ok(device)
    }
}

/// Hardware H.264 decoder (Windows' built-in decoder using the GPU).
/// Each decoded picture is copied into `frame()`, an NV12 texture we own,
/// so the decoder can reuse its buffers immediately.
pub struct H264Decoder {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    transform: IMFTransform,
    _manager: IMFDXGIDeviceManager,
    frame: Option<ID3D11Texture2D>,
    /// Visible picture size (the texture may be a bit larger, e.g. 1088 rows).
    pub width: u32,
    pub height: u32,
}

impl H264Decoder {
    pub fn new(device: &ID3D11Device, width: u32, height: u32, fps: u32) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let transform = find_decoder()?;

            let attrs = transform.GetAttributes()?;
            if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) == 0 {
                bail!("the H.264 decoder can't use the GPU on this PC");
            }
            // Output each picture as soon as it is decoded, no reordering delay.
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            if let Ok(api) = transform.cast::<ICodecAPI>() {
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
            }

            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.context("no DXGI device manager")?;
            manager.ResetDevice(device, token)?;
            transform
                .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
                .context("decoder refused the GPU (hardware decoding unavailable)")?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, ((fps.max(1) as u64) << 32) | 1)?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &in_type, 0)?;
            set_nv12_output(&transform)?;

            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            Ok(Self {
                device: device.clone(),
                context: device.GetImmediateContext()?,
                transform,
                _manager: manager,
                frame: None,
                width,
                height,
            })
        }
    }

    /// The latest decoded picture (NV12), if any.
    pub fn frame(&self) -> Option<&ID3D11Texture2D> {
        self.frame.as_ref()
    }

    /// Decodes one H.264 access unit. Returns true if a new picture is ready.
    pub fn decode(&mut self, data: &[u8], pts: Duration) -> Result<bool> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime((pts.as_nanos() / 100) as i64)?;

            let mut got = false;
            if let Err(e) = self.transform.ProcessInput(0, &sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(e.into());
                }
                // Decoder is full: collect its output, then retry.
                got |= self.drain()?;
                self.transform.ProcessInput(0, &sample, 0)?;
            }
            got |= self.drain()?;
            Ok(got)
        }
    }

    /// Takes every ready picture; keeps the newest.
    unsafe fn drain(&mut self) -> Result<bool> {
        let mut got = false;
        loop {
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER::default()];
            let mut status = 0u32;
            let result = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let sample = ManuallyDrop::take(&mut buffers[0].pSample);
            let _events = ManuallyDrop::take(&mut buffers[0].pEvents);
            match result {
                Ok(()) => {
                    if let Some(sample) = sample {
                        self.keep(&sample)?;
                        got = true;
                    }
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(got),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // First keyframe tells the decoder the real format.
                    set_nv12_output(&self.transform)?;
                    if let Ok(t) = self.transform.GetOutputCurrentType(0) {
                        if let Ok(size) = t.GetUINT64(&MF_MT_FRAME_SIZE) {
                            self.width = (size >> 32) as u32;
                            self.height = size as u32;
                        }
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    unsafe fn keep(&mut self, sample: &IMFSample) -> Result<()> {
        let buffer = sample.GetBufferByIndex(0)?;
        let dxgi: IMFDXGIBuffer = buffer.cast().context("decoder output is not on the GPU")?;
        let mut raw = std::ptr::null_mut();
        dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
        let src = ID3D11Texture2D::from_raw(raw);
        let index = dxgi.GetSubresourceIndex()?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        src.GetDesc(&mut desc);
        let needs_new = match &self.frame {
            Some(t) => {
                let mut d = D3D11_TEXTURE2D_DESC::default();
                t.GetDesc(&mut d);
                d.Width != desc.Width || d.Height != desc.Height
            }
            None => true,
        };
        if needs_new {
            let own = D3D11_TEXTURE2D_DESC {
                Width: desc.Width,
                Height: desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: 0,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            // The video processor reads NV12 from textures made for the
            // decoder or as render targets; drivers differ, so try in order.
            let mut tex = None;
            for flags in [D3D11_BIND_DECODER.0, D3D11_BIND_RENDER_TARGET.0, D3D11_BIND_SHADER_RESOURCE.0, 0] {
                let d = D3D11_TEXTURE2D_DESC { BindFlags: flags as u32, ..own };
                if self.device.CreateTexture2D(&d, None, Some(&mut tex)).is_ok() {
                    break;
                }
            }
            self.frame = Some(tex.context("could not create a texture for decoded pictures")?);
        }
        let dst = self.frame.as_ref().context("no frame texture")?;
        self.context.CopySubresourceRegion(dst, 0, 0, 0, 0, &src, index, None);
        Ok(())
    }
}

impl Drop for H264Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

unsafe fn set_nv12_output(t: &IMFTransform) -> Result<()> {
    let mut i = 0;
    while let Ok(mt) = t.GetOutputAvailableType(0, i) {
        i += 1;
        if mt.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
            t.SetOutputType(0, &mt, 0)?;
            return Ok(());
        }
    }
    bail!("decoder can't output NV12")
}

unsafe fn find_decoder() -> Result<IMFTransform> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_DECODER,
        MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
        Some(&input),
        None,
        &mut activates,
        &mut count,
    )?;
    let mut found = None;
    for i in 0..count as usize {
        let Some(a) = std::ptr::read(activates.add(i)) else { continue };
        if found.is_none() {
            found = a.ActivateObject::<IMFTransform>().ok();
        }
    }
    if !activates.is_null() {
        CoTaskMemFree(Some(activates as _));
    }
    found.context("no H.264 decoder found (is the Media Feature Pack installed?)")
}
