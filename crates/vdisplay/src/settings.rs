//! The driver's settings file (`vdd_settings.xml`) and which driver we use.

/// Pinned driver release. Changing it means checking the new release and
/// updating the hash.
pub const DRIVER_VERSION: &str = "25.7.23";
pub const DRIVER_URL: &str = "https://github.com/VirtualDrivers/Virtual-Display-Driver/releases/download/25.7.23/VirtualDisplayDriver-x86.Driver.Only.zip";
pub const DRIVER_SHA256: &str = "e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a";
/// Hardware ID of the driver's root device (from MttVDD.inf).
pub const HARDWARE_ID: &str = "Root\\MttVDD";

/// Sizes offered to Windows by default. Any other size a viewer needs is
/// added when it connects.
pub const COMMON_SIZES: &[(u32, u32)] = &[
    (1280, 720),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (1920, 1200),
    (2560, 1440),
    (2560, 1600),
    (2880, 1800),
    (3840, 2160),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// GPU that draws the virtual monitor (its Windows name). It must be the
    /// GPU whose hardware encoder we use, to avoid copies between GPUs.
    pub gpu: String,
    pub sizes: Vec<(u32, u32)>,
    pub refresh: u32,
}

impl Settings {
    pub fn new(gpu: &str) -> Self {
        Self { gpu: gpu.to_string(), sizes: COMMON_SIZES.to_vec(), refresh: 60 }
    }

    pub fn has_size(&self, w: u32, h: u32) -> bool {
        self.sizes.contains(&(w, h))
    }

    /// Adds a size; returns false if it was already there.
    pub fn add_size(&mut self, w: u32, h: u32) -> bool {
        if self.has_size(w, h) {
            return false;
        }
        self.sizes.push((w, h));
        true
    }

    pub fn to_xml(&self) -> String {
        let mut res = String::new();
        for (w, h) in &self.sizes {
            res += &format!(
                "        <resolution>\n            <width>{w}</width>\n            <height>{h}</height>\n            <refresh_rate>{}</refresh_rate>\n        </resolution>\n",
                self.refresh
            );
        }
        format!(
            "<?xml version='1.0' encoding='utf-8'?>\n\
<!-- Written by Gamennetto. Changes here are replaced when Gamennetto needs a new size. -->\n\
<vdd_settings>\n\
    <monitors>\n        <count>1</count>\n    </monitors>\n\
    <gpu>\n        <friendlyname>{}</friendlyname>\n    </gpu>\n\
    <global>\n        <g_refresh_rate>{}</g_refresh_rate>\n    </global>\n\
    <resolutions>\n{res}    </resolutions>\n\
    <options>\n\
        <CustomEdid>false</CustomEdid>\n\
        <PreventSpoof>false</PreventSpoof>\n\
        <EdidCeaOverride>false</EdidCeaOverride>\n\
        <HardwareCursor>false</HardwareCursor>\n\
        <SDR10bit>false</SDR10bit>\n\
        <HDRPlus>false</HDRPlus>\n\
        <logging>false</logging>\n\
        <debuglogging>false</debuglogging>\n\
    </options>\n\
</vdd_settings>\n",
            escape(&self.gpu),
            self.refresh
        )
    }

    /// Reads a settings file written by us (or a similar one).
    pub fn parse(xml: &str) -> Self {
        let gpu = between(xml, "<friendlyname>", "</friendlyname>").map(unescape).unwrap_or_else(|| "default".into());
        let refresh =
            between(xml, "<refresh_rate>", "</refresh_rate>").and_then(|s| s.trim().parse().ok()).unwrap_or(60);
        let mut sizes = Vec::new();
        let mut rest = xml;
        while let Some(i) = rest.find("<resolution>") {
            rest = &rest[i + 12..];
            let end = rest.find("</resolution>").unwrap_or(rest.len());
            let block = &rest[..end];
            let w = between(block, "<width>", "</width>").and_then(|s| s.trim().parse().ok());
            let h = between(block, "<height>", "</height>").and_then(|s| s.trim().parse().ok());
            if let (Some(w), Some(h)) = (w, h) {
                if !sizes.contains(&(w, h)) {
                    sizes.push((w, h));
                }
            }
            rest = &rest[end..];
        }
        Self { gpu, sizes, refresh }
    }
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let len = s[start..].find(close)?;
    Some(&s[start..start + len])
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn unescape(s: &str) -> String {
    s.trim().replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// SHA-256 of a file's bytes, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_roundtrip() {
        let mut s = Settings::new("Intel(R) Iris(R) Xe Graphics & <co>");
        assert!(s.add_size(2400, 1080));
        assert!(!s.add_size(1920, 1080));
        let back = Settings::parse(&s.to_xml());
        assert_eq!(back, s);
    }

    #[test]
    fn parses_the_drivers_own_file() {
        let xml = "<vdd_settings><gpu><friendlyname>default</friendlyname></gpu>\
            <resolutions><resolution><width>800</width><height>600</height>\
            <refresh_rate>30</refresh_rate></resolution></resolutions></vdd_settings>";
        let s = Settings::parse(xml);
        assert_eq!(s.gpu, "default");
        assert_eq!(s.sizes, vec![(800, 600)]);
        assert_eq!(s.refresh, 30);
    }

    #[test]
    fn hashes() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
