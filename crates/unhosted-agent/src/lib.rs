//! The unhosted agent runtime.
//!
//! A **consumer of the inference endpoint** (ADR-0002), not part of the core
//! itself. Drives a model in a tool-calling loop ([`agent`]) with a filesystem
//! sandbox ([`agent_fs`]), a critique gate ([`critique`]), private memory
//! ([`memory`]), the chat store ([`chats`]), an optional Cognitive Twin persona
//! ([`persona`]) that lets the agent reason and speak as a specific person when
//! the user enables it, and an optional cloned-voice bridge ([`voice`]) that
//! renders replies in that person's actual voice.
//!
//! Shared primitives (audit, dlp, metrics, web_fetch, paths) come from
//! [`unhosted_core_base`]; this crate does not depend on the core endpoint's
//! cluster/inference internals.

pub mod agent;
pub mod agent_fs;
pub mod chats;
pub mod critique;
pub mod memory;
pub mod persona;
pub mod voice;
