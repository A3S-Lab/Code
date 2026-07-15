use serde::{Deserialize, Serialize};

/// A permission rule with pattern matching support
///
/// Format: `ToolName(pattern)` or `ToolName` (matches all)
///
/// Examples:
/// - `Bash(cargo:*)` - matches all cargo commands
/// - `Bash(npm run test:*)` - matches npm run test with any args
/// - `Read(src/**/*.rs)` - matches Rust files in src/
/// - `Grep(*)` - matches all grep invocations
/// - `mcp__pencil` - matches all pencil MCP tools
///
/// Deserialization supports both plain strings and `{rule: "..."}` objects:
/// ```yaml
/// allow:
///   - read                   # plain string
///   - rule: "Bash(cargo:*)"  # struct form
/// ```
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PermissionRule {
    /// The original rule string
    pub rule: String,
    /// Parsed tool name
    #[serde(skip)]
    pub(crate) tool_name: Option<String>,
    /// Parsed argument pattern (None means match all)
    #[serde(skip)]
    pub(crate) arg_pattern: Option<String>,
}

impl<'de> Deserialize<'de> for PermissionRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Helper enum to accept both `"read"` and `{rule: "read"}` in YAML/JSON.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RuleRepr {
            Plain(String),
            Struct { rule: String },
        }

        let rule_str = match RuleRepr::deserialize(deserializer)? {
            RuleRepr::Plain(s) => s,
            RuleRepr::Struct { rule } => rule,
        };
        // `new()` calls `parse_rule()` to populate tool_name and arg_pattern.
        Ok(PermissionRule::new(&rule_str))
    }
}

impl PermissionRule {
    /// Create a new permission rule from a pattern string
    pub fn new(rule: &str) -> Self {
        let (tool_name, arg_pattern) = Self::parse_rule(rule);
        Self {
            rule: rule.to_string(),
            tool_name,
            arg_pattern,
        }
    }

    /// Parse rule string into tool name and argument pattern
    fn parse_rule(rule: &str) -> (Option<String>, Option<String>) {
        // Handle format: ToolName(pattern) or ToolName
        if let Some(paren_start) = rule.find('(') {
            if rule.ends_with(')') {
                let tool_name = rule[..paren_start].to_string();
                let pattern = rule[paren_start + 1..rule.len() - 1].to_string();
                return (Some(tool_name), Some(pattern));
            }
        }
        // No parentheses - tool name only, matches all args
        (Some(rule.to_string()), None)
    }

    /// Check if this rule matches a tool invocation
    pub fn matches(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        if !self.matches_tool(tool_name) {
            return false;
        }

        // If no argument pattern, match all
        let pattern = match &self.arg_pattern {
            Some(p) => p,
            None => return true,
        };

        // Match against argument pattern
        self.matches_args(pattern, tool_name, args)
    }

    /// Whether this rule can match a tool, independent of invocation args.
    pub(super) fn matches_tool(&self, tool_name: &str) -> bool {
        self.tool_name
            .as_deref()
            .is_some_and(|rule_tool| self.matches_tool_name(rule_tool, tool_name))
    }

    /// Whether a matching tool name is denied for every possible argument.
    pub(super) fn matches_all_args(&self) -> bool {
        self.arg_pattern
            .as_deref()
            .is_none_or(|pattern| pattern == "*")
    }

    /// Check if tool names match (case-insensitive, wildcard-aware)
    fn matches_tool_name(&self, rule_tool: &str, actual_tool: &str) -> bool {
        // If the rule contains wildcards, use glob matching on the tool name directly.
        // e.g. "mcp__longvt__*" must use glob, not starts_with, because starts_with
        // treats '*' as a literal character and will never match.
        if rule_tool.contains('*') || rule_tool.contains('?') {
            return self.glob_match(rule_tool, actual_tool);
        }

        // Handle MCP tools: mcp__server matches mcp__server__tool
        if rule_tool.starts_with("mcp__") && actual_tool.starts_with("mcp__") {
            // mcp__pencil matches mcp__pencil__batch_design
            if actual_tool.starts_with(rule_tool) {
                return true;
            }
        }
        rule_tool.eq_ignore_ascii_case(actual_tool)
    }

    /// Match argument pattern against tool arguments
    fn matches_args(&self, pattern: &str, tool_name: &str, args: &serde_json::Value) -> bool {
        // Handle wildcard pattern "*" - matches everything
        if pattern == "*" {
            return true;
        }

        // Build argument string based on tool type
        let arg_string = self.build_arg_string(tool_name, args);

        // Perform glob-style matching
        self.glob_match(pattern, &arg_string)
    }

    /// Build a string representation of arguments for matching
    fn build_arg_string(&self, tool_name: &str, args: &serde_json::Value) -> String {
        match tool_name.to_lowercase().as_str() {
            "bash" => {
                // For Bash, use the command field
                args.get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
            "read" | "write" | "edit" => {
                // For file operations, use the file_path field
                args.get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
            "glob" => {
                // For glob, use the pattern field
                args.get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
            "grep" => {
                // For grep, combine pattern and path
                let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                format!("{} {}", pattern, path)
            }
            "ls" => {
                // For ls, use the path field
                args.get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
            _ => {
                // For other tools, serialize the entire args
                serde_json::to_string(args).unwrap_or_default()
            }
        }
    }

    /// Perform glob-style pattern matching
    ///
    /// Supports:
    /// - `*` matches any sequence of characters (except /)
    /// - `**` matches any sequence including /
    /// - `:*` at the end matches any suffix (including empty)
    fn glob_match(&self, pattern: &str, text: &str) -> bool {
        // Handle special `:*` suffix (matches any args after the prefix)
        if let Some(prefix) = pattern.strip_suffix(":*") {
            return text.starts_with(prefix);
        }

        // Normalize Windows backslashes to forward slashes for consistent matching
        let text = text.replace('\\', "/");

        // Convert glob pattern to regex pattern
        let regex_pattern = Self::glob_to_regex(pattern);
        if let Ok(re) = regex::Regex::new(&regex_pattern) {
            re.is_match(&text)
        } else {
            // Fallback to simple prefix match if regex fails
            text.starts_with(pattern)
        }
    }

    /// Convert glob pattern to regex pattern
    fn glob_to_regex(pattern: &str) -> String {
        let mut regex = String::from("^");
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];
            match c {
                '*' => {
                    // Check for ** (matches anything including /)
                    if i + 1 < chars.len() && chars[i + 1] == '*' {
                        // ** matches any path including /
                        // Skip optional following /
                        if i + 2 < chars.len() && chars[i + 2] == '/' {
                            regex.push_str(".*");
                            i += 3;
                        } else {
                            regex.push_str(".*");
                            i += 2;
                        }
                    } else {
                        // * matches anything except path separators
                        regex.push_str("[^/\\\\]*");
                        i += 1;
                    }
                }
                '?' => {
                    // ? matches any single character except path separators
                    regex.push_str("[^/\\\\]");
                    i += 1;
                }
                '.' | '+' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                    // Escape regex special characters
                    regex.push('\\');
                    regex.push(c);
                    i += 1;
                }
                _ => {
                    regex.push(c);
                    i += 1;
                }
            }
        }

        regex.push('$');
        regex
    }
}
