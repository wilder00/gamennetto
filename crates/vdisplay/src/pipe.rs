//! Commands to the running driver over its named pipe.

use anyhow::{bail, Result};
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Pipes::WaitNamedPipeW;

const PIPE: windows::core::PCWSTR = w!("\\\\.\\pipe\\MTTVirtualDisplayPipe");

fn open() -> Result<HANDLE> {
    for _ in 0..3 {
        match unsafe {
            CreateFileW(
                PIPE,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        } {
            Ok(h) => return Ok(h),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => unsafe {
                let _ = WaitNamedPipeW(PIPE, 2000);
            },
            Err(e) => bail!("the virtual display driver is not running ({e})"),
        }
    }
    bail!("the virtual display driver is busy")
}

/// True if the driver is loaded and answering.
pub fn driver_running() -> bool {
    match open() {
        Ok(h) => {
            unsafe {
                let _ = CloseHandle(h);
            }
            true
        }
        Err(_) => false,
    }
}

/// Sends one command (e.g. `RELOAD_DRIVER`) and returns whatever the driver
/// writes back within `wait`.
pub fn send_command(cmd: &str, wait: Duration) -> Result<String> {
    let h = open()?;
    let bytes: Vec<u8> = cmd.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let mut written = 0;
    let r = unsafe { WriteFile(h, Some(&bytes), Some(&mut written), None) };
    if let Err(e) = r {
        unsafe {
            let _ = CloseHandle(h);
        }
        bail!("sending '{cmd}' to the virtual display driver: {e}");
    }
    // The driver answers (if at all) and then hangs up. Read on another
    // thread so a silent driver can't block us.
    let raw = h.0 as usize;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let h = HANDLE(raw as *mut _);
        let mut out = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let mut n = 0;
            let ok = unsafe { ReadFile(h, Some(&mut buf), Some(&mut n), None) };
            if n > 0 {
                out.extend_from_slice(&buf[..n as usize]);
            }
            if ok.is_err() || n == 0 {
                break;
            }
        }
        unsafe {
            let _ = CloseHandle(h);
        }
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(wait).unwrap_or_default();
    Ok(String::from_utf8_lossy(&out).into_owned())
}
