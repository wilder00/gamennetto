//! Each PC's own identity, and the PCs it trusts.
//!
//! Every PC has one long-lived key and certificate (made the first time
//! Gamennetto runs, kept in `%LOCALAPPDATA%\Gamennetto\identity`). Its
//! **fingerprint** (SHA-256 of the certificate) is what other PCs remember.
//! TLS proves that the other side really holds the key behind its
//! fingerprint, so a remembered PC can't be impersonated.

use anyhow::{bail, Context, Result};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::SERVER_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(cert_der: &[u8]) -> Self {
        let d = ring::digest::digest(&ring::digest::SHA256, cert_der);
        let mut out = [0u8; 32];
        out.copy_from_slice(d.as_ref());
        Self(out)
    }

    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.len() != 64 {
            bail!("a fingerprint has 64 hex digits");
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("not a hex fingerprint")?;
        }
        Ok(Self(out))
    }

    /// Short form for people: the first 8 bytes, e.g. `3f2a 91c0 77de 0b14`.
    pub fn short(&self) -> String {
        let h = self.hex();
        format!("{} {} {} {}", &h[0..4], &h[4..8], &h[8..12], &h[12..16])
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.short())
    }
}

/// This PC's key and certificate.
pub struct Identity {
    pub(crate) cert: CertificateDer<'static>,
    pub(crate) key: PrivatePkcs8KeyDer<'static>,
    pub fingerprint: Fingerprint,
}

impl Identity {
    /// A new identity (tests, or the first run).
    pub fn generate() -> Result<Self> {
        let ck = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_string()])?;
        let cert = ck.cert.der().clone();
        let key = PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der());
        Ok(Self { fingerprint: Fingerprint::of(&cert), cert, key })
    }

    /// Loads this PC's identity from `dir`, making it the first time.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = (dir.join("device.cert.der"), dir.join("device.key.der"));
        if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            let cert = CertificateDer::from(cert);
            return Ok(Self { fingerprint: Fingerprint::of(&cert), cert, key: PrivatePkcs8KeyDer::from(key) });
        }
        let id = Self::generate()?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&key_path, id.key.secret_pkcs8_der())?;
        std::fs::write(&cert_path, id.cert.as_ref())?;
        Ok(id)
    }
}

/// `%LOCALAPPDATA%\Gamennetto` (or a temp folder if that's not set).
/// `GAMENNETTO_DATA_DIR` replaces it: a separate identity and list of
/// approved PCs, e.g. to test pairing on one PC.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GAMENNETTO_DATA_DIR") {
        return PathBuf::from(d);
    }
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Gamennetto")
}

/// Which way the trust goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// PCs this PC may connect to and show.
    Host,
    /// PCs allowed to watch (and, if allowed, control) this PC.
    Viewer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub name: String,
    pub fingerprint: String,
    /// When it was approved (UTC, `YYYY-MM-DD HH:MM`).
    pub added: String,
}

/// PCs approved before, saved in `trusted-devices.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pub hosts: Vec<Device>,
    #[serde(default)]
    pub viewers: Vec<Device>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl TrustStore {
    pub fn load(path: &Path) -> Result<Self> {
        let mut s: Self = match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("reading {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        s.path = Some(path.to_path_buf());
        Ok(s)
    }

    /// The store in the usual place.
    pub fn open() -> Result<Self> {
        Self::load(&data_dir().join("trusted-devices.json"))
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn list(&self, role: Role) -> &Vec<Device> {
        match role {
            Role::Host => &self.hosts,
            Role::Viewer => &self.viewers,
        }
    }

    fn list_mut(&mut self, role: Role) -> &mut Vec<Device> {
        match role {
            Role::Host => &mut self.hosts,
            Role::Viewer => &mut self.viewers,
        }
    }

    pub fn find(&self, role: Role, fp: &Fingerprint) -> Option<&Device> {
        let hex = fp.hex();
        self.list(role).iter().find(|d| d.fingerprint == hex)
    }

    pub fn is_trusted(&self, role: Role, fp: &Fingerprint) -> bool {
        self.find(role, fp).is_some()
    }

    /// A remembered PC with this name but another fingerprint (reinstalled,
    /// or someone pretending to be it).
    pub fn same_name_other_key(&self, role: Role, name: &str, fp: &Fingerprint) -> bool {
        let hex = fp.hex();
        self.list(role).iter().any(|d| d.name.eq_ignore_ascii_case(name) && d.fingerprint != hex)
    }

    pub fn add(&mut self, role: Role, name: &str, fp: &Fingerprint) {
        let hex = fp.hex();
        let list = self.list_mut(role);
        list.retain(|d| d.fingerprint != hex);
        list.push(Device { name: name.to_string(), fingerprint: hex, added: now_utc() });
    }

    /// Removes devices whose name matches, or whose fingerprint starts with
    /// `what` (spaces ignored). Returns how many were removed.
    pub fn remove(&mut self, what: &str) -> usize {
        let key: String = what.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
        if key.is_empty() {
            return 0;
        }
        let mut n = 0;
        for role in [Role::Host, Role::Viewer] {
            let list = self.list_mut(role);
            let before = list.len();
            list.retain(|d| !(d.name.eq_ignore_ascii_case(what.trim()) || d.fingerprint.starts_with(&key)));
            n += before - list.len();
        }
        n
    }
}

fn now_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rest / 3600, rest % 3600 / 60)
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_kept_between_runs() {
        let dir = std::env::temp_dir().join(format!("gn-id-{}", std::process::id()));
        let a = Identity::load_or_create(&dir).unwrap();
        let b = Identity::load_or_create(&dir).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trust_store_roundtrip() {
        let path = std::env::temp_dir().join(format!("gn-trust-{}.json", std::process::id()));
        let fp = Identity::generate().unwrap().fingerprint;
        let other = Identity::generate().unwrap().fingerprint;
        let mut s = TrustStore::load(&path).unwrap();
        s.add(Role::Viewer, "LEGION", &fp);
        s.save().unwrap();
        let mut s = TrustStore::load(&path).unwrap();
        assert!(s.is_trusted(Role::Viewer, &fp));
        assert!(!s.is_trusted(Role::Host, &fp), "trust is one way");
        assert!(s.same_name_other_key(Role::Viewer, "legion", &other));
        assert_eq!(s.remove(&fp.short()), 1);
        assert!(!s.is_trusted(Role::Viewer, &fp));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fingerprint_hex_roundtrip_and_dates() {
        let fp = Identity::generate().unwrap().fingerprint;
        assert_eq!(Fingerprint::from_hex(&fp.hex()).unwrap(), fp);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_782), (2026, 11, 25));
    }
}
