//! Agent API integration tests.
//!
//! Shared fixtures live in [`fixtures`]; concern-specific cases are split into
//! sibling modules. Nested modules import via `use super::*`.

use super::*;

#[allow(unused_imports)]
pub(crate) use crate::config::{ModelConfig, ModelModalities, ProviderConfig};
#[allow(unused_imports)]
pub(crate) use crate::llm::{ContentBlock, LlmResponse, StreamEvent, TokenUsage};
#[allow(unused_imports)]
pub(crate) use crate::store::SessionStore;
#[allow(unused_imports)]
pub(crate) use fixtures::*;

mod fact_log;
mod fixtures;

mod agents_workers;
mod async_mcp;
mod attachment_cap;
mod background_settlement;
mod checkpoint_capability_recovery;
mod close;
mod cognitive;
mod compact;
mod conversation_busy;
mod durable_engineering;
mod dynamic_workflow;
#[cfg(feature = "evaluation")]
mod evaluation_evidence;
mod history_concurrency;
mod host_adapter;
mod long_horizon_capabilities;
mod memory;
mod options;
mod path_instructions_fact;
mod persistence;
mod queue;
mod runtime;
mod scheduler;
mod session_construction;
mod skill_collision;
mod stream;
mod stream_identity;
mod subagent;
mod timeouts;
mod worktree_settlement;
