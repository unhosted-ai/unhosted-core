//! The unhosted agent runtime.
//!
//! A **consumer of the inference endpoint** (ADR-0002), not part of the core
//! itself. Drives a model in a tool-calling loop ([`agent`]) with a filesystem
//! sandbox ([`agent_fs`]), a critique gate ([`critique`]), private memory
//! ([`memory`]), and the chat store ([`chats`]).
//!
//! Shared primitives (audit, dlp, metrics, web_fetch, paths) come from
//! [`unhosted_core_base`]; this crate does not depend on the core endpoint's
//! cluster/inference internals.

pub mod agent;
pub mod agent_fs;
pub mod chats;
pub mod critique;
pub mod memory;
