//! Slash-command entry vendored from the grok-build pager.
//!
//! A leading `/` opens the command menu. Enter runs the command.

mod matcher;
mod parse;

pub use matcher::FuzzyMatcher;
pub use parse::{parse_invocation, SlashInvocation};

/// Maximum number of visible rows in the dropdown (scroll beyond this).
pub const MAX_VISIBLE_SUGGESTIONS: usize = 8;

/// Commands the prompt offers when the line starts with `/`.
pub const SLASH_COMMANDS: &[&str] = &["help", "model", "clear", "exit", "quit"];

/// Command token after `/`, without arguments.
pub fn parse_slash(line: &str) -> Option<&str> {
    parse_invocation(line.trim()).map(|invocation| invocation.token)
}

/// Menu rows ranked by the pager's fuzzy matcher.
pub fn matching_commands(line: &str) -> Vec<&'static str> {
    let query = line
        .trim()
        .strip_prefix('/')
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("");
    let mut matcher = FuzzyMatcher::new();
    matcher
        .rank(SLASH_COMMANDS, query, MAX_VISIBLE_SUGGESTIONS, |name| name)
        .into_iter()
        .filter_map(|(index, _score)| SLASH_COMMANDS.get(index).copied())
        .collect()
}
