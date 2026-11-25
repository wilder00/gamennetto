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
    let kinds = adapter_kinds();
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            // IPv4 only for now; link-local (169.254.x.x) addresses are not usable.
            IpAddr::V4(v4) if !v4.is_link_local() => Some(Iface {
                is_virtual: is_virtual(&i.name, kinds.iter().find(|k| k.name == i.name)),
                name: i.name,
                addr: v4.to_string(),
                is_private_lan: v4.is_private(),
            }),
            _ => None,
        })
        .collect()
}

/// What Windows says about an adapter.
#[derive(Debug, Clone, PartialEq)]
struct AdapterKind {
    name: String,
    description: String,
    /// IANA interface type: 6 Ethernet, 71 Wi-Fi, 53 virtual (VPNs such as
    /// WireGuard), 131 tunnel, 23 PPP...
    if_type: u32,
}

const IF_ETHERNET: u32 = 6;
const IF_WIFI: u32 = 71;

/// Real Wi-Fi/Ethernet, or not? The adapter type catches VPNs whatever
/// their name ("ProtonVPN" is a WireGuard tunnel, type 53); the name and
/// description catch VM adapters, which call themselves Ethernet.
fn is_virtual(name: &str, kind: Option<&AdapterKind>) -> bool {
    if looks_virtual(name) {
        return true;
    }
    match kind {
        Some(k) => looks_virtual(&k.description) || !matches!(k.if_type, IF_ETHERNET | IF_WIFI),
        None => false,
    }
}

#[cfg(windows)]
fn adapter_kinds() -> Vec<AdapterKind> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GET_ADAPTERS_ADDRESSES_FLAGS, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::AF_INET;

    let mut out = Vec::new();
    let mut size = 16 * 1024u32;
    for _ in 0..3 {
        // u64 elements keep the buffer aligned for the structs inside it.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let first = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let r = unsafe {
            GetAdaptersAddresses(AF_INET.0 as u32, GET_ADAPTERS_ADDRESSES_FLAGS(0), None, Some(first), &mut size)
        };
        if r == 111 {
            continue; // ERROR_BUFFER_OVERFLOW: `size` now says how much is needed
        }
        if r != 0 {
            return out;
        }
        let mut p = first as *const IP_ADAPTER_ADDRESSES_LH;
        while let Some(a) = unsafe { p.as_ref() } {
            let text = |s: windows::core::PWSTR| unsafe { s.to_string().unwrap_or_default() };
            out.push(AdapterKind { name: text(a.FriendlyName), description: text(a.Description), if_type: a.IfType });
            p = a.Next;
        }
        return out;
    }
    out
}

#[cfg(not(windows))]
fn adapter_kinds() -> Vec<AdapterKind> {
    Vec::new()
}

/// Name-based guess; good enough to rank adapters, not a guarantee.
fn looks_virtual(name: &str) -> bool {
    const MARKERS: &[&str] = &[
        "vmware",
        "virtualbox",
        "vbox",
        "hyper-v",
        "vethernet",
        "wsl",
        "docker",
        "loopback",
        "tailscale",
        "zerotier",
        "hamachi",
        "wireguard",
        "openvpn",
        "tap-",
        "tun",
        "npcap",
        "vpn",
        "nordlynx",
        "wan miniport",
    ];
    let lower = name.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str, description: &str, if_type: u32) -> AdapterKind {
        AdapterKind { name: name.into(), description: description.into(), if_type }
    }

    #[test]
    fn detects_virtual_adapters() {
        assert!(looks_virtual("VMware Network Adapter VMnet8"));
        assert!(looks_virtual("vEthernet (WSL)"));
        assert!(!looks_virtual("Wi-Fi"));
        assert!(!looks_virtual("Ethernet 2"));
    }

    #[test]
    fn vpns_are_virtual_whatever_their_name() {
        // A WireGuard VPN named after its company: type 53.
        let k = kind("Mi conexión", "WireGuard Tunnel", 53);
        assert!(is_virtual("Mi conexión", Some(&k)));
        // VMware calls itself Ethernet; its description gives it away.
        let k = kind("Ethernet 3", "VMware Virtual Ethernet Adapter for VMnet8", 6);
        assert!(is_virtual("Ethernet 3", Some(&k)));
        let k = kind("Wi-Fi", "Intel(R) Wi-Fi 6E AX211 160MHz", 71);
        assert!(!is_virtual("Wi-Fi", Some(&k)));
        let k = kind("Ethernet", "Realtek PCIe GbE Family Controller", 6);
        assert!(!is_virtual("Ethernet", Some(&k)));
        assert!(!is_virtual("Ethernet", None));
    }

    #[cfg(windows)]
    #[test]
    fn reads_adapter_types() {
        // Every PC that runs the tests has at least one adapter with a name.
        assert!(adapter_kinds().iter().any(|k| !k.name.is_empty()));
    }
}
