//! Full-screen coding TUI for A3S Code.
//!
//! Scrollback layout, the prompt text area, and slash-command entry are
//! vendored from the grok-build pager and renamed for A3S. Model selection
//! stays on ACL `default_model`. A coding turn is an a3s-code 9.0.0 fact-log
//! `send`.

mod model;
mod screen;
mod scrollback;
mod session;
mod slash;
mod transcript;

pub use model::{resolve_acl_file, resolve_acl_model, ResolvedModel};
pub use screen::run_fullscreen;
pub use scrollback::{HorizontalLayout, LayoutConfig};
pub use session::submit_configured_turn;
pub use slash::{parse_invocation, parse_slash, FuzzyMatcher, SlashInvocation, SLASH_COMMANDS};
pub use transcript::Scrollback;

/// User-visible product name. The TUI does not present another vendor.
pub const PRODUCT_NAME: &str = "A3S";

#[cfg(test)]
mod tests;
