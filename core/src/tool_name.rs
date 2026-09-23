//! Canonical tool-name helpers shared by permission matching and the registry.

/// Strip OpenAI-style `functions.` prefixes so policy rules like `Read(*)`
/// and registry keys like `read` match model-emitted names such as
/// `functions.read` (including nested `batch` calls).
pub fn canonical_tool_name(name: &str) -> &str {
    const PREFIX: &str = "functions.";
    if name.len() > PREFIX.len() && name[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        &name[PREFIX.len()..]
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_tool_name;

    #[test]
    fn strips_functions_prefix_case_insensitively() {
        assert_eq!(canonical_tool_name("functions.read"), "read");
        assert_eq!(canonical_tool_name("Functions.Read"), "Read");
        assert_eq!(canonical_tool_name("FUNCTIONS.LS"), "LS");
        assert_eq!(canonical_tool_name("read"), "read");
        assert_eq!(
            canonical_tool_name("mcp__server__tool"),
            "mcp__server__tool"
        );
        assert_eq!(canonical_tool_name("functions."), "functions.");
    }
}
