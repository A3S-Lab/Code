//! Middleware system for A3S Code
//!
//! Provides an Express-like middleware pipeline for intercepting and
//! modifying agent execution flow.
//!
//! # Architecture
//!
//! The middleware system follows a layered architecture:
//!
//! - **Layer 1 (Rust Core)**: Lightweight Express-like middleware pipeline
//! - **Layer 2 (SDK Bindings)**: PyO3/napi-rs bridges for Python/TypeScript
//! - **Layer 3 (SDK Framework)**: DI + decorators + high-level abstractions
//!
//! # Example
//!
//! ```rust
//! use a3s_code_core::middleware::{
//!     MiddlewarePipeline, LoggingMiddleware, SecurityMiddleware
//! };
//! use std::sync::Arc;
//!
//! let mut pipeline = MiddlewarePipeline::new();
//! pipeline.use_middleware(Arc::new(LoggingMiddleware::new("debug")));
//! pipeline.use_middleware(Arc::new(SecurityMiddleware::new(security_provider)));
//! ```

mod context;
mod logging;
mod permission;
mod pipeline;
mod result;
mod security;
mod trait_def;

pub use context::{MiddlewareContext, ToolCallInfo};
pub use logging::LoggingMiddleware;
pub use permission::PermissionMiddleware;
pub use pipeline::MiddlewarePipeline;
pub use result::MiddlewareResult;
pub use security::SecurityMiddleware;
pub use trait_def::Middleware;
