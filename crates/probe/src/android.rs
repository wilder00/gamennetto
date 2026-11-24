use serde::Serialize;
use std::path::PathBuf;

/// Only needed for USB mode and for building the Android app. Not required
/// to use the PC side over Wi-Fi.
#[derive(Debug, Clone, Serialize)]
pub struct AndroidTools {
    pub adb: Option<String>,
    pub sdk_dir: Option<String>,
}

pub fn detect() -> AndroidTools {
    let sdk_dir = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|p| p.is_dir());

    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut candidates: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join(exe)).collect()).unwrap_or_default();
    if let Some(sdk) = &sdk_dir {
        candidates.push(sdk.join("platform-tools").join(exe));
    }

    AndroidTools {
        adb: candidates.into_iter().find(|p| p.is_file()).map(|p| p.display().to_string()),
        sdk_dir: sdk_dir.map(|p| p.display().to_string()),
    }
}
