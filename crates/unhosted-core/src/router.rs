//! Routing across local + peers for v0.0.2 request distribution.
//!
//! The router maintains an ordered list of targets (`Local` plus each
//! configured `Peer`, priority-ascending) and picks one per request via a
//! lock-free atomic counter (round-robin).
//!
//! Failure handling lives in the call site: if a chosen peer is unreachable,
//! the caller falls back to `Target::Local`. The router itself stays
//! stateless about peer health — that comes in v0.0.3 with health-aware
//! routing.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::peer::Peer;

/// Where a single request should be served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Serve locally — proxy to this node's own upstream llama-server.
    Local,
    /// Forward to a remote peer's `/v1/run` over HTTP.
    Peer { name: String, addr: SocketAddr },
}

/// Round-robin router across all available targets.
pub struct Router {
    targets: Vec<Target>,
    counter: AtomicUsize,
}

impl Router {
    /// Build a router whose first target is always `Local`, followed by all
    /// peers in priority-ascending order (lower priority = preferred).
    pub fn new(peers: &[Peer]) -> Self {
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

        Self {
            targets,
            counter: AtomicUsize::new(0),
        }
    }

    /// Return the next target in round-robin order. Always returns a value;
    /// the local-only case is just a one-element rotation.
    pub fn next(&self) -> Target {
        let i = self.counter.fetch_add(1, Ordering::Relaxed);
        self.targets[i % self.targets.len()].clone()
    }

    /// How many targets are in the rotation (including `Local`).
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Whether any peers are configured (i.e., would the router ever route
    /// off-box).
    pub fn has_peers(&self) -> bool {
        self.targets.len() > 1
    }
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
        // Two full rotations
        assert_eq!(seen[0..3], expected[..]);
        assert_eq!(seen[3..6], expected[..]);
    }
}
