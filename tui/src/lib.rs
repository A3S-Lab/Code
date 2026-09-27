//! Full-screen coding TUI for A3S Code.
//!
//! Scrollback layout, the prompt text area, and slash-command entry are
//! vendored from the a3s-build pager and renamed for A3S. Model selection
//! stays on ACL `default_model`. A coding turn is an a3s-code 9.0.0 fact-log
//! `send` via the ACP-shaped [`agent::CodeAgentAdapter`].

pub mod agent;
mod hubs;
mod markdown_plain;
mod model;
mod screen;
mod scrollback;
mod session;
mod slash;
mod transcript;

pub use agent::{AgentTurn, CodeAgentAdapter, PermissionMode};
pub use a3s_markdown_core as markdown;
pub use a3s_ratatui_inline as inline;
pub use markdown_plain::markdown_to_plain;
pub use model::{
    merge_launch_layers, resolve_acl_file, resolve_acl_model, LaunchLayers, MergedLaunch,
    ResolvedModel,
};
pub use screen::run_fullscreen;
pub use scrollback::{HorizontalLayout, LayoutConfig};
pub use session::{submit_configured_turn, ConfiguredTurn};
pub use slash::{
    find_command, matching_commands, parse_invocation, parse_slash, FuzzyMatcher, SlashCommand,
    SlashCommandGroup, SlashInvocation, SLASH_BROWSE_HIDDEN, SLASH_COMMANDS,
};
pub use transcript::Scrollback;

/// User-visible product name. The TUI does not present another vendor.
pub const PRODUCT_NAME: &str = "A3S";

#[cfg(test)]
mod tests;
