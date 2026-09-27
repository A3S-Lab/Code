//! Foundation modules shared by the a3s shell crate family.
//! Extracted from `a3s-code-shell` (which re-exports them at their original paths) so they build in parallel and stop rebuilding on shell edits.

#![deny(clippy::indexing_slicing)]

pub mod cpu_profile;
pub mod env;
pub mod util;
