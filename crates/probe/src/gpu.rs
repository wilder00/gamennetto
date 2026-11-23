use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Microsoft,
    Other,
}

impl Vendor {
    pub fn from_pci_id(id: u32) -> Self {
        match id {
            0x10DE => Vendor::Nvidia,
            0x1002 | 0x1022 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            0x1414 => Vendor::Microsoft,
            _ => Vendor::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Gpu {
    pub name: String,
    pub vendor: Vendor,
    pub vram_mb: u64,
    /// True for software rasterizers such as "Microsoft Basic Render Driver".
    pub is_software: bool,
    /// True if a monitor is attached to this GPU, i.e. it draws the desktop.
    /// On laptops this is often the integrated GPU, not the fast one.
    pub drives_display: bool,
}

#[cfg(windows)]
pub fn detect() -> Vec<Gpu> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

    let mut out = Vec::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return out;
    };
    let mut i = 0;
    // EnumAdapters1 returns an error (DXGI_ERROR_NOT_FOUND) after the last adapter.
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
        i += 1;
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        let len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        out.push(Gpu {
            name: String::from_utf16_lossy(&desc.Description[..len]),
            vendor: Vendor::from_pci_id(desc.VendorId),
            vram_mb: (desc.DedicatedVideoMemory / (1024 * 1024)) as u64,
            is_software: desc.Flags & 2 != 0, // DXGI_ADAPTER_FLAG_SOFTWARE
            drives_display: unsafe { adapter.EnumOutputs(0) }.is_ok(),
        });
    }
    out
}

#[cfg(not(windows))]
pub fn detect() -> Vec<Gpu> {
    // TODO(linux): enumerate via DRM / Vulkan.
    Vec::new()
}
