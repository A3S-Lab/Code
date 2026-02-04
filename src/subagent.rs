//! Subagent System
//!
//! Provides a system for delegating specialized tasks to focused child agents.
//! Each subagent runs in an isolated child session with restricted permissions.
//!
//! ## Architecture
//!
//! ```text
//! Parent Session
//!   └── Task Tool
//!         ├── AgentRegistry (lookup agent definitions)
//!         └── Child Session (isolated execution)
//!               ├── Restricted permissions
//!               ├── Optional model override
//!               └── Event forwarding to parent
//! ```
//!
//! ## Built-in Agents
//!
//! - `explore`: Fast codebase exploration (read-only)
//! - `general`: Multi-step task execution
//! - `plan`: Read-only planning mode
//! - `title`: Session title generation (hidden)
//! - `summary`: Session summarization (hidden)

use crate::permissions::PermissionPolicy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

/// Agent execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Primary agent (main conversation)
    #[default]
    Primary,
    /// Subagent (child session for delegated tasks)
    Subagent,
}

/// Model configuration for agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model identifier (e.g., "claude-3-5-sonnet-20241022")
    pub model: String,
    /// Optional provider override
    pub provider: Option<String>,
}

/// Agent definition
///
/// Defines the configuration and capabilities of an agent type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Agent identifier (e.g., "explore", "plan", "general")
    pub name: String,
    /// Description of what the agent does
    pub description: String,
    /// Agent mode: "subagent" or "primary"
    #[serde(default)]
    pub mode: AgentMode,
    /// Whether this is a built-in agent
    #[serde(default)]
    pub native: bool,
    /// Whether to hide from UI
    #[serde(default)]
    pub hidden: bool,
    /// Permission rules for this agent
    #[serde(default)]
    pub permissions: PermissionPolicy,
    /// Optional model override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelConfig>,
    /// System prompt for this agent
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Maximum execution steps (tool rounds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    /// Whether this agent can spawn subagents (default: false)
    #[serde(default)]
    pub can_spawn_subagents: bool,
}

impl AgentDefinition {
    /// Create a new agent definition
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            mode: AgentMode::Subagent,
            native: false,
            hidden: false,
            permissions: PermissionPolicy::default(),
            model: None,
            prompt: None,
            max_steps: None,
            can_spawn_subagents: false,
        }
    }

    /// Set agent mode
    pub fn with_mode(mut self, mode: AgentMode) -> Self {
        self.mode = mode;
        self
    }

    /// Mark as native (built-in)
    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    /// Mark as hidden from UI
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Set permission policy
    pub fn with_permissions(mut self, permissions: PermissionPolicy) -> Self {
        self.permissions = permissions;
        self
    }

    /// Set model override
    pub fn with_model(mut self, model: ModelConfig) -> Self {
        self.model = Some(model);
        self
    }

    /// Set system prompt
    pub fn with_prompt(mut self, prompt: &str) -> Self {
        self.prompt = Some(prompt.to_string());
        self
    }

    /// Set maximum execution steps
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = Some(max_steps);
        self
    }

    /// Allow spawning subagents
    pub fn allow_subagents(mut self) -> Self {
        self.can_spawn_subagents = true;
        self
    }
}

/// Agent registry for managing agent definitions
///
/// Thread-safe registry that stores agent definitions and provides
/// lookup functionality.
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentDefinition>>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    /// Create a new agent registry with built-in agents
    pub fn new() -> Self {
        let registry = Self {
            agents: RwLock::new(HashMap::new()),
        };

        // Register built-in agents
        for agent in builtin_agents() {
            registry.register(agent);
        }

        registry
    }

    /// Register an agent definition
    pub fn register(&self, agent: AgentDefinition) {
        let mut agents = self.agents.write().unwrap();
        tracing::debug!("Registering agent: {}", agent.name);
        agents.insert(agent.name.clone(), agent);
    }

    /// Unregister an agent by name
    ///
    /// Returns true if the agent was removed, false if not found.
    pub fn unregister(&self, name: &str) -> bool {
        let mut agents = self.agents.write().unwrap();
        agents.remove(name).is_some()
    }

    /// Get an agent definition by name
    pub fn get(&self, name: &str) -> Option<AgentDefinition> {
        let agents = self.agents.read().unwrap();
        agents.get(name).cloned()
    }

    /// List all registered agents
    pub fn list(&self) -> Vec<AgentDefinition> {
        let agents = self.agents.read().unwrap();
        agents.values().cloned().collect()
    }

    /// List visible agents (not hidden)
    pub fn list_visible(&self) -> Vec<AgentDefinition> {
        let agents = self.agents.read().unwrap();
        agents.values().filter(|a| !a.hidden).cloned().collect()
    }

    /// Check if an agent exists
    pub fn exists(&self, name: &str) -> bool {
        let agents = self.agents.read().unwrap();
        agents.contains_key(name)
    }

    /// Get the number of registered agents
    pub fn len(&self) -> usize {
        let agents = self.agents.read().unwrap();
        agents.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Create built-in agent definitions
pub fn builtin_agents() -> Vec<AgentDefinition> {
    vec![
        // Explore agent: Fast codebase exploration (read-only)
        AgentDefinition::new(
            "explore",
            "Fast codebase exploration agent. Use for searching files, reading code, \
             and understanding codebase structure. Read-only operations only.",
        )
        .native()
        .with_permissions(explore_permissions())
        .with_max_steps(20)
        .with_prompt(EXPLORE_PROMPT),

        // General agent: Multi-step task execution
        AgentDefinition::new(
            "general",
            "General-purpose agent for multi-step task execution. Can read, write, \
             and execute commands. Cannot spawn subagents.",
        )
        .native()
        .with_permissions(general_permissions())
        .with_max_steps(50),

        // Plan agent: Read-only planning mode
        AgentDefinition::new(
            "plan",
            "Planning agent for designing implementation approaches. Read-only access \
             to explore codebase and create plans.",
        )
        .native()
        .with_mode(AgentMode::Primary)
        .with_permissions(plan_permissions())
        .with_max_steps(30)
        .with_prompt(PLAN_PROMPT),

        // Title agent: Session title generation (hidden)
        AgentDefinition::new(
            "title",
            "Generate a concise title for the session based on conversation content.",
        )
        .native()
        .hidden()
        .with_mode(AgentMode::Primary)
        .with_permissions(PermissionPolicy::new())
        .with_max_steps(1)
        .with_prompt(TITLE_PROMPT),

        // Summary agent: Session summarization (hidden)
        AgentDefinition::new(
            "summary",
            "Summarize the session conversation for context compaction.",
        )
        .native()
        .hidden()
        .with_mode(AgentMode::Primary)
        .with_permissions(summary_permissions())
        .with_max_steps(5)
        .with_prompt(SUMMARY_PROMPT),
    ]
}

// ============================================================================
// Permission Policies for Built-in Agents
// ============================================================================

/// Permission policy for explore agent (read-only)
fn explore_permissions() -> PermissionPolicy {
    PermissionPolicy::new()
        .allow_all(&["read", "grep", "glob", "ls"])
        .deny_all(&["write", "edit", "task"])
        .allow("Bash(ls:*)")
        .allow("Bash(cat:*)")
        .allow("Bash(head:*)")
        .allow("Bash(tail:*)")
        .allow("Bash(find:*)")
        .allow("Bash(wc:*)")
        .deny("Bash(rm:*)")
        .deny("Bash(mv:*)")
        .deny("Bash(cp:*)")
}

/// Permission policy for general agent (full access except task)
fn general_permissions() -> PermissionPolicy {
    PermissionPolicy::new()
        .allow_all(&["read", "write", "edit", "grep", "glob", "ls", "bash"])
        .deny("task")
}

/// Permission policy for plan agent (read-only)
fn plan_permissions() -> PermissionPolicy {
    PermissionPolicy::new()
        .allow_all(&["read", "grep", "glob", "ls"])
        .deny_all(&["write", "edit", "bash", "task"])
}

/// Permission policy for summary agent (read-only)
fn summary_permissions() -> PermissionPolicy {
    PermissionPolicy::new()
        .allow("read")
        .deny_all(&["write", "edit", "bash", "grep", "glob", "ls", "task"])
}

// ============================================================================
// System Prompts for Built-in Agents
// ============================================================================

const EXPLORE_PROMPT: &str = r#"You are an exploration agent focused on understanding codebases.

Your task is to explore and understand the codebase structure, find relevant files,
and gather information. You have read-only access to the filesystem.

Guidelines:
- Use glob to find files by pattern
- Use grep to search for code patterns
- Use read to examine file contents
- Use ls to list directory contents
- Be thorough but efficient in your exploration
- Report your findings clearly and concisely

You cannot modify any files. Focus on gathering information and understanding."#;

const PLAN_PROMPT: &str = r#"You are a planning agent focused on designing implementation approaches.

Your task is to analyze requirements, explore the codebase, and create a detailed
implementation plan. You have read-only access to the filesystem.

Guidelines:
- Understand the existing codebase structure first
- Identify files that need to be modified
- Consider edge cases and potential issues
- Create a step-by-step implementation plan
- Be specific about what changes are needed

You cannot modify any files. Focus on creating a clear, actionable plan."#;

const TITLE_PROMPT: &str = r#"Generate a concise title (5-10 words) for this conversation.
The title should capture the main topic or task being discussed.
Return only the title, no explanation."#;

const SUMMARY_PROMPT: &str = r#"Summarize the key points of this conversation.
Focus on:
- Main topics discussed
- Decisions made
- Important information shared
- Outstanding questions or tasks

Keep the summary concise but comprehensive."#;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_definition_builder() {
        let agent = AgentDefinition::new("test", "Test agent")
            .native()
            .hidden()
            .with_max_steps(10);

        assert_eq!(agent.name, "test");
        assert_eq!(agent.description, "Test agent");
        assert!(agent.native);
        assert!(agent.hidden);
        assert_eq!(agent.max_steps, Some(10));
        assert!(!agent.can_spawn_subagents);
    }

    #[test]
    fn test_agent_registry_new() {
        let registry = AgentRegistry::new();

        // Should have built-in agents
        assert!(registry.exists("explore"));
        assert!(registry.exists("general"));
        assert!(registry.exists("plan"));
        assert!(registry.exists("title"));
        assert!(registry.exists("summary"));
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn test_agent_registry_get() {
        let registry = AgentRegistry::new();

        let explore = registry.get("explore").unwrap();
        assert_eq!(explore.name, "explore");
        assert!(explore.native);
        assert!(!explore.hidden);

        let title = registry.get("title").unwrap();
        assert!(title.hidden);

        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_agent_registry_register_unregister() {
        let registry = AgentRegistry::new();
        let initial_count = registry.len();

        // Register custom agent
        let custom = AgentDefinition::new("custom", "Custom agent");
        registry.register(custom);
        assert_eq!(registry.len(), initial_count + 1);
        assert!(registry.exists("custom"));

        // Unregister
        assert!(registry.unregister("custom"));
        assert_eq!(registry.len(), initial_count);
        assert!(!registry.exists("custom"));

        // Unregister non-existent
        assert!(!registry.unregister("nonexistent"));
    }

    #[test]
    fn test_agent_registry_list_visible() {
        let registry = AgentRegistry::new();

        let visible = registry.list_visible();
        let all = registry.list();

        // Hidden agents should not be in visible list
        assert!(visible.len() < all.len());
        assert!(visible.iter().all(|a| !a.hidden));
    }

    #[test]
    fn test_builtin_agents() {
        let agents = builtin_agents();

        // Check we have expected agents
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"explore"));
        assert!(names.contains(&"general"));
        assert!(names.contains(&"plan"));
        assert!(names.contains(&"title"));
        assert!(names.contains(&"summary"));

        // Check explore is read-only (has deny rules for write)
        let explore = agents.iter().find(|a| a.name == "explore").unwrap();
        assert!(!explore.permissions.deny.is_empty());

        // Check general cannot spawn subagents
        let general = agents.iter().find(|a| a.name == "general").unwrap();
        assert!(!general.can_spawn_subagents);
    }

    #[test]
    fn test_agent_mode_default() {
        let mode = AgentMode::default();
        assert_eq!(mode, AgentMode::Primary);
    }
}
