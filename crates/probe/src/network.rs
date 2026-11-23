use serde::Serialize;
use std::net::IpAddr;

#[derive(Debug, Clone, Serialize)]
pub struct Iface {
    pub name: String,
    pub addr: String,
    /// Address in a private LAN range (10/8, 172.16/12, 192.168/16).
    pub is_private_lan: bool,
    /// Adapter created by software (VMs, containers, VPNs), not a real
    /// Wi-Fi/Ethernet connection. Other devices can't reach these.
    pub is_virtual: bool,
}

pub fn detect() -> Vec<Iface> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            // IPv4 only for now; link-local (169.254.x.x) addresses are not usable.
            IpAddr::V4(v4) if !v4.is_link_local() => Some(Iface {
                is_virtual: looks_virtual(&i.name),
                name: i.name,
                addr: v4.to_string(),
                is_private_lan: v4.is_private(),
            }),
            _ => None,
        })
        .collect()
}

/// Name-based guess; good enough to rank adapters, not a guarantee.
fn looks_virtual(name: &str) -> bool {
    const MARKERS: &[&str] = &[
        "vmware", "virtualbox", "vbox", "hyper-v", "vethernet", "wsl", "docker",
        "loopback", "tailscale", "zerotier", "hamachi", "wireguard", "openvpn",
        "tap-", "tun", "npcap",
    ];
    let lower = name.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::looks_virtual;

    #[test]
    fn detects_virtual_adapters() {
        assert!(looks_virtual("VMware Network Adapter VMnet8"));
        assert!(looks_virtual("vEthernet (WSL)"));
        assert!(!looks_virtual("Wi-Fi"));
        assert!(!looks_virtual("Ethernet 2"));
    }
}
