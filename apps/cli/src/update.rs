//! `gamennetto update`: installs the newest release from GitHub.
//!
//! Each release has `latest.json` (the same layout Tauri's updater uses):
//! `{"version": "0.2.0", "platforms": {"windows-x86_64": {"url": ..., "signature": ...}}}`.
//! The installer is checked against a minisign signature made with the
//! project's private key (kept as a GitHub secret). The public key is built
//! into release binaries (`GAMENNETTO_UPDATE_PUBKEY`), so a file swapped on
//! the way, or on GitHub, is refused.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

const LATEST_URL: &str = "https://github.com/wilder00/gamennetto/releases/latest/download/latest.json";
const PLATFORM: &str = "windows-x86_64";
/// Set by the release workflow; local builds have none and can't self-update.
const PUBKEY: Option<&str> = option_env!("GAMENNETTO_UPDATE_PUBKEY");

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "1.2.3" (or "v1.2.3") -> (1, 2, 3). Anything after a '-' is ignored.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let s = s.split(['-', '+']).next()?;
    let mut it = s.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

struct Release {
    version: String,
    url: String,
    signature: String,
}

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("curl.exe")
        .args(["-L", "--fail", "--silent", "--show-error"])
        .args(args)
        .output()
        .context("running curl.exe")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

fn latest(timeout_s: u32) -> Result<Release> {
    let body = match curl(&["--max-time", &timeout_s.to_string(), LATEST_URL]) {
        Ok(b) => b,
        Err(e) if e.to_string().contains("404") => bail!("no version has been published on GitHub yet"),
        Err(e) => return Err(e.context("checking for updates")),
    };
    let j: serde_json::Value = serde_json::from_slice(&body).context("reading latest.json")?;
    let p = &j["platforms"][PLATFORM];
    let get = |v: &serde_json::Value| v.as_str().map(str::to_string);
    Ok(Release {
        version: get(&j["version"]).context("latest.json has no version")?,
        url: get(&p["url"]).with_context(|| format!("latest.json has no {PLATFORM} installer"))?,
        signature: get(&p["signature"]).context("latest.json has no signature")?,
    })
}

/// Checks a minisign signature: both the key and the signature are base64
/// of the text minisign writes (see tools/xtask).
fn verify(data: &[u8], signature_b64: &str, pubkey_b64: &str) -> Result<()> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let text = |s: &str| -> Result<String> { Ok(String::from_utf8(b64.decode(s.trim())?)?) };
    let pk = minisign_verify::PublicKey::decode(&text(pubkey_b64)?).context("bad update public key")?;
    let sig = minisign_verify::Signature::decode(&text(signature_b64)?).context("bad update signature")?;
    pk.verify(data, &sig, false).context("the installer's signature does not match: not installing it")?;
    Ok(())
}

pub fn run(check_only: bool) -> Result<()> {
    let current = current_version();
    println!("Installed: Gamennetto {current}");
    let r = latest(20)?;
    if !is_newer(&r.version, current) {
        println!("You have the newest version ({}).", r.version.trim_start_matches('v'));
        return Ok(());
    }
    println!("New version available: {}", r.version.trim_start_matches('v'));
    if check_only {
        println!("Run `gamennetto update` to install it.");
        return Ok(());
    }
    let Some(pubkey) = PUBKEY else {
        bail!(
            "this copy was built by hand, so it can't update itself.\n\
             Download the installer from https://github.com/wilder00/gamennetto/releases/latest"
        );
    };
    if std::env::current_exe()?.components().any(|c| c.as_os_str() == "target") {
        bail!("this is a development build (inside target\\); update it with git and cargo instead");
    }

    println!("Downloading {} ...", r.url);
    let data = curl(&["--max-time", "600", &r.url])?;
    verify(&data, &r.signature, pubkey)?;
    println!("Signature OK.");

    let file = std::env::temp_dir().join(format!("Gamennetto-{}-setup.exe", r.version.trim_start_matches('v')));
    std::fs::write(&file, &data)?;
    // The installer replaces this program, so it must not be running:
    // start the installer on its own and quit right away.
    Command::new(&file)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
        .spawn()
        .context("starting the installer")?;
    println!("Installing Gamennetto {} in the background. It's ready when the installer window closes.", r.version);
    Ok(())
}

fn stamp_file() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Gamennetto").join("last-update-check")
}

/// At most once a day, and only for installed release builds: says so if a
/// newer version exists. Runs in the background and never blocks.
pub fn notify_in_background() {
    if PUBKEY.is_none() {
        return;
    }
    let stamp = stamp_file();
    let recent = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(24 * 3600));
    if recent {
        return;
    }
    std::thread::spawn(move || {
        if let Some(dir) = stamp.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&stamp, b"");
        if let Ok(r) = latest(5) {
            if is_newer(&r.version, current_version()) {
                println!(
                    "\n  Gamennetto {} is available (you have {}). Run `gamennetto update` to install it.\n",
                    r.version.trim_start_matches('v'),
                    current_version()
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("v0.12.3"), Some((0, 12, 3)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v0.10.0", "0.9.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("junk", "0.1.0"));
    }

    /// Signs like `xtask sign` (base64 of minisign's text) and checks.
    #[test]
    fn verifies_minisign_signatures() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD;
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk = b64.encode(kp.pk.to_box().unwrap().to_string());
        let data = b"installer bytes".to_vec();
        let sig = minisign::sign(Some(&kp.pk), &kp.sk, std::io::Cursor::new(&data), None, None).unwrap();
        let sig = b64.encode(sig.to_string());
        assert!(verify(&data, &sig, &pk).is_ok());
        assert!(verify(b"tampered bytes", &sig, &pk).is_err());
    }

    /// End-to-end check with files made by `xtask keygen` + `xtask sign`.
    /// Runs only when GAMENNETTO_TEST_SIGNED_FILE points at a signed file
    /// (with FILE.sig next to it) and GAMENNETTO_TEST_PUBKEY_FILE at the key.
    #[test]
    fn verifies_files_signed_by_xtask() {
        let (Ok(file), Ok(pk)) =
            (std::env::var("GAMENNETTO_TEST_SIGNED_FILE"), std::env::var("GAMENNETTO_TEST_PUBKEY_FILE"))
        else {
            return;
        };
        let data = std::fs::read(&file).unwrap();
        let sig = std::fs::read_to_string(format!("{file}.sig")).unwrap();
        let pk = std::fs::read_to_string(pk).unwrap();
        verify(&data, &sig, &pk).unwrap();
        let mut bad = data.clone();
        bad.push(b'!');
        assert!(verify(&bad, &sig, &pk).is_err());
    }
}
