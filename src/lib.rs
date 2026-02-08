//! A3S Code Agent Library
//!
//! Rust implementation of the coding agent that runs inside the guest VM.
//! Provides gRPC service for host-guest communication.
//!
//! ## Architecture
//!
//! ```text
//! Host (SDK) --gRPC-over-vsock--> Guest Agent
//!                                    |
//!                                    +-- Session Manager
//!                                    |      +-- Session 1 (with Queue + HITL)
//!                                    |      +-- Session 2 (with Queue + HITL)
//!                                    |      +-- ...
//!                                    |
//!                                    +-- Agent Loop
//!                                    |      +-- LLM Client
//!                                    |      +-- Tool Executor
//!                                    |      +-- HITL Confirmation
//!                                    |
//!                                    +-- Tools
//!                                           +-- bash
//!                                           +-- read/write/edit
//!                                           +-- grep/glob/ls
//! ```
//!
//! ## Human-in-the-Loop (HITL)
//!
//! The agent supports HITL confirmation for sensitive tool executions:
//! - Configurable per-session via `ConfirmationPolicy`
//! - YOLO mode for auto-approving specific lanes
//! - Timeout handling with reject/auto-approve options
//!
//! ## Lane-Based Queue
//!
//! Each session has its own command queue with priority lanes:
//! - Control (P0): pause, resume, cancel
//! - Query (P1): read, glob, ls, grep
//! - Execute (P2): bash, write, edit
//! - Generate (P3): LLM calls
//!
//! ## Permission System
//!
//! Declarative permission system similar to Claude Code:
//! - `allow` rules: auto-approve matching tool invocations
//! - `deny` rules: always block matching invocations
//! - `ask` rules: require user confirmation
//! - Evaluation order: Deny → Allow → Ask → Default
//!
//! ## Hooks System
//!
//! Extensible hook system for intercepting and customizing agent behavior:
//! - `PreToolUse`: Before tool execution (can block/modify)
//! - `PostToolUse`: After tool execution (fire-and-forget)
//! - `GenerateStart`: Before LLM generation
//! - `GenerateEnd`: After LLM generation
//! - `SessionStart`: When session is created
//! - `SessionEnd`: When session is destroyed

pub mod agent;
pub mod config;
pub mod context;
pub mod convert;
pub mod hitl;
pub mod hooks;
pub mod lane_integration;
pub mod llm;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod planning;
pub mod prompts;
pub mod queue;
pub mod reflection;
pub mod safeclaw;
pub mod service;
pub mod session;
pub mod session_lane_queue;
pub mod store;
pub mod subagent;
pub mod telemetry;
pub mod todo;
pub mod tools;

pub use config::{CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
