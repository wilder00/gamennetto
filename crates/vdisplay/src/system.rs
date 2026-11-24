//! Administrator rights, the settings folder, and the session lock.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{
    CreateMutexW, GetExitCodeProcess, OpenMutexW, WaitForSingleObject, INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::settings::Settings;

pub const REG_KEY: &str = "SOFTWARE\\MikeTheTech\\VirtualDisplayDriver";

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

/// Runs this program again with administrator rights (Windows shows its
/// "allow changes?" prompt) and waits for it. Returns its exit code.
pub fn run_elevated(args: &str) -> Result<u32> {
    let exe = std::env::current_exe()?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).context("administrator permission was not given")?;
        if info.hProcess.is_invalid() {
            bail!("could not start the administrator step");
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Ok(code)
    }
}

/// Folder for the driver's settings. Gamennetto points the driver here
/// (registry value VDDPATH) and makes it writable for normal users, so
/// adding a new size later doesn't need administrator rights.
pub fn settings_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| "C:\\ProgramData".into());
    base.join("Gamennetto").join("VirtualDisplay")
}

/// Where the installed driver reads its settings from, if configured.
pub fn configured_settings_dir() -> Option<PathBuf> {
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let key = HSTRING::from(REG_KEY);
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            w!("VDDPATH"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

pub fn settings_file(dir: &Path) -> PathBuf {
    dir.join("vdd_settings.xml")
}

pub fn read_settings() -> Option<Settings> {
    let dir = configured_settings_dir()?;
    let xml = std::fs::read_to_string(settings_file(&dir)).ok()?;
    Some(Settings::parse(&xml))
}

pub fn write_settings(dir: &Path, s: &Settings) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(settings_file(dir), s.to_xml()).with_context(|| format!("writing {}", settings_file(dir).display()))
}

/// Held while a Gamennetto host is using the virtual monitor, so other
/// Gamennetto commands know not to detach it.
pub struct SessionLock(HANDLE);

const LOCK_NAME: PCWSTR = w!("Local\\Gamennetto.VirtualDisplay");

impl SessionLock {
    /// None if another Gamennetto host already holds it.
    pub fn acquire() -> Option<Self> {
        unsafe {
            let h = CreateMutexW(None, false, LOCK_NAME).ok()?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(h);
                return None;
            }
            Some(Self(h))
        }
    }

    pub fn held_elsewhere() -> bool {
        unsafe {
            match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, LOCK_NAME) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    true
                }
                Err(_) => false,
            }
        }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
