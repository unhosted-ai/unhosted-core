//! Routing across local + peers for v0.0.2 request distribution.
//!
//! v0.0.3 update: targets are stored behind an `RwLock` so peers can be
//! added or removed at runtime (via the UI or future hot-reload signals)
//! without restarting the daemon.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

use crate::peer::Peer;

/// Where a single request should be served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Local,
    Peer { name: String, addr: SocketAddr },
}

/// Round-robin router across all available targets. Hot-reloadable.
pub struct Router {
    targets: RwLock<Vec<Target>>,
    counter: AtomicUsize,
}

impl Router {
    pub fn new(peers: &[Peer]) -> Self {
        Self {
            targets: RwLock::new(build_targets(peers)),
            counter: AtomicUsize::new(0),
        }
    }

    /// Return the next target in round-robin order.
    pub fn next(&self) -> Target {
        let targets = self.targets.read().expect("router lock");
        let i = self.counter.fetch_add(1, Ordering::Relaxed);
        targets[i % targets.len()].clone()
    }

    /// Replace the peer list. Counter keeps advancing so the rotation
    /// stays smooth across reloads.
    pub fn replace_peers(&self, peers: &[Peer]) {
        let new_targets = build_targets(peers);
        *self.targets.write().expect("router lock") = new_targets;
    }

    pub fn target_count(&self) -> usize {
        self.targets.read().expect("router lock").len()
    }

    pub fn has_peers(&self) -> bool {
        self.target_count() > 1
    }
}

fn build_targets(peers: &[Peer]) -> Vec<Target> {
    let mut sorted: Vec<&Peer> = peers.iter().collect();
    sorted.sort_by_key(|p| p.priority);

    let mut targets = Vec::with_capacity(1 + sorted.len());
    targets.push(Target::Local);
    for peer in sorted {
        targets.push(Target::Peer {
            name: peer.name.clone(),
            addr: peer.addr,
        });
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(name: &str, port: u16, priority: u8) -> Peer {
        Peer {
            name: name.into(),
            addr: format!("127.0.0.1:{port}").parse().unwrap(),
            priority,
            models: vec![],
            pubkey: None,
        }
    }

    #[test]
    fn local_only_when_no_peers() {
        let r = Router::new(&[]);
        assert_eq!(r.target_count(), 1);
        assert_eq!(r.next(), Target::Local);
        assert_eq!(r.next(), Target::Local);
        assert!(!r.has_peers());
    }

    #[test]
    fn rotates_across_local_and_peers_in_priority_order() {
        let r = Router::new(&[peer("late", 7779, 10), peer("early", 7778, 1)]);
        assert_eq!(r.target_count(), 3);
        assert!(r.has_peers());

        let seen: Vec<Target> = (0..6).map(|_| r.next()).collect();
        let expected = [
            Target::Local,
            Target::Peer {
                name: "early".into(),
                addr: "127.0.0.1:7778".parse().unwrap(),
            },
            Target::Peer {
                name: "late".into(),
                addr: "127.0.0.1:7779".parse().unwrap(),
            },
        ];
        assert_eq!(seen[0..3], expected[..]);
        assert_eq!(seen[3..6], expected[..]);
    }

    #[test]
    fn replace_peers_swaps_routing() {
        let r = Router::new(&[]);
        assert_eq!(r.target_count(), 1);
        r.replace_peers(&[peer("alpha", 7778, 5)]);
        assert_eq!(r.target_count(), 2);
        let seen: Vec<Target> = (0..2).map(|_| r.next()).collect();
        assert!(seen.contains(&Target::Local));
        assert!(seen
            .iter()
            .any(|t| matches!(t, Target::Peer { name, .. } if name == "alpha")));
    }
}
