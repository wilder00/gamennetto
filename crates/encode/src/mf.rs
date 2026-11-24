use anyhow::{bail, Context, Result};
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VARIANT;

#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Target bits per second.
    pub bitrate: u32,
    /// Frames between forced keyframes. Keyframes let a viewer (re)join and
    /// recover from lost packets; more of them cost bandwidth.
    pub keyframe_interval: u32,
}

pub struct EncodedFrame {
    /// H.264 Annex B byte stream (start codes, SPS/PPS before keyframes).
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub pts: Duration,
}

/// A hardware H.264 encoder driven through Media Foundation, configured for
/// low latency: no B-frames, constant bitrate, one frame out per frame in.
pub struct H264Encoder {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    codec_api: Option<ICodecAPI>,
    _manager: IMFDXGIDeviceManager,
    name: String,
    /// Input requests received from the encoder but not used yet.
    need_input: u32,
    frame_duration: i64,
}

const NO_WAIT: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS = MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(1);

impl H264Encoder {
    /// Opens a hardware H.264 encoder on the same GPU as `device`, so frames
    /// never leave that GPU.
    pub fn new(device: &ID3D11Device, cfg: &EncoderConfig) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;

            let vendor = device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?.VendorId;

            let (transform, name) = open_hardware_h264(vendor)?;

            let attrs = transform.GetAttributes()?;
            // Hardware encoders are asynchronous and must be unlocked first.
            attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);

            // Give the encoder our D3D device so it can read GPU textures.
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.context("no DXGI device manager")?;
            manager.ResetDevice(device, token)?;
            transform
                .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
                .context("encoder refused the D3D11 device")?;

            // Rate control and latency settings go in before the media types.
            let codec_api = transform.cast::<ICodecAPI>().ok();
            if let Some(api) = &codec_api {
                let set = |key: &GUID, v: VARIANT| {
                    let _ = api.SetValue(key, &v);
                };
                set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
                set(&CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32));
                set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(cfg.bitrate));
                set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
                set(&CODECAPI_AVEncMPVGOPSize, VARIANT::from(cfg.keyframe_interval));
            }

            let size = ((cfg.width as u64) << 32) | cfg.height as u64;
            let rate = ((cfg.fps as u64) << 32) | 1;

            let out_type = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate)?;
            out_type.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            out_type.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            // Main profile: decodes in hardware on practically every phone and PC.
            out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
            set_bt709_limited(&out_type);
            transform
                .SetOutputType(0, &out_type, 0)
                .context("encoder rejected the output format (resolution too big?)")?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            set_bt709_limited(&in_type);
            transform.SetInputType(0, &in_type, 0).context("encoder rejected NV12 input")?;

            transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let events: IMFMediaEventGenerator = transform.cast()?;
            Ok(Self {
                transform,
                events,
                codec_api,
                _manager: manager,
                name,
                need_input: 0,
                frame_duration: 10_000_000 / cfg.fps.max(1) as i64,
            })
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Makes the next frame a keyframe (e.g. a viewer joined or lost data).
    pub fn request_keyframe(&self) {
        if let Some(api) = &self.codec_api {
            let _ = unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32)) };
        }
    }

    /// Encodes one NV12 texture and waits (up to `timeout`) for its output.
    pub fn encode(&mut self, nv12: &ID3D11Texture2D, pts: Duration, timeout: Duration) -> Result<Vec<EncodedFrame>> {
        let mut out = Vec::new();
        let deadline = Instant::now() + timeout;

        // 1. Wait until the encoder asks for input.
        while self.need_input == 0 {
            if !self.pump_event(&mut out)? {
                if Instant::now() >= deadline {
                    bail!("encoder did not ask for input in time");
                }
                std::thread::sleep(Duration::from_micros(250));
            }
        }

        // 2. Hand over the GPU texture without copying it.
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, nv12, 0, false)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime((pts.as_nanos() / 100) as i64)?;
            sample.SetSampleDuration(self.frame_duration)?;
            self.transform.ProcessInput(0, &sample, 0)?;
        }
        self.need_input -= 1;

        // 3. Low-latency mode gives one output per input: wait for it.
        let before = out.len();
        while out.len() == before && Instant::now() < deadline {
            if !self.pump_event(&mut out)? {
                std::thread::sleep(Duration::from_micros(250));
            }
        }
        Ok(out)
    }

    /// Handles one pending encoder event. Returns false if none was waiting.
    fn pump_event(&mut self, out: &mut Vec<EncodedFrame>) -> Result<bool> {
        let event = match unsafe { self.events.GetEvent(NO_WAIT) } {
            Ok(e) => e,
            Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let kind = unsafe { event.GetType()? };
        if kind == METransformNeedInput.0 as u32 {
            self.need_input += 1;
        } else if kind == METransformHaveOutput.0 as u32 {
            if let Some(frame) = self.take_output()? {
                out.push(frame);
            }
        }
        Ok(true)
    }

    fn take_output(&mut self) -> Result<Option<EncodedFrame>> {
        unsafe {
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER::default()];
            let mut status = 0u32;
            let result = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let sample = ManuallyDrop::take(&mut buffers[0].pSample);
            let _events = ManuallyDrop::take(&mut buffers[0].pEvents);
            if let Err(e) = result {
                if e.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                    // The encoder changed its output format; accept the new one.
                    let t = self.transform.GetOutputAvailableType(0, 0)?;
                    self.transform.SetOutputType(0, &t, 0)?;
                    return Ok(None);
                }
                if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                    return Ok(None);
                }
                return Err(e.into());
            }
            let Some(sample) = sample else {
                return Ok(None);
            };

            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut ptr, None, Some(&mut len))?;
            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            buffer.Unlock()?;

            let keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let pts = sample.GetSampleTime().unwrap_or(0).max(0) as u64;
            Ok(Some(EncodedFrame { data, keyframe, pts: Duration::from_nanos(pts * 100) }))
        }
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

/// Finds a hardware H.264 encoder belonging to the GPU with PCI `vendor` id,
/// and returns the first one that actually starts.
unsafe fn open_hardware_h264(vendor: u32) -> Result<(IMFTransform, String)> {
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        None,
        Some(&output),
        &mut activates,
        &mut count,
    )?;

    let wanted = format!("VEN_{vendor:04X}");
    let mut found = None;
    let mut errors = Vec::new();
    for i in 0..count as usize {
        let Some(activate) = std::ptr::read(activates.add(i)) else {
            continue;
        };
        if found.is_some() {
            continue; // still read the rest so they are released
        }
        let name = get_string(&activate, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default();
        let ven = get_string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).unwrap_or_default();
        if !ven.to_ascii_uppercase().contains(&wanted) {
            continue;
        }
        match activate.ActivateObject::<IMFTransform>() {
            Ok(t) => found = Some((t, name)),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    if !activates.is_null() {
        CoTaskMemFree(Some(activates as _));
    }
    match found {
        Some(f) => Ok(f),
        None if errors.is_empty() => {
            bail!("no hardware H.264 encoder on this GPU (vendor {wanted}); software encoding is not implemented yet")
        }
        None => bail!("hardware H.264 encoders failed to start: {}", errors.join("; ")),
    }
}

/// Marks a media type as BT.709 limited range, matching what the Converter
/// produces, so decoders don't have to guess the colours.
unsafe fn set_bt709_limited(t: &IMFMediaType) {
    let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
    let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
}

unsafe fn get_string(activate: &IMFActivate, key: &GUID) -> Option<String> {
    let mut s = windows::core::PWSTR::null();
    let mut len = 0u32;
    activate.GetAllocatedString(key, &mut s, &mut len).ok()?;
    let out = s.to_string().ok();
    CoTaskMemFree(Some(s.0 as _));
    out
}
