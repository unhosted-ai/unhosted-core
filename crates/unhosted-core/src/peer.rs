//! Peer registry — the v0.0.2 substrate for multi-node clusters.
//!
//! A `Peer` is another `unhosted` daemon reachable over the network. The
//! registry persists to `~/.config/unhosted/peers.toml` (XDG-respecting on
//! Linux/macOS) so peer relationships survive restarts.
//!
//! v0.0.2 only uses this for request distribution. Layer splitting (true
//! VRAM pooling via llama.cpp's RPC backend) lands in v0.0.3+ on the same
//! types — see [`design/0003-two-node-lan-cluster.md`].

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A remote `unhosted` daemon the local node can route requests to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peer {
    /// Human-readable name. Unique within the registry.
    pub name: String,
    /// Address of the peer's HTTP API (default daemon port: 7777).
    pub addr: SocketAddr,
    /// Lower priorities are preferred. Default 10.
    #[serde(default = "default_priority")]
    pub priority: u8,
    /// Models the peer claims to serve. Empty means "ask before assuming."
    #[serde(default)]
    pub models: Vec<String>,
}

fn default_priority() -> u8 {
    10
}

/// Disk-backed list of peers. Persists to TOML on every mutation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PeerRegistry {
    #[serde(default)]
    pub peers: Vec<Peer>,
}

impl PeerRegistry {
    /// Where the registry is stored on disk.
    /// Honors `XDG_CONFIG_HOME`; falls back to `~/.config/unhosted/peers.toml`.
    pub fn config_path() -> Result<PathBuf> {
        let dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg)
        } else {
            let home = std::env::var("HOME").context("HOME env var not set")?;
            PathBuf::from(home).join(".config")
        };
        Ok(dir.join("unhosted").join("peers.toml"))
    }

    /// Load from disk. Returns an empty registry if no config file exists.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Persist to disk, creating the config directory if needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing peers")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Add or replace a peer by name. Persists immediately.
    pub fn add(&mut self, peer: Peer) -> Result<()> {
        if let Some(existing) = self.peers.iter_mut().find(|p| p.name == peer.name) {
            *existing = peer;
        } else {
            self.peers.push(peer);
        }
        self.save()
    }

    /// Remove a peer by name. Returns true if a peer was removed.
    pub fn remove(&mut self, name: &str) -> Result<bool> {
        let before = self.peers.len();
        self.peers.retain(|p| p.name != name);
        let removed = self.peers.len() < before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Peers sorted by ascending priority (preferred first).
    pub fn by_priority(&self) -> Vec<&Peer> {
        let mut v: Vec<&Peer> = self.peers.iter().collect();
        v.sort_by_key(|p| p.priority);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_replaces_by_name() {
        let mut reg = PeerRegistry::default();
        // Use a dummy save by setting XDG_CONFIG_HOME to a temp dir.
        let tmp = std::env::temp_dir().join(format!("unhosted-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &tmp);

        reg.add(Peer {
            name: "thunder".into(),
            addr: "127.0.0.1:7778".parse().unwrap(),
            priority: 1,
            models: vec![],
        })
        .unwrap();

        reg.add(Peer {
            name: "thunder".into(),
            addr: "127.0.0.1:7779".parse().unwrap(),
            priority: 2,
            models: vec!["llama3.2:1b".into()],
        })
        .unwrap();

        assert_eq!(reg.peers.len(), 1);
        assert_eq!(reg.peers[0].priority, 2);
        assert_eq!(reg.peers[0].models, vec!["llama3.2:1b".to_string()]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn by_priority_sorts_ascending() {
        let reg = PeerRegistry {
            peers: vec![
                Peer {
                    name: "a".into(),
                    addr: "127.0.0.1:1".parse().unwrap(),
                    priority: 5,
                    models: vec![],
                },
                Peer {
                    name: "b".into(),
                    addr: "127.0.0.1:2".parse().unwrap(),
                    priority: 1,
                    models: vec![],
                },
                Peer {
                    name: "c".into(),
                    addr: "127.0.0.1:3".parse().unwrap(),
                    priority: 3,
                    models: vec![],
                },
            ],
        };
        let names: Vec<&str> = reg.by_priority().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
    }
}
