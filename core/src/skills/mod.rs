//! Skill System
//!
//! Provides a lightweight skill system compatible with Claude Code skill format.
//! Skills are defined in Markdown files with YAML frontmatter.
//!
//! ## Skill Format
//!
//! ```markdown
//! ---
//! name: my-skill
//! description: What the skill does
//! allowed-tools: "read(*), grep(*)"
//! kind: instruction  # or "persona" or "tool"
//! ---
//! # Skill Instructions
//!
//! You are a specialized assistant that...
//! ```
//!
//! ## Skill Kinds
//!
//! - `instruction` (default): Injected into system prompt when matched
//! - `persona`: Session-level system prompt (bound at session creation)
//! - `tool`: Tool-like skill with specialized functionality (treated like instruction)

mod builtin;
pub mod feedback;
mod manage;
pub mod preprocessor;
mod registry;
pub mod validator;

pub use builtin::builtin_skills;
pub use feedback::{DefaultSkillScorer, SkillFeedback, SkillOutcome, SkillScore, SkillScorer};
pub use manage::ManageSkillTool;
pub use registry::SkillRegistry;
pub use validator::{
    DefaultSkillValidator, SkillValidationError, SkillValidator, ValidationErrorKind,
};

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Skill kind classification
///
/// Determines how the skill is used:
/// - `Instruction`: Prompt/instruction content injected into system prompt
/// - `Persona`: Session-level system prompt (bound at session creation, not injected globally)
/// - `Tool`: Tool-like skill that provides specialized functionality (treated as instruction)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SkillKind {
    #[default]
    Instruction,
    Persona,
    Tool,
}

/// Tool permission pattern
///
/// Represents a tool permission in Claude Code format:
/// - `Bash(gh issue view:*)` -> tool: "Bash", pattern: "gh issue view:*"
/// - `read(*)` -> tool: "read", pattern: "*"
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolPermission {
    pub tool: String,
    pub pattern: String,
}

impl ToolPermission {
    /// Parse a tool permission from Claude Code format
    ///
    /// Examples:
    /// - "Bash(gh issue view:*)" -> ToolPermission { tool: "Bash", pattern: "gh issue view:*" }
    /// - "read(*)" -> ToolPermission { tool: "read", pattern: "*" }
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();

        // Find opening parenthesis
        let open = s.find('(')?;
        let close = s.rfind(')')?;

        if close <= open {
            return None;
        }

        let tool = s[..open].trim().to_string();
        let pattern = s[open + 1..close].trim().to_string();

        Some(ToolPermission { tool, pattern })
    }
}

/// Skill definition (Claude Code compatible)
///
/// Represents a skill loaded from a Markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Skill name (from frontmatter or filename)
    #[serde(default)]
    pub name: String,

    /// Skill description
    #[serde(default)]
    pub description: String,

    /// Allowed tools (Claude Code format: "Bash(pattern:*), read(*)")
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<String>,

    /// Whether to disable model invocation
    #[serde(default, rename = "disable-model-invocation")]
    pub disable_model_invocation: bool,

    /// Skill kind (instruction or persona)
    #[serde(default)]
    pub kind: SkillKind,

    /// Skill content (markdown instructions)
    #[serde(skip)]
    pub content: String,

    /// Optional tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Optional version
    #[serde(default)]
    pub version: Option<String>,
}

impl Skill {
    /// Parse a skill from markdown content
    ///
    /// Expected format:
    /// ```markdown
    /// ---
    /// name: skill-name
    /// description: What it does
    /// allowed-tools: "read(*), grep(*)"
    /// ---
    /// # Instructions
    /// ...
    /// ```
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

    /// Load a skill from a file
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())?;
        let mut skill =
            Self::parse(&content).ok_or_else(|| anyhow::anyhow!("Failed to parse skill file"))?;

        // Use filename as name if not specified
        if skill.name.is_empty() {
            if let Some(stem) = path.as_ref().file_stem() {
                skill.name = stem.to_string_lossy().to_string();
            }
        }

        Ok(skill)
    }

    /// Parse allowed tools into a set of tool permissions
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

    /// Check if a tool is allowed by this skill
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        let permissions = self.parse_allowed_tools();

        if permissions.is_empty() {
            // No restrictions = all tools allowed
            return true;
        }

        // Check if any permission matches
        permissions
            .iter()
            .any(|perm| perm.tool.eq_ignore_ascii_case(tool_name) && perm.pattern == "*")
    }

    /// Get the skill content formatted for injection into system prompt
    pub fn to_system_prompt(&self) -> String {
        format!(
            "# Skill: {}\n\n{}\n\n{}",
            self.name, self.description, self.content
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill() {
        let content = r#"---
name: test-skill
description: A test skill
allowed-tools: "read(*), grep(*)"
kind: instruction
---
# Instructions

You are a test assistant.
"#;

        let skill = Skill::parse(content).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.kind, SkillKind::Instruction);
        assert!(skill.content.contains("You are a test assistant"));
    }

    #[test]
    fn test_parse_tool_permission() {
        let perm = ToolPermission::parse("Bash(gh issue view:*)").unwrap();
        assert_eq!(perm.tool, "Bash");
        assert_eq!(perm.pattern, "gh issue view:*");

        let perm = ToolPermission::parse("read(*)").unwrap();
        assert_eq!(perm.tool, "read");
        assert_eq!(perm.pattern, "*");
    }

    #[test]
    fn test_parse_allowed_tools() {
        let skill = Skill {
            name: "test".to_string(),
            description: "test".to_string(),
            allowed_tools: Some("read(*), grep(*), Bash(gh:*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        };

        let permissions = skill.parse_allowed_tools();
        assert_eq!(permissions.len(), 3);
    }

    #[test]
    fn test_is_tool_allowed() {
        let skill = Skill {
            name: "test".to_string(),
            description: "test".to_string(),
            allowed_tools: Some("read(*), grep(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        };

        assert!(skill.is_tool_allowed("read"));
        assert!(skill.is_tool_allowed("grep"));
        assert!(!skill.is_tool_allowed("write"));
    }
}
