//! Hardware / environment probing.
//!
//! Everything here is detected at runtime, so the same binary adapts to
//! whatever machine it runs on. Nothing in this crate assumes a specific GPU.

mod android;
mod encoders;
mod gpu;
mod network;

use serde::Serialize;

pub use android::AndroidTools;
pub use encoders::{Codec, Encoder};
pub use gpu::{Gpu, Vendor};
pub use network::Iface;

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub os: String,
    pub gpus: Vec<Gpu>,
    pub encoders: Vec<Encoder>,
    pub interfaces: Vec<Iface>,
    pub android: AndroidTools,
    pub recommendation: Recommendation,
}

/// What the app would pick on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    /// Chosen H.264 encoder, or None if no encoder could be created at all.
    pub h264_encoder: Option<String>,
    pub hardware_encoding: bool,
    /// The address other devices should connect to.
    pub lan_address: Option<String>,
    pub notes: Vec<String>,
}

pub fn probe() -> Report {
    let gpus = gpu::detect();
    let encoders = encoders::detect();
    let interfaces = network::detect();
    let android = android::detect();
    let recommendation = recommend(&gpus, &encoders, &interfaces);
    Report { os: std::env::consts::OS.to_string(), gpus, encoders, interfaces, android, recommendation }
}

/// Fallback order for H.264:
/// 1. a working hardware encoder on the GPU that draws the screen
///    (no copying frames between GPUs, lowest latency),
/// 2. a working hardware encoder on another GPU,
/// 3. a working software encoder.
///
/// This is a first guess from what is installed; the streaming code will
/// confirm it by measuring real encode times.
fn recommend(gpus: &[Gpu], encoders: &[Encoder], ifaces: &[Iface]) -> Recommendation {
    let mut notes = Vec::new();

    let display_vendors: Vec<Vendor> =
        gpus.iter().filter(|g| g.drives_display && !g.is_software).map(|g| g.vendor).collect();
    let h264 =
        |hardware: bool| encoders.iter().filter(move |e| e.codec == Codec::H264 && e.hardware == hardware && e.works);

    for e in encoders.iter().filter(|e| e.hardware && !e.works) {
        notes.push(format!(
            "{} is installed but failed to start ({}).",
            e.name,
            e.error.as_deref().unwrap_or("unknown error")
        ));
    }

    let same_gpu = h264(true).find(|e| e.vendor.is_some_and(|v| display_vendors.contains(&v)));
    let chosen = if let Some(e) = same_gpu {
        Some((e, true))
    } else if let Some(e) = h264(true).next() {
        notes.push(format!(
            "{} is on a different GPU than the one drawing the screen. \
             Frames must be copied between GPUs, which adds a little latency.",
            e.name
        ));
        Some((e, true))
    } else if let Some(e) = h264(false).next() {
        notes.push(
            "No working hardware H.264 encoder: using software encoding. \
             Expect higher CPU use and lower resolution/fps."
                .into(),
        );
        Some((e, false))
    } else {
        notes.push("No working H.264 encoder found. Streaming from this PC is not possible.".into());
        None
    };

    if gpus.is_empty() {
        notes.push("No GPU was detected.".into());
    }

    let lan_address = ifaces.iter().find(|i| i.is_private_lan && !i.is_virtual).map(|i| i.addr.clone());
    if lan_address.is_none() {
        notes.push(
            "No real Wi-Fi/Ethernet LAN connection found. Connect to your network, \
             or use USB tethering from the phone."
                .into(),
        );
    }

    Recommendation {
        h264_encoder: chosen.map(|(e, _)| e.name.clone()),
        hardware_encoding: chosen.is_some_and(|(_, hw)| hw),
        lan_address,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(vendor: Vendor, drives_display: bool) -> Gpu {
        Gpu { name: format!("{vendor:?}"), vendor, vram_mb: 1024, is_software: false, drives_display }
    }

    fn enc(name: &str, vendor: Option<Vendor>, hardware: bool, works: bool) -> Encoder {
        Encoder { codec: Codec::H264, name: name.into(), hardware, vendor, works, error: None }
    }

    fn iface(name: &str, addr: &str, is_virtual: bool) -> Iface {
        Iface { name: name.into(), addr: addr.into(), is_private_lan: true, is_virtual }
    }

    #[test]
    fn prefers_encoder_on_display_gpu() {
        // Typical gaming laptop: Intel draws the screen, NVIDIA is listed first.
        let gpus = [gpu(Vendor::Intel, true), gpu(Vendor::Nvidia, false)];
        let encs = [enc("NV", Some(Vendor::Nvidia), true, true), enc("QSV", Some(Vendor::Intel), true, true)];
        let r = recommend(&gpus, &encs, &[]);
        assert_eq!(r.h264_encoder.as_deref(), Some("QSV"));
        assert!(r.hardware_encoding);
    }

    #[test]
    fn uses_other_gpu_with_note() {
        let gpus = [gpu(Vendor::Intel, true)];
        let encs = [enc("NV", Some(Vendor::Nvidia), true, true)];
        let r = recommend(&gpus, &encs, &[]);
        assert_eq!(r.h264_encoder.as_deref(), Some("NV"));
        assert!(r.notes.iter().any(|n| n.contains("different GPU")));
    }

    #[test]
    fn skips_encoders_that_fail_to_start() {
        let gpus = [gpu(Vendor::Nvidia, true)];
        let encs = [enc("NV", Some(Vendor::Nvidia), true, false), enc("SW", None, false, true)];
        let r = recommend(&gpus, &encs, &[]);
        assert_eq!(r.h264_encoder.as_deref(), Some("SW"));
        assert!(!r.hardware_encoding);
        assert!(r.notes.iter().any(|n| n.contains("failed to start")));
    }

    #[test]
    fn no_encoder_at_all() {
        let r = recommend(&[], &[], &[]);
        assert_eq!(r.h264_encoder, None);
        assert!(!r.hardware_encoding);
    }

    #[test]
    fn ignores_virtual_adapters_for_lan_address() {
        let ifaces =
            [iface("VMware Network Adapter VMnet1", "192.168.174.1", true), iface("Wi-Fi", "192.168.100.7", false)];
        let r = recommend(&[], &[], &ifaces);
        assert_eq!(r.lan_address.as_deref(), Some("192.168.100.7"));
    }

    #[test]
    fn vendor_from_pci_id() {
        assert_eq!(Vendor::from_pci_id(0x10DE), Vendor::Nvidia);
        assert_eq!(Vendor::from_pci_id(0x8086), Vendor::Intel);
        assert_eq!(Vendor::from_pci_id(0x1002), Vendor::Amd);
    }
}
