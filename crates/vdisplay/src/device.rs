//! Installing and removing the driver's device (needs administrator rights).
//! Works like Microsoft's `devcon install`: create a root device with the
//! driver's hardware ID, then install the driver for it.

use anyhow::{bail, Context, Result};
use std::path::Path;
use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::System::Registry::{RegCloseKey, RegQueryValueExW, KEY_READ};

use crate::settings::HARDWARE_ID;

const DISPLAY_CLASS: GUID = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub instance_id: String,
    /// Windows' problem code if the device isn't working (e.g. 22 = disabled).
    pub problem: Option<u32>,
    /// The installed driver package, e.g. `oem42.inf`.
    pub inf: Option<String>,
}

fn wide_str(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Runs `f` for each device of ours. Stops early if `f` returns true.
unsafe fn for_each_device(mut f: impl FnMut(HDEVINFO, &mut SP_DEVINFO_DATA) -> Result<bool>) -> Result<()> {
    let set = SetupDiGetClassDevsW(Some(&DISPLAY_CLASS), PCWSTR::null(), None, DIGCF_PRESENT)?;
    let mut i = 0;
    let result = loop {
        let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
        if SetupDiEnumDeviceInfo(set, i, &mut data).is_err() {
            break Ok(());
        }
        i += 1;
        let mut buf = [0u8; 1024];
        if SetupDiGetDeviceRegistryPropertyW(set, &data, SPDRP_HARDWAREID, None, Some(&mut buf), None).is_err() {
            continue;
        }
        let ids: Vec<u16> = buf.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let ours = ids.split(|&c| c == 0).any(|id| String::from_utf16_lossy(id).eq_ignore_ascii_case(HARDWARE_ID));
        if ours {
            match f(set, &mut data) {
                Ok(true) => break Ok(()),
                Ok(false) => {}
                Err(e) => break Err(e),
            }
        }
    };
    let _ = SetupDiDestroyDeviceInfoList(set);
    result
}

unsafe fn device_info(set: HDEVINFO, data: &SP_DEVINFO_DATA) -> DeviceInfo {
    let mut id = [0u16; 512];
    let _ = SetupDiGetDeviceInstanceIdW(set, data, Some(&mut id), None);
    let mut status = CM_DEVNODE_STATUS_FLAGS(0);
    let mut problem = CM_PROB(0);
    let problem = if CM_Get_DevNode_Status(&mut status, &mut problem, data.DevInst, 0) == CR_SUCCESS
        && (status.0 & DN_HAS_PROBLEM.0) != 0
    {
        Some(problem.0)
    } else {
        None
    };
    let mut inf = None;
    if let Ok(key) = SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL.0, 0, DIREG_DRV, KEY_READ.0) {
        let mut buf = [0u16; 260];
        let mut size = (buf.len() * 2) as u32;
        if RegQueryValueExW(
            key,
            &HSTRING::from("InfPath"),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut size),
        ) == ERROR_SUCCESS
        {
            inf = Some(wide_str(&buf));
        }
        let _ = RegCloseKey(key);
    }
    DeviceInfo { instance_id: wide_str(&id), problem, inf }
}

/// The installed virtual display device, if any.
pub fn find_device() -> Result<Option<DeviceInfo>> {
    let mut found = None;
    unsafe {
        for_each_device(|set, data| {
            found = Some(device_info(set, data));
            Ok(true)
        })?;
    }
    Ok(found)
}

/// Installs the driver from a folder containing MttVDD.inf. Needs
/// administrator rights. Windows may ask whether to trust the publisher.
pub fn install_driver(dir: &Path) -> Result<()> {
    let inf = dir.join("MttVDD.inf");
    if !inf.exists() {
        bail!("{} not found", inf.display());
    }
    let inf_w = HSTRING::from(inf.as_os_str());
    unsafe {
        if find_device()?.is_none() {
            let mut guid = GUID::zeroed();
            let mut class = [0u16; 64];
            SetupDiGetINFClassW(&inf_w, &mut guid, &mut class, None).context("reading the driver's INF file")?;
            let set = SetupDiCreateDeviceInfoList(Some(&guid), None)?;
            let mut data =
                SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            let created = (|| -> Result<()> {
                SetupDiCreateDeviceInfoW(
                    set,
                    PCWSTR(class.as_ptr()),
                    &guid,
                    PCWSTR::null(),
                    None,
                    DICD_GENERATE_ID,
                    Some(&mut data),
                )
                .context("creating the virtual display device")?;
                // REG_MULTI_SZ: the ID, then two terminating zeros.
                let mut hwid: Vec<u16> = HARDWARE_ID.encode_utf16().collect();
                hwid.extend([0, 0]);
                let bytes: Vec<u8> = hwid.iter().flat_map(|c| c.to_le_bytes()).collect();
                SetupDiSetDeviceRegistryPropertyW(set, &mut data, SPDRP_HARDWAREID, Some(&bytes))?;
                SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, Some(&data))
                    .context("registering the virtual display device")?;
                Ok(())
            })();
            let _ = SetupDiDestroyDeviceInfoList(set);
            created?;
        }
        let mut reboot = windows::core::BOOL(0);
        UpdateDriverForPlugAndPlayDevicesW(
            None::<HWND>,
            &HSTRING::from(HARDWARE_ID),
            &inf_w,
            INSTALLFLAG_FORCE,
            Some(&mut reboot),
        )
        .context("installing the virtual display driver")?;
        if reboot.as_bool() {
            println!("Windows says a restart is needed to finish installing the driver.");
        }
    }
    Ok(())
}

/// Removes the device and the driver package. Needs administrator rights.
pub fn uninstall_driver() -> Result<bool> {
    let mut infs = Vec::new();
    unsafe {
        for_each_device(|set, data| {
            if let Some(inf) = device_info(set, data).inf {
                infs.push(inf);
            }
            SetupDiCallClassInstaller(DIF_REMOVE, set, Some(data)).context("removing the virtual display device")?;
            Ok(false)
        })?;
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        for inf in &infs {
            let path = HSTRING::from(format!("{windir}\\INF\\{inf}"));
            let _ = DiUninstallDriverW(None, &path, DIUNINSTALLDRIVER_FLAGS(0), None);
        }
    }
    Ok(!infs.is_empty())
}
