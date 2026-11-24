//! Finding hosts on the local network with mDNS (the same mechanism
//! printers and Chromecasts use), so nobody has to type IP addresses.

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

const SERVICE: &str = "_gamennetto._udp.local.";

#[derive(Debug, Clone)]
pub struct FoundHost {
    pub name: String,
    pub addr: SocketAddr,
}

/// Keeps the host visible on the network while alive.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Announces a host called `name` at `ip:port`.
pub fn advertise(name: &str, ip: Ipv4Addr, port: u16) -> Result<Advertisement> {
    let daemon = ServiceDaemon::new()?;
    let instance: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let host = format!("{instance}.local.");
    let props = [("name", name)];
    let info = ServiceInfo::new(SERVICE, &instance, &host, IpAddr::V4(ip), port, &props[..])?;
    let fullname = info.get_fullname().to_string();
    daemon.register(info)?;
    Ok(Advertisement { daemon, fullname })
}

/// Looks for hosts for `wait`, then returns what it found.
pub fn browse(wait: Duration) -> Result<Vec<FoundHost>> {
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE)?;
    let end = Instant::now() + wait;
    let mut found: HashMap<String, FoundHost> = HashMap::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        let Ok(ev) = rx.recv_timeout(left) else { break };
        if let ServiceEvent::ServiceResolved(info) = ev {
            let name = info.get_property_val_str("name").unwrap_or(info.get_fullname()).to_string();
            let ip = info.get_addresses_v4().into_iter().next();
            if let Some(ip) = ip {
                let addr = SocketAddr::new(IpAddr::V4(ip), info.get_port());
                found.insert(info.get_fullname().to_string(), FoundHost { name, addr });
            }
        }
    }
    let _ = daemon.shutdown();
    let mut v: Vec<_> = found.into_values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}
