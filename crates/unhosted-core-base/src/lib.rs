//! Shared primitives for the unhosted workspace.
//!
//! This is the small kernel that both `unhosted-core` (the inference endpoint)
//! and `unhosted-agent` (the agent runtime, a consumer of the endpoint) depend
//! on. Pulling these modules out of `unhosted-core` breaks the dependency cycle
//! that would otherwise exist once the agent is its own crate
//! (agent → audit/dlp/web_fetch/paths/metrics, while core's daemon → agent).
//!
//! See `ARCHITECTURE.md` (extraction slice 1).

pub mod audit;
pub mod dlp;
pub mod metrics;
pub mod paths;
pub mod web_fetch;
