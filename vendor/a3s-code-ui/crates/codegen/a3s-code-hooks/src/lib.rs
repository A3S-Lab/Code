//! # a3s-code-hooks
//!
//! Runtime hook system for A3S Code: file-based discovery, command execution, and policy enforcement.
//!
//! ## Overview
//!
//! This crate provides a minimal hooks system for A3S Code. Hooks are discovered
//! from dedicated directories (`~/.a3s/hooks/` and `<git-worktree-root>/.a3s/hooks/`),
//! defined in JSON files (compatible settings format), and executed as child processes.
//!
//! ## Scope
//!
//! - Event types: `session_start`, `pre_tool_use`, `post_tool_use`, `user_prompt_submit`, `stop`/`subagent_stop`, `notification`, `session_end`
//! - Both command-backed and HTTP hooks
//! - `pre_tool_use` hooks can allow/ask/deny and rewrite tool input via `updatedInput`
//! - Prompt and stop gates can block
//! - Fail-open by default: a hook error never blocks
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::path::Path;
//! use a3s_code_hooks::discovery::load_hooks;
//! use a3s_code_hooks::event::HookEventName;
//!
//! let (registry, errors) = load_hooks(
//!     Some(Path::new("/home/user/.a3s/hooks")),
//!     Some(Path::new("/project/.a3s/hooks")),
//! );
//!
//! for err in &errors {
//!     eprintln!("hook load warning: {err}");
//! }
//!
//! let pre_hooks = registry.hooks_for(HookEventName::PreToolUse);
//! println!("loaded {} pre_tool_use hooks", pre_hooks.len());
//! ```

#![deny(clippy::indexing_slicing)]

pub mod config;
pub mod discovery;
pub mod dispatcher;
mod env_expand;
pub mod error;
pub mod event;
pub mod matcher;
pub mod result;
pub mod runner;
#[cfg(test)]
mod test_support;
pub mod trust;
