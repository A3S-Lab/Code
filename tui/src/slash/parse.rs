//! Slash invocation parser vendored from the grok-build pager.

/// Parsed slash command invocation.
pub struct SlashInvocation<'a> {
    /// Command token (for example, "model" for "/model name").
    pub token: &'a str,
    /// Everything after the command token, trimmed on the left.
    pub args: &'a str,
}

/// Parse a line into a slash command invocation.
///
/// Returns `None` if the line doesn't start with `/` or has no command token.
pub fn parse_invocation(line: &str) -> Option<SlashInvocation<'_>> {
    let remainder = line.strip_prefix('/')?;
    if remainder.is_empty() {
        return None;
    }

    let mut command_end = remainder.len();
    for (idx, ch) in remainder.char_indices() {
        if ch.is_whitespace() {
            command_end = idx;
            break;
        }
    }
    let token = remainder.get(..command_end)?.trim();
    if token.is_empty() {
        return None;
    }
    let args = if command_end < remainder.len() {
        remainder.get(command_end..)?.trim_start()
    } else {
        ""
    };
    Some(SlashInvocation { token, args })
}
