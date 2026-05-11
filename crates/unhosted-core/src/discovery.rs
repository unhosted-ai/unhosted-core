//! mDNS / Bonjour discovery for zero-config peer discovery on a LAN.
//!
//! Each `unhosted serve` registers itself as `_unhosted._tcp.local.` so other
//! daemons on the same network can find it without manual `peer add`. The
//! daemon also browses the same service type, building a live list of
//! reachable peers that the UI can offer for pairing.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;

pub const SERVICE_TYPE: &str = "_unhosted._tcp.local.";

/// A peer the daemon has discovered on the LAN but hasn't necessarily paired
/// with yet. Pairing moves it into the persistent peer registry.
#[derive(Clone, Debug, Serialize)]
pub struct DiscoveredPeer {
    pub name: String,
    pub addr: SocketAddr,
    pub host: Option<String>,
    pub version: Option<String>,
    /// Unix-ms timestamp of when we last heard from this peer.
    pub last_seen_ms: u64,
}

#[derive(Clone)]
pub struct Discovery {
    inner: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    /// Hold the daemon handle alive for the lifetime of the daemon.
    /// Dropping it tears down both the registration and the browse.
    _handle: Arc<ServiceDaemon>,
}

impl Discovery {
    /// Start mDNS: register the local node + begin browsing. Returns a clone-
    /// able handle to query the currently discovered peers.
    ///
    /// `local_addr` is the address the local daemon is listening on. Used to
    /// filter ourselves out of the discovered list.
    pub fn start(name: &str, local_addr: SocketAddr, version: &str) -> anyhow::Result<Self> {
        let mdns = ServiceDaemon::new().map_err(|e| anyhow::anyhow!("mdns init: {e}"))?;

        // ----- register the local node -----
        let mut props = HashMap::new();
        props.insert("version".to_string(), version.to_string());

        let mut info = ServiceInfo::new(
            SERVICE_TYPE,
            name,
            &format!("{}.local.", sanitize_host(name)),
            "",
            local_addr.port(),
            Some(props),
        )
        .map_err(|e| anyhow::anyhow!("mdns service info: {e}"))?;
        info = info.enable_addr_auto();
        mdns.register(info)
            .map_err(|e| anyhow::anyhow!("mdns register: {e}"))?;

        // ----- browse for peers -----
        let receiver = mdns
            .browse(SERVICE_TYPE)
            .map_err(|e| anyhow::anyhow!("mdns browse: {e}"))?;

        let map: Arc<Mutex<HashMap<String, DiscoveredPeer>>> = Arc::new(Mutex::new(HashMap::new()));
        let map_clone = map.clone();
        let local_port = local_addr.port();
        let local_name = name.to_string();

        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let svc_name = info
                            .get_fullname()
                            .strip_suffix(&format!(".{SERVICE_TYPE}"))
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| info.get_fullname().to_string());

                        // Skip our own announcement.
                        if svc_name == local_name && info.get_port() == local_port {
                            continue;
                        }

                        let addr_opt: Option<IpAddr> = info.get_addresses().iter().next().copied();
                        let Some(ip) = addr_opt else { continue };
                        let sock = SocketAddr::new(ip, info.get_port());

                        let version = info
                            .get_property("version")
                            .map(|p| p.val_str().to_string());

                        let peer = DiscoveredPeer {
                            name: svc_name.clone(),
                            addr: sock,
                            host: Some(info.get_hostname().to_string()),
                            version,
                            last_seen_ms: now_ms(),
                        };

                        if let Ok(mut m) = map_clone.lock() {
                            m.insert(svc_name, peer);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        let svc_name = fullname
                            .strip_suffix(&format!(".{SERVICE_TYPE}"))
                            .map(|s| s.to_string())
                            .unwrap_or(fullname);
                        if let Ok(mut m) = map_clone.lock() {
                            m.remove(&svc_name);
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            inner: map,
            _handle: Arc::new(mdns),
        })
    }

    /// Snapshot of currently-discovered peers. Drops anything not seen in
    /// the last 60s so the UI doesn't show ghosts.
    pub fn snapshot(&self) -> Vec<DiscoveredPeer> {
        let cutoff = now_ms().saturating_sub(60_000);
        let m = match self.inner.lock() {
            Ok(m) => m,
            Err(_) => return vec![],
        };
        let mut peers: Vec<DiscoveredPeer> = m
            .values()
            .filter(|p| p.last_seen_ms >= cutoff)
            .cloned()
            .collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        peers
    }
}

fn now_ms() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn sanitize_host(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Best-effort hostname for the local node, with a sensible fallback.
pub fn default_node_name() -> String {
    if let Some(envvar) = std::env::var_os("UNHOSTED_NODE_NAME") {
        if let Ok(s) = envvar.into_string() {
            if !s.is_empty() {
                return s;
            }
        }
    }
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|h| {
            // strip ".local" suffix that macOS often adds
            h.strip_suffix(".local").unwrap_or(&h).to_string()
        })
        .unwrap_or_else(|| "unhosted".to_string())
}
