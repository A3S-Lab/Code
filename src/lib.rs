//! A3S Code Agent Server
//!
//! gRPC server for the A3S Code agent. All business logic lives in
//! `a3s-code-core`; this crate provides the gRPC service layer,
//! proto conversions, and telemetry initialization.
//!
//! ## Architecture
//!
//! ```text
//! a3s-code (this crate — server)
//!   +-- service.rs      (gRPC service impl)
//!   +-- convert.rs      (proto <-> domain conversions)
//!   +-- telemetry_init   (OTel subscriber setup)
//!   +-- main.rs          (binary entry point)
//!   |
//!   +-- re-exports from a3s-code-core (all business logic)
//! ```

// Re-export all core library modules so that service.rs and convert.rs
// can continue using `use crate::module::...` unchanged.
pub use a3s_code_core::agent;
pub use a3s_code_core::agent_api;
pub use a3s_code_core::checkpoint;
pub use a3s_code_core::config;
pub use a3s_code_core::context;
pub use a3s_code_core::context_store;
pub use a3s_code_core::export;
pub use a3s_code_core::file_history;
pub use a3s_code_core::hitl;
pub use a3s_code_core::hooks;
pub use a3s_code_core::lane_integration;
pub use a3s_code_core::llm;
pub use a3s_code_core::lsp;
pub use a3s_code_core::mcp;
pub use a3s_code_core::memory;
pub use a3s_code_core::permissions;
pub use a3s_code_core::planning;
pub use a3s_code_core::project_memory;
pub use a3s_code_core::prompts;
pub use a3s_code_core::queue;
pub use a3s_code_core::reflection;
pub use a3s_code_core::retry;
pub use a3s_code_core::security;
pub use a3s_code_core::session;
pub use a3s_code_core::session_lane_queue;
pub use a3s_code_core::store;
pub use a3s_code_core::subagent;
pub use a3s_code_core::telemetry;
pub use a3s_code_core::todo;
pub use a3s_code_core::tools;

// Server-specific modules
pub mod convert;
pub mod rest;
pub mod service;
pub mod telemetry_init;

// Re-export key types from core for backward compatibility
pub use a3s_code_core::{CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
