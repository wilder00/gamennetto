use crate::gpu::Vendor;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Codec {
    H264,
    Hevc,
}

#[derive(Debug, Clone, Serialize)]
pub struct Encoder {
    pub codec: Codec,
    pub name: String,
    pub hardware: bool,
    /// GPU vendor a hardware encoder belongs to, when Windows reports it.
    pub vendor: Option<Vendor>,
    /// True if the encoder could actually be created, not just listed.
    pub works: bool,
    /// Windows error when `works` is false.
    pub error: Option<String>,
}

/// Enumerates the video encoders Windows Media Foundation knows about.
/// Hardware encoders (NVENC, AMF, QuickSync) register themselves here when
/// the GPU driver supports them, so this adapts to any machine.
#[cfg(windows)]
pub fn detect() -> Vec<Encoder> {
    use windows::core::GUID;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

    let mut out = Vec::new();

    unsafe {
        // Ignore "already initialised" results; we only need COM to be usable.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, MFSTARTUP_FULL).is_err() {
            return out;
        }

        let hw = MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER;
        let sw = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER;
        let queries: [(Codec, GUID, bool, MFT_ENUM_FLAG); 4] = [
            (Codec::H264, MFVideoFormat_H264, true, hw),
            (Codec::Hevc, MFVideoFormat_HEVC, true, hw),
            (Codec::H264, MFVideoFormat_H264, false, sw),
            (Codec::Hevc, MFVideoFormat_HEVC, false, sw),
        ];

        for (codec, subtype, hardware, flags) in queries {
            let output = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: subtype,
            };
            let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            if MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                flags,
                None,
                Some(&output),
                &mut activates,
                &mut count,
            )
            .is_err()
                || activates.is_null()
            {
                continue;
            }

            for i in 0..count as usize {
                // Take ownership so the COM reference is released on drop.
                let Some(activate) = std::ptr::read(activates.add(i)) else {
                    continue;
                };
                let name = get_string(&activate, &MFT_FRIENDLY_NAME_Attribute)
                    .unwrap_or_else(|| "unnamed encoder".into());
                // The same encoder can match more than one query; keep it once.
                if out.iter().any(|e: &Encoder| e.codec == codec && e.name == name) {
                    continue;
                }
                // Hardware MFTs report e.g. "VEN_10DE" for NVIDIA.
                let vendor = get_string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
                    .and_then(|s| parse_vendor_id(&s))
                    .map(Vendor::from_pci_id);
                // Listing is not enough: drivers can register an encoder that
                // then fails to start. Try to create it.
                let result = activate.ActivateObject::<IMFTransform>();
                let error = result
                    .as_ref()
                    .err()
                    .map(|e| format!("0x{:08X} {}", e.code().0 as u32, e.message()));
                drop(result);
                let _ = activate.ShutdownObject();

                out.push(Encoder {
                    codec,
                    name,
                    hardware,
                    vendor,
                    works: error.is_none(),
                    error,
                });
            }
            CoTaskMemFree(Some(activates as _));
        }

        let _ = MFShutdown();
    }
    out
}

#[cfg(windows)]
unsafe fn get_string(
    activate: &windows::Win32::Media::MediaFoundation::IMFActivate,
    key: &windows::core::GUID,
) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Com::CoTaskMemFree;

    let mut s = PWSTR::null();
    let mut len = 0u32;
    activate.GetAllocatedString(key, &mut s, &mut len).ok()?;
    let out = s.to_string().ok();
    CoTaskMemFree(Some(s.0 as _));
    out
}

/// Parses "VEN_10DE" (any case, possibly followed by more text) into 0x10DE.
fn parse_vendor_id(s: &str) -> Option<u32> {
    let upper = s.to_ascii_uppercase();
    let hex = upper.split("VEN_").nth(1)?.get(..4)?;
    u32::from_str_radix(hex, 16).ok()
}

#[cfg(not(windows))]
pub fn detect() -> Vec<Encoder> {
    // TODO(linux): probe VAAPI / NVENC.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::parse_vendor_id;

    #[test]
    fn parses_vendor_ids() {
        assert_eq!(parse_vendor_id("VEN_10DE"), Some(0x10DE));
        assert_eq!(parse_vendor_id("ven_8086&DEV_1234"), Some(0x8086));
        assert_eq!(parse_vendor_id("nonsense"), None);
    }
}
