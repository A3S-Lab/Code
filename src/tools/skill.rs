//! Skill System
//!
//! Provides support for loading skills in markdown format.
//! Skills use a simple format focused on prompts/instructions
//! with optional tool permissions.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Skill kind classification
///
/// Determines how the skill is injected into the agent session:
/// - `Instruction`: Prompt/instruction content injected into system prompt
/// - `Tool`: Registers executable tools via the skill loader
/// - `Agent`: Agent definition (future: registered in AgentRegistry)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SkillKind {
    #[default]
    Instruction,
    Tool,
    Agent,
}

/// Skill definition
///
/// Represents a skill with:
/// - name: skill identifier
/// - description: what the skill does
/// - allowed_tools: tool permissions (e.g., "Bash(gh:*)")
/// - disable_model_invocation: whether to disable model calls
/// - content: the prompt/instruction content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Skill name (from frontmatter or filename)
    #[serde(default)]
    pub name: String,

    /// Skill description
    #[serde(default)]
    pub description: String,

    /// Allowed tools (Claude Code format: "Bash(pattern:*)")
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<String>,

    /// Whether to disable model invocation
    #[serde(default, rename = "disable-model-invocation")]
    pub disable_model_invocation: bool,

    /// Skill kind (instruction, tool, or agent)
    #[serde(default)]
    pub kind: SkillKind,

    /// Skill content (markdown instructions)
    #[serde(skip)]
    pub content: String,
}

impl Skill {
    /// Parse a skill from markdown content
    pub fn parse(content: &str) -> Option<Self> {
        // Parse frontmatter (YAML between --- markers)
        let parts: Vec<&str> = content.splitn(3, "---").collect();

        if parts.len() < 3 {
            return None;
        }

        let frontmatter = parts[1].trim();
        let body = parts[2].trim();

        // Parse YAML frontmatter
        let mut skill: Skill = serde_yaml::from_str(frontmatter).ok()?;
        skill.content = body.to_string();

        Some(skill)
    }

    /// Parse allowed tools into a set of tool patterns
    ///
    /// Claude Code format: "Bash(gh issue view:*), Bash(gh search:*)"
    /// Returns patterns like: ["Bash:gh issue view:*", "Bash:gh search:*"]
    pub fn parse_allowed_tools(&self) -> HashSet<ToolPermission> {
        let mut permissions = HashSet::new();

        let Some(allowed) = &self.allowed_tools else {
            return permissions;
        };

        // Parse comma-separated tool permissions
        for part in allowed.split(',') {
            let part = part.trim();
            if let Some(perm) = ToolPermission::parse(part) {
                permissions.insert(perm);
            }
        }

        permissions
    }

    /// Check if a tool call is allowed by this skill
    pub fn is_tool_allowed(&self, tool_name: &str, args: &str) -> bool {
        let permissions = self.parse_allowed_tools();

        // If no permissions specified, all tools are allowed
        if permissions.is_empty() {
            return true;
        }

        // Check if any permission matches
        permissions.iter().any(|p| p.matches(tool_name, args))
    }
}

/// Tool permission pattern
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolPermission {
    /// Tool name (e.g., "Bash")
    pub tool: String,
    /// Pattern to match (e.g., "gh issue view:*")
    pub pattern: String,
}

impl ToolPermission {
    /// Parse a tool permission from Claude Code format
    ///
    /// Format: "ToolName(pattern)" or "ToolName(pattern:*)"
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();

        // Find the opening parenthesis
        let paren_start = s.find('(')?;
        let paren_end = s.rfind(')')?;

        if paren_start >= paren_end {
            return None;
        }

        let tool = s[..paren_start].trim().to_string();
        let pattern = s[paren_start + 1..paren_end].trim().to_string();

        Some(Self { tool, pattern })
    }

    /// Check if this permission matches a tool call
    pub fn matches(&self, tool_name: &str, args: &str) -> bool {
        // Tool name must match
        if self.tool != tool_name {
            return false;
        }

        // Check pattern match
        self.pattern_matches(args)
    }

    /// Check if the pattern matches the given arguments
    fn pattern_matches(&self, args: &str) -> bool {
        let pattern = &self.pattern;

        // Handle wildcard patterns
        if pattern == "*" {
            return true;
        }

        // Handle prefix wildcard (e.g., "gh:*" matches "gh status")
        if let Some(prefix) = pattern.strip_suffix(":*") {
            return args.starts_with(prefix);
        }

        // Handle suffix wildcard (e.g., "*:view" matches "gh issue view")
        if let Some(suffix) = pattern.strip_prefix("*:") {
            return args.ends_with(suffix);
        }

        // Handle glob-style wildcards
        if pattern.contains('*') {
            return glob_match(pattern, args);
        }

        // Exact match
        pattern == args
    }
}

/// Simple glob matching for patterns with *
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.is_empty() {
        return true;
    }

    let mut pos = 0;

    // First part must match at start (if not empty)
    if !parts[0].is_empty() {
        if !text.starts_with(parts[0]) {
            return false;
        }
        pos = parts[0].len();
    }

    // Middle parts must be found in order
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        if let Some(found) = text[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }

    // Last part must match at end (if not empty)
    if let Some(last) = parts.last() {
        if !last.is_empty() && !text[pos..].ends_with(last) {
            return false;
        }
    }

    true
}

/// Built-in skills compiled into the binary
///
/// These skills are always available without loading from disk.
pub fn builtin_skills() -> Vec<Skill> {
    let mut skills = Vec::new();

    let find_skills_content = include_str!("../../skills/find-skills.md");
    if let Some(skill) = Skill::parse(find_skills_content) {
        skills.push(skill);
    }

    skills
}

/// Load skills from a directory
///
/// Scans for .md files and parses them as skills.
/// Returns skills that have valid frontmatter.
pub fn load_skills(dir: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::warn!(
            "Failed to read skills directory: {}",
            dir.display()
        );
        return skills;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip non-files
        if !path.is_file() {
            continue;
        }

        // Only process .md files
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        if ext != "md" {
            continue;
        }

        // Read and parse the skill file
        let Ok(content) = std::fs::read_to_string(&path) else {
            tracing::warn!("Failed to read skill file: {}", path.display());
            continue;
        };

        if let Some(mut skill) = Skill::parse(&content) {
            // Use filename as name if not specified
            if skill.name.is_empty() {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    skill.name = stem.to_string();
                }
            }
            tracing::debug!("Loaded Claude Code skill: {}", skill.name);
            skills.push(skill);
        }
    }

    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill() {
        let content = r#"---
name: test-skill
description: A test skill
allowed-tools: Bash(gh:*)
---
This is the skill content.
"#;

        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.allowed_tools, Some("Bash(gh:*)".to_string()));
        assert!(!skill.disable_model_invocation);
        assert_eq!(skill.content, "This is the skill content.");
    }

    #[test]
    fn test_parse_skill_with_disable_model() {
        let content = r#"---
name: restricted-skill
disable-model-invocation: true
---
Content here.
"#;

        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.name, "restricted-skill");
        assert!(skill.disable_model_invocation);
    }

    #[test]
    fn test_parse_tool_permission() {
        let perm = ToolPermission::parse("Bash(gh issue view:*)").unwrap();
        assert_eq!(perm.tool, "Bash");
        assert_eq!(perm.pattern, "gh issue view:*");
    }

    #[test]
    fn test_parse_tool_permission_simple() {
        let perm = ToolPermission::parse("Read(*)").unwrap();
        assert_eq!(perm.tool, "Read");
        assert_eq!(perm.pattern, "*");
    }

    #[test]
    fn test_tool_permission_matches_wildcard() {
        let perm = ToolPermission::parse("Bash(*)").unwrap();
        assert!(perm.matches("Bash", "any command"));
        assert!(perm.matches("Bash", ""));
        assert!(!perm.matches("Read", "file.txt"));
    }

    #[test]
    fn test_tool_permission_matches_prefix() {
        let perm = ToolPermission::parse("Bash(gh:*)").unwrap();
        assert!(perm.matches("Bash", "gh status"));
        assert!(perm.matches("Bash", "gh pr view"));
        assert!(!perm.matches("Bash", "git status"));
    }

    #[test]
    fn test_parse_allowed_tools() {
        let content = r#"---
name: multi-tool
allowed-tools: Bash(gh issue view:*), Bash(gh pr:*), Read(*)
---
"#;

        let skill = Skill::parse(content).unwrap();
        let permissions = skill.parse_allowed_tools();

        assert_eq!(permissions.len(), 3);
    }

    #[test]
    fn test_is_tool_allowed() {
        let content = r#"---
name: github-skill
allowed-tools: Bash(gh:*)
---
"#;

        let skill = Skill::parse(content).unwrap();

        assert!(skill.is_tool_allowed("Bash", "gh status"));
        assert!(skill.is_tool_allowed("Bash", "gh pr view 123"));
        assert!(!skill.is_tool_allowed("Bash", "rm -rf /"));
        assert!(!skill.is_tool_allowed("Read", "file.txt"));
    }

    #[test]
    fn test_is_tool_allowed_no_restrictions() {
        let content = r#"---
name: open-skill
---
"#;

        let skill = Skill::parse(content).unwrap();

        // No restrictions means all tools allowed
        assert!(skill.is_tool_allowed("Bash", "any command"));
        assert!(skill.is_tool_allowed("Read", "any file"));
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("gh*", "gh status"));
        assert!(glob_match("*view", "gh pr view"));
        assert!(glob_match("gh*view", "gh pr view"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("gh*", "git status"));
    }

    #[test]
    fn test_load_skills() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Create a Claude Code skill file
        std::fs::write(
            temp_dir.path().join("github.md"),
            r#"---
name: github-commands
description: GitHub CLI commands
allowed-tools: Bash(gh:*)
---
Use gh CLI for GitHub operations.
"#,
        )
        .unwrap();

        // Create another skill
        std::fs::write(
            temp_dir.path().join("code-review.md"),
            r#"---
name: code-review
description: Code review skill
allowed-tools: Bash(gh pr:*), Read(*)
disable-model-invocation: false
---
Review pull requests.
"#,
        )
        .unwrap();

        let skills = load_skills(temp_dir.path());
        assert_eq!(skills.len(), 2);

        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"github-commands"));
        assert!(names.contains(&"code-review"));
    }

    #[test]
    fn test_parse_skill_minimal() {
        let content = r#"---
name: minimal
---
Content only.
"#;
        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.name, "minimal");
        assert_eq!(skill.description, "");
        assert!(skill.allowed_tools.is_none());
        assert!(!skill.disable_model_invocation);
    }

    #[test]
    fn test_parse_skill_invalid_frontmatter() {
        let content = r#"---
invalid yaml: [
---
Content
"#;
        let skill = Skill::parse(content);
        assert!(skill.is_none());
    }

    #[test]
    fn test_parse_skill_no_frontmatter() {
        let content = "Just content without frontmatter";
        let skill = Skill::parse(content);
        assert!(skill.is_none());
    }

    #[test]
    fn test_parse_skill_single_separator() {
        let content = r#"---
name: test
"#;
        let skill = Skill::parse(content);
        assert!(skill.is_none());
    }

    #[test]
    fn test_tool_permission_parse_invalid() {
        assert!(ToolPermission::parse("NoParenthesis").is_none());
        assert!(ToolPermission::parse("Missing(").is_none());
        assert!(ToolPermission::parse("Reversed)pattern(").is_none());
        assert!(ToolPermission::parse("").is_none());
    }

    #[test]
    fn test_tool_permission_matches_exact() {
        let perm = ToolPermission::parse("Bash(gh status)").unwrap();
        assert!(perm.matches("Bash", "gh status"));
        assert!(!perm.matches("Bash", "gh pr"));
        assert!(!perm.matches("Read", "gh status"));
    }

    #[test]
    fn test_tool_permission_matches_suffix_wildcard() {
        let perm = ToolPermission::parse("Bash(*:view)").unwrap();
        assert!(perm.matches("Bash", "gh issue view"));
        assert!(perm.matches("Bash", "gh pr view"));
        assert!(!perm.matches("Bash", "gh issue list"));
    }

    #[test]
    fn test_tool_permission_matches_middle_wildcard() {
        let perm = ToolPermission::parse("Bash(gh*view)").unwrap();
        assert!(perm.matches("Bash", "gh issue view"));
        assert!(perm.matches("Bash", "gh pr view"));
        assert!(!perm.matches("Bash", "gh status"));
    }

    #[test]
    fn test_glob_match_only_wildcard() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn test_glob_match_multiple_wildcards() {
        assert!(glob_match("*test*file*", "my test data file here"));
        assert!(!glob_match("*test*file*", "my data here"));
    }

    #[test]
    fn test_glob_match_start_wildcard() {
        assert!(glob_match("*end", "start middle end"));
        assert!(!glob_match("*end", "start middle"));
    }

    #[test]
    fn test_glob_match_end_wildcard() {
        assert!(glob_match("start*", "start middle end"));
        assert!(!glob_match("start*", "middle end"));
    }

    #[test]
    fn test_parse_allowed_tools_empty() {
        let content = r#"---
name: test
allowed-tools: ""
---
"#;
        let skill = Skill::parse(content).unwrap();
        let permissions = skill.parse_allowed_tools();
        assert_eq!(permissions.len(), 0);
    }

    #[test]
    fn test_parse_allowed_tools_whitespace() {
        let content = r#"---
name: test
allowed-tools: "  Bash(gh:*)  ,  Read(*)  "
---
"#;
        let skill = Skill::parse(content).unwrap();
        let permissions = skill.parse_allowed_tools();
        assert_eq!(permissions.len(), 2);
    }

    #[test]
    fn test_is_tool_allowed_multiple_patterns() {
        let content = r#"---
name: test
allowed-tools: Bash(gh:*), Bash(git:*), Read(*)
---
"#;
        let skill = Skill::parse(content).unwrap();
        assert!(skill.is_tool_allowed("Bash", "gh status"));
        assert!(skill.is_tool_allowed("Bash", "git log"));
        assert!(skill.is_tool_allowed("Read", "file.txt"));
        assert!(!skill.is_tool_allowed("Write", "file.txt"));
    }

    #[test]
    fn test_tool_permission_equality() {
        let perm1 = ToolPermission {
            tool: "Bash".to_string(),
            pattern: "gh:*".to_string(),
        };
        let perm2 = ToolPermission {
            tool: "Bash".to_string(),
            pattern: "gh:*".to_string(),
        };
        let perm3 = ToolPermission {
            tool: "Read".to_string(),
            pattern: "*".to_string(),
        };
        assert_eq!(perm1, perm2);
        assert_ne!(perm1, perm3);
    }

    #[test]
    fn test_tool_permission_clone() {
        let perm = ToolPermission {
            tool: "Bash".to_string(),
            pattern: "test:*".to_string(),
        };
        let cloned = perm.clone();
        assert_eq!(perm, cloned);
    }

    #[test]
    fn test_tool_permission_debug() {
        let perm = ToolPermission {
            tool: "Bash".to_string(),
            pattern: "gh:*".to_string(),
        };
        let debug_str = format!("{:?}", perm);
        assert!(debug_str.contains("Bash"));
        assert!(debug_str.contains("gh:*"));
    }

    #[test]
    fn test_skill_clone() {
        let skill = Skill {
            name: "test".to_string(),
            description: "desc".to_string(),
            allowed_tools: Some("Bash(*)".to_string()),
            disable_model_invocation: true,
            kind: SkillKind::Instruction,
            content: "content".to_string(),
        };
        let cloned = skill.clone();
        assert_eq!(skill.name, cloned.name);
        assert_eq!(skill.description, cloned.description);
        assert_eq!(
            skill.disable_model_invocation,
            cloned.disable_model_invocation
        );
    }

    #[test]
    fn test_skill_debug() {
        let skill = Skill {
            name: "test".to_string(),
            description: "desc".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "content".to_string(),
        };
        let debug_str = format!("{:?}", skill);
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_load_skills_nonexistent_dir() {
        let skills = load_skills(std::path::Path::new("/nonexistent/path"));
        assert_eq!(skills.len(), 0);
    }

    #[test]
    fn test_load_skills_skip_non_md() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("skill.txt"),
            r#"---
name: test
---
"#,
        )
        .unwrap();
        let skills = load_skills(temp_dir.path());
        assert_eq!(skills.len(), 0);
    }

    #[test]
    fn test_load_skills_use_filename() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("my-skill.md"),
            r#"---
description: Test skill
---
Content
"#,
        )
        .unwrap();
        let skills = load_skills(temp_dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
    }

    #[test]
    fn test_load_skills_skip_subdirs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let subdir = temp_dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(
            subdir.join("skill.md"),
            r#"---
name: test
---
"#,
        )
        .unwrap();
        let skills = load_skills(temp_dir.path());
        assert_eq!(skills.len(), 0);
    }

    #[test]
    fn test_parse_allowed_tools_invalid_format() {
        let content = r#"---
name: test
allowed-tools: InvalidFormat, AlsoInvalid
---
"#;
        let skill = Skill::parse(content).unwrap();
        let permissions = skill.parse_allowed_tools();
        assert_eq!(permissions.len(), 0);
    }

    // ===================
    // Built-in Skills Tests
    // ===================

    #[test]
    fn test_builtin_skills() {
        let skills = builtin_skills();
        assert!(!skills.is_empty(), "Should have at least one built-in Claude Code skill");

        // Verify find-skills is present
        let find_skills = skills.iter().find(|s| s.name == "find-skills");
        assert!(find_skills.is_some(), "find-skills skill should be present");

        let skill = find_skills.unwrap();
        assert!(!skill.description.is_empty(), "find-skills should have a description");
        assert!(!skill.content.is_empty(), "find-skills should have content");
    }

    #[test]
    fn test_builtin_find_skills_content() {
        let skills = builtin_skills();
        let skill = skills.iter().find(|s| s.name == "find-skills").unwrap();

        // Verify key content sections exist
        assert!(skill.content.contains("search_skills"), "Should reference search_skills tool");
        assert!(skill.content.contains("install_skill"), "Should reference install_skill tool");
        assert!(skill.content.contains("skills.sh"), "Should reference skills.sh");
    }

    // ===================
    // SkillKind Tests
    // ===================

    #[test]
    fn test_skill_kind_default_is_instruction() {
        let kind = SkillKind::default();
        assert_eq!(kind, SkillKind::Instruction);
    }

    #[test]
    fn test_parse_skill_kind_instruction() {
        let content = r#"---
name: guide
kind: instruction
---
Some instructions.
"#;
        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.kind, SkillKind::Instruction);
    }

    #[test]
    fn test_parse_skill_kind_tool() {
        let content = r#"---
name: my-tool
kind: tool
---
Tool content.
"#;
        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.kind, SkillKind::Tool);
    }

    #[test]
    fn test_parse_skill_kind_agent() {
        let content = r#"---
name: my-agent
kind: agent
---
Agent content.
"#;
        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.kind, SkillKind::Agent);
    }

    #[test]
    fn test_parse_skill_kind_missing_defaults_to_instruction() {
        let content = r#"---
name: old-skill
description: No kind field
---
Content here.
"#;
        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.kind, SkillKind::Instruction);
    }

    #[test]
    fn test_skill_kind_serialize() {
        assert_eq!(
            serde_json::to_string(&SkillKind::Instruction).unwrap(),
            "\"instruction\""
        );
        assert_eq!(
            serde_json::to_string(&SkillKind::Tool).unwrap(),
            "\"tool\""
        );
        assert_eq!(
            serde_json::to_string(&SkillKind::Agent).unwrap(),
            "\"agent\""
        );
    }

    #[test]
    fn test_skill_kind_clone_copy() {
        let kind = SkillKind::Tool;
        let cloned = kind.clone();
        let copied = kind;
        assert_eq!(kind, cloned);
        assert_eq!(kind, copied);
    }
}
