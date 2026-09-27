#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
//! Session-support modules extracted from `a3s-code-shell`'s `session/` tree so they build in parallel and stop rebuilding on shell edits.
//! Shell re-exports them at their original paths.
#![deny(clippy::indexing_slicing)]
pub mod managed_mcp;
