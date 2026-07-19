//! Delegated Agent System
//!
//! Provides a system for delegating specialized tasks to focused child agents.
//! Each delegated child run uses an isolated child session with restricted permissions.
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
//! - `deep-research`: Parent-policy evidence collection (hidden)
//! - `plan`: Read-only planning mode
//! - `verification`: Adversarial verification specialist
//! - `review`: Code review specialist
//!
//! ## Loading Agents from Files
//!
//! Agents can be loaded from YAML or Markdown files:
//!
//! ### YAML Format
//! ```yaml
//! name: my-agent
//! description: Custom agent for specific tasks
//! hidden: false
//! max_steps: 30
//! permissions:
//!   allow:
//!     - read
//!     - grep
//!   deny:
//!     - write
//! prompt: |
//!   You are a specialized agent...
//! ```
//!
//! ### Markdown Format
//! ```markdown
//! ---
//! name: my-agent
//! description: Custom agent
//! max_steps: 30
//! ---
//! # System Prompt
//! You are a specialized agent...
//! ```

use crate::config::CodeConfig;
use crate::permissions::{PermissionChecker, PermissionDecision, PermissionPolicy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use crate::error::{read_or_recover, write_or_recover};

/// How a child run resolves tools that require confirmation (PermissionDecision::Ask).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationInheritance {
    /// Auto-approve all Ask decisions. Safe when the agent has explicit allow-list
    /// permissions that already define the access boundary.
    #[default]
    AutoApprove,
    /// Deny all Ask decisions (strict mode — only explicitly allowed tools run).
    DenyOnAsk,
    /// Inherit the parent session's confirmation manager.
    InheritParent,
}

/// Model configuration for agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model identifier (e.g., "claude-3-5-sonnet-20241022")
    pub model: String,
    /// Optional provider override
    pub provider: Option<String>,
}

impl ModelConfig {
    /// Create a model override that inherits the default provider.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            provider: None,
        }
    }

    /// Create a model override from provider and model parts.
    pub fn with_provider(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            provider: Some(provider.into()),
        }
    }

    /// Parse a conventional `provider/model` reference.
    ///
    /// If no slash is present, the provider stays unset and the session binding
    /// falls back to the host agent's default provider behavior.
    pub fn from_model_ref(model_ref: impl AsRef<str>) -> Self {
        let model_ref = model_ref.as_ref();
        if let Some((provider, model)) = model_ref.split_once('/') {
            Self::with_provider(provider, model)
        } else {
            Self::new(model_ref)
        }
    }

    /// Return the model as `provider/model` when a provider is set.
    pub fn model_ref(&self) -> String {
        match &self.provider {
            Some(provider) => format!("{}/{}", provider, self.model),
            None => self.model.clone(),
        }
    }
}

/// Cattle-style worker agent role.
///
/// A worker role is a reproducible preset for disposable, task-scoped agents.
/// Use [`WorkerAgentSpec`] to create many consistent workers instead of hand-tuning
/// unique "pet" agents one by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerAgentKind {
    /// Read-only exploration: fast code search and inspection.
    #[serde(alias = "readonly", alias = "read-only", alias = "explore")]
    ReadOnly,
    /// Read-only planning: design work without modifying the workspace.
    #[serde(alias = "plan")]
    Planner,
    /// Implementation work: read, edit, write, and run commands; no recursive task spawning.
    #[serde(alias = "implementation", alias = "general")]
    Implementer,
    /// Verification work: run checks and inspect failures without editing files.
    #[serde(alias = "verification", alias = "verify")]
    Verifier,
    /// Review work: inspect changes and report findings.
    #[serde(alias = "review", alias = "code-review")]
    Reviewer,
    /// Strict custom worker: asks for any unspecified tool until permissions are supplied.
    Custom,
}

/// Reproducible recipe for a disposable worker/subagent.
///
/// This is the public "cattle mode" API: callers define a small, serializable
/// worker spec and register/spawn it repeatedly. The spec compiles to an
/// [`AgentDefinition`] consumed by the existing delegation/runtime pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAgentSpec {
    /// Stable worker name used in task delegation (e.g., "frontend-fixer").
    pub name: String,
    /// Human-readable purpose shown to users and model selectors.
    pub description: String,
    /// Preset permission/prompt/step profile.
    pub kind: WorkerAgentKind,
    /// Hide from UI lists while still allowing explicit delegation.
    #[serde(default)]
    pub hidden: bool,
    /// Optional permission policy override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermissionPolicy>,
    /// Optional model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelConfig>,
    /// Optional worker-specific prompt appended to the core agentic prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Maximum execution steps/tool rounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    /// How child runs resolve Ask decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_inheritance: Option<ConfirmationInheritance>,
}

impl WorkerAgentKind {
    /// Stable snake_case identifier for this role.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Planner => "planner",
            Self::Implementer => "implementer",
            Self::Verifier => "verifier",
            Self::Reviewer => "reviewer",
            Self::Custom => "custom",
        }
    }

    fn default_permissions(self) -> PermissionPolicy {
        match self {
            Self::ReadOnly => explore_permissions(),
            Self::Planner => plan_permissions(),
            Self::Implementer => general_permissions(),
            Self::Verifier => verification_permissions(),
            Self::Reviewer => review_permissions(),
            Self::Custom => PermissionPolicy::strict(),
        }
    }

    fn default_prompt(self) -> Option<&'static str> {
        match self {
            Self::ReadOnly => Some(EXPLORE_PROMPT),
            Self::Planner => Some(PLAN_PROMPT),
            Self::Verifier => Some(VERIFICATION_PROMPT),
            Self::Reviewer => Some(REVIEW_PROMPT),
            Self::Implementer | Self::Custom => None,
        }
    }

    fn default_max_steps(self) -> usize {
        match self {
            Self::ReadOnly => 20,
            Self::Planner => 30,
            Self::Implementer => 50,
            Self::Verifier => 30,
            Self::Reviewer => 25,
            Self::Custom => 30,
        }
    }
}

impl std::fmt::Display for WorkerAgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for WorkerAgentKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read_only" | "readonly" | "read-only" | "explore" | "scanner" => Ok(Self::ReadOnly),
            "planner" | "plan" => Ok(Self::Planner),
            "implementer" | "implementation" | "general" | "executor" => Ok(Self::Implementer),
            "verifier" | "verification" | "verify" | "tester" => Ok(Self::Verifier),
            "reviewer" | "review" | "code-review" | "code_reviewer" => Ok(Self::Reviewer),
            "custom" => Ok(Self::Custom),
            other => Err(anyhow::anyhow!("unknown worker agent kind '{}'", other)),
        }
    }
}

/// Backward-friendly alias for callers that name this pattern cattle mode.
pub type CattleAgentKind = WorkerAgentKind;
/// Backward-friendly alias for callers that name this pattern cattle mode.
pub type CattleAgentSpec = WorkerAgentSpec;

impl WorkerAgentSpec {
    /// Create a worker spec from an explicit preset.
    pub fn new(
        kind: WorkerAgentKind,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind,
            hidden: false,
            permissions: None,
            model: None,
            prompt: None,
            max_steps: None,
            confirmation_inheritance: None,
        }
    }

    /// Read-only exploration worker.
    pub fn read_only(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(WorkerAgentKind::ReadOnly, name, description)
    }

    /// Read-only planning worker.
    pub fn planner(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(WorkerAgentKind::Planner, name, description)
    }

    /// Implementation worker with read/write/bash capability and no recursive task spawning.
    pub fn implementer(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(WorkerAgentKind::Implementer, name, description)
    }

    /// Verification worker for tests/checks/reproductions without edits.
    pub fn verifier(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(WorkerAgentKind::Verifier, name, description)
    }

    /// Review worker for correctness/regression/security findings.
    pub fn reviewer(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(WorkerAgentKind::Reviewer, name, description)
    }

    /// Strict custom worker. Provide permissions explicitly for non-HITL execution.
    pub fn custom(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(WorkerAgentKind::Custom, name, description)
    }

    /// Hide or show this worker in UI lists.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Override the preset permission policy.
    pub fn with_permissions(mut self, permissions: PermissionPolicy) -> Self {
        self.permissions = Some(permissions);
        self
    }

    /// Override the preset model.
    pub fn with_model(mut self, model: ModelConfig) -> Self {
        self.model = Some(model);
        self
    }

    /// Override the preset model using `provider/model` or a model id.
    pub fn with_model_ref(mut self, model_ref: impl AsRef<str>) -> Self {
        self.model = Some(ModelConfig::from_model_ref(model_ref));
        self
    }

    /// Override the preset model using provider and model separately.
    pub fn with_provider_model(
        mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.model = Some(ModelConfig::with_provider(provider, model));
        self
    }

    /// Override the preset prompt.
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    /// Override the preset step budget.
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = Some(max_steps);
        self
    }

    /// Set confirmation inheritance policy for child runs.
    pub fn with_confirmation(mut self, inheritance: ConfirmationInheritance) -> Self {
        self.confirmation_inheritance = Some(inheritance);
        self
    }

    /// Compile this worker recipe into a runtime agent definition.
    pub fn into_agent_definition(self) -> AgentDefinition {
        let mut agent = AgentDefinition::new(&self.name, &self.description)
            .with_permissions(
                self.permissions
                    .unwrap_or_else(|| self.kind.default_permissions()),
            )
            .with_max_steps(
                self.max_steps
                    .unwrap_or_else(|| self.kind.default_max_steps()),
            );

        if self.hidden {
            agent = agent.hidden();
        }
        if let Some(model) = self.model {
            agent = agent.with_model(model);
        }
        if let Some(prompt) = self
            .prompt
            .or_else(|| self.kind.default_prompt().map(str::to_string))
        {
            agent = agent.with_prompt(&prompt);
        }
        if let Some(ci) = self.confirmation_inheritance {
            agent = agent.with_confirmation(ci);
        }
        agent
    }
}

impl From<WorkerAgentSpec> for AgentDefinition {
    fn from(spec: WorkerAgentSpec) -> Self {
        spec.into_agent_definition()
    }
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
    /// Whether this role is a pure model decision with no visible or executable tools.
    #[serde(default)]
    pub tool_free: bool,
    /// How child runs resolve Ask decisions. Default: AutoApprove when
    /// the agent has explicit allow rules, DenyOnAsk otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_inheritance: Option<ConfirmationInheritance>,
}

impl AgentDefinition {
    /// Create a new agent definition
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            native: false,
            hidden: false,
            permissions: PermissionPolicy::default(),
            model: None,
            prompt: None,
            max_steps: None,
            tool_free: false,
            confirmation_inheritance: None,
        }
    }

    /// Create an agent definition from a disposable worker recipe.
    pub fn worker(spec: WorkerAgentSpec) -> Self {
        spec.into_agent_definition()
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

    /// Make this role a pure LLM step. Tool definitions are removed before the
    /// child turn, and parent tool permissions are not inherited into it.
    pub fn tool_free(mut self) -> Self {
        self.tool_free = true;
        self
    }

    /// Whether this definition has non-empty permission rules.
    pub fn has_defined_permissions(&self) -> bool {
        !self.permissions.allow.is_empty() || !self.permissions.deny.is_empty()
    }

    /// Apply this definition's declared configuration to a mutable AgentConfig.
    ///
    /// Follows the "host overrides win" principle: only fills fields that are
    /// currently at their default/None state. Callers who want to force values
    /// should set them *after* calling `apply_to`.
    pub(crate) fn apply_to(&self, config: &mut crate::agent::AgentConfig) {
        use std::sync::Arc;

        if self.tool_free {
            config.tools.clear();
            let policy = PermissionPolicy::strict();
            config.permission_checker = Some(Arc::new(policy.clone()));
            config.permission_policy = Some(policy);
        }

        if !self.tool_free && config.permission_checker.is_none() && self.has_defined_permissions()
        {
            config.permission_checker =
                Some(Arc::new(self.permissions.clone()) as Arc<dyn PermissionChecker>);
            config.permission_policy = Some(self.permissions.clone());
        }

        if let Some(ref prompt) = self.prompt {
            if config.prompt_slots.extra.is_none() {
                config.prompt_slots.extra = Some(prompt.clone());
            }
        }

        if let Some(max_steps) = self.max_steps {
            if config.max_tool_rounds == crate::agent::MAX_TOOL_ROUNDS {
                config.max_tool_rounds = max_steps;
            }
        }

        // Confirmation inheritance: record how child-local Ask decisions are
        // resolved before the parent boundary is composed into this run.
        if config.confirmation_inheritance.is_none() {
            config.confirmation_inheritance =
                Some(self.confirmation_inheritance.clone().unwrap_or_else(|| {
                    if self.has_defined_permissions() {
                        ConfirmationInheritance::AutoApprove
                    } else {
                        ConfirmationInheritance::DenyOnAsk
                    }
                }));
        }
        if config.confirmation_manager.is_none() {
            match config.confirmation_inheritance.as_ref() {
                Some(ConfirmationInheritance::AutoApprove) => {
                    config.confirmation_manager =
                        Some(Arc::new(crate::hitl::AutoApproveConfirmation));
                }
                Some(ConfirmationInheritance::DenyOnAsk) | None => {
                    /* leave None — safety_gate denies */
                }
                Some(ConfirmationInheritance::InheritParent) => {
                    /* caller passes parent's manager */
                }
            }
        }
    }

    /// Set confirmation inheritance policy for child runs.
    pub fn with_confirmation(mut self, inheritance: ConfirmationInheritance) -> Self {
        self.confirmation_inheritance = Some(inheritance);
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

fn canonical_agent_name(name: &str) -> &str {
    match name.trim() {
        "general-purpose" | "general_purpose" | "generalpurpose" => "general",
        "deep_research" | "deepresearch" => "deep-research",
        "verify" | "verifier" => "verification",
        "code-review" | "code_reviewer" | "reviewer" => "review",
        other => other,
    }
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

    /// Create a new agent registry with configuration
    ///
    /// Loads built-in agents first, then loads agents from configured directories.
    pub fn with_config(config: &CodeConfig) -> Self {
        let registry = Self::new();

        // Load agents from configured directories
        for dir in &config.agent_dirs {
            let agents = load_agents_from_dir(dir);
            for agent in agents {
                tracing::info!("Loaded agent '{}' from {}", agent.name, dir.display());
                registry.register(agent);
            }
        }

        registry
    }

    /// Register an agent definition
    pub fn register(&self, agent: AgentDefinition) {
        let mut agents = write_or_recover(&self.agents);
        tracing::debug!("Registering agent: {}", agent.name);
        agents.insert(agent.name.clone(), agent);
    }

    /// Register a disposable worker agent from a reproducible spec.
    ///
    /// Returns the compiled [`AgentDefinition`] so callers can inspect or pass it
    /// directly to `session_for_agent`.
    pub fn register_worker(&self, spec: WorkerAgentSpec) -> AgentDefinition {
        let agent = spec.into_agent_definition();
        self.register(agent.clone());
        agent
    }

    /// Register multiple disposable worker agents and return their definitions.
    pub fn register_workers<I>(&self, specs: I) -> Vec<AgentDefinition>
    where
        I: IntoIterator<Item = WorkerAgentSpec>,
    {
        specs
            .into_iter()
            .map(|spec| self.register_worker(spec))
            .collect()
    }

    /// Unregister an agent by name
    ///
    /// Returns true if the agent was removed, false if not found.
    pub fn unregister(&self, name: &str) -> bool {
        let mut agents = write_or_recover(&self.agents);
        agents.remove(name).is_some()
    }

    /// Get an agent definition by name
    pub fn get(&self, name: &str) -> Option<AgentDefinition> {
        let agents = read_or_recover(&self.agents);
        agents
            .get(name)
            .or_else(|| agents.get(canonical_agent_name(name)))
            .cloned()
    }

    /// List all registered agents
    pub fn list(&self) -> Vec<AgentDefinition> {
        let agents = read_or_recover(&self.agents);
        agents.values().cloned().collect()
    }

    /// List visible agents (not hidden)
    pub fn list_visible(&self) -> Vec<AgentDefinition> {
        let agents = read_or_recover(&self.agents);
        agents.values().filter(|a| !a.hidden).cloned().collect()
    }

    /// Check if an agent exists
    pub fn exists(&self, name: &str) -> bool {
        let agents = read_or_recover(&self.agents);
        agents.contains_key(name) || agents.contains_key(canonical_agent_name(name))
    }

    /// Get the number of registered agents
    pub fn len(&self) -> usize {
        let agents = read_or_recover(&self.agents);
        agents.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ============================================================================
// Agent File Loading
// ============================================================================

#[path = "subagent/loader.rs"]
mod loader;
pub use loader::{load_agents_from_dir, parse_agent_md, parse_agent_yaml};

/// Create built-in agent definitions
pub fn builtin_agents() -> Vec<AgentDefinition> {
    vec![
        // Explore agent: Fast codebase exploration (read-only)
        AgentDefinition::new(
            "explore",
            "Fast read-only exploration agent. Use for searching files, reading code, \
             understanding codebase structure, and gathering external web evidence.",
        )
        .native()
        .with_permissions(explore_permissions())
        .with_max_steps(20)
        .with_prompt(EXPLORE_PROMPT),
        // General agent: Multi-step task execution
        AgentDefinition::new(
            "general",
            "General-purpose agent for multi-step task execution. Can read, write, \
             and execute commands.",
        )
        .native()
        .with_permissions(general_permissions())
        .with_max_steps(50),
        // DeepResearch evidence agent: inherit the parent session's tool/skill
        // policy instead of applying explore's read-only default-deny policy.
        AgentDefinition::new(
            "deep-research",
            "DeepResearch evidence agent. Inherits the parent session's tools, \
             skills, permissions, and HITL policy for bounded evidence collection.",
        )
        .native()
        .hidden()
        .with_confirmation(ConfirmationInheritance::InheritParent)
        .with_max_steps(50),
        // Loop Engineering decision roles are intentionally tool-free. Makers
        // gather evidence; planners and checkers only make structured decisions.
        AgentDefinition::new(
            "loop-planner",
            "Tool-free semantic planner for engineered loops.",
        )
        .native()
        .hidden()
        .tool_free()
        .with_max_steps(4)
        .with_prompt(LOOP_PLANNER_PROMPT),
        AgentDefinition::new(
            "loop-checker",
            "Tool-free independent checker for engineered loops.",
        )
        .native()
        .hidden()
        .tool_free()
        .with_max_steps(4)
        .with_prompt(LOOP_CHECKER_PROMPT),
        // Plan agent: Read-only planning mode
        AgentDefinition::new(
            "plan",
            "Planning agent for designing implementation approaches. Read-only access \
             to explore codebase and create plans.",
        )
        .native()
        .with_permissions(plan_permissions())
        .with_max_steps(30)
        .with_prompt(PLAN_PROMPT),
        // Verification agent: adversarial validation and repro
        AgentDefinition::new(
            "verification",
            "Verification agent for adversarial validation. Prefer real checks, \
             reproductions, and regression testing over code reading alone.",
        )
        .native()
        .with_permissions(verification_permissions())
        .with_max_steps(30)
        .with_prompt(VERIFICATION_PROMPT),
        // Review agent: review-focused analysis
        AgentDefinition::new(
            "review",
            "Code review agent focused on correctness, regressions, security, \
             maintainability, and clear findings.",
        )
        .native()
        .with_permissions(review_permissions())
        .with_max_steps(25)
        .with_prompt(REVIEW_PROMPT),
    ]
}

// ============================================================================
// Permission Policies for Built-in Agents
// ============================================================================

/// Permission policy for explore agent (read-only)
fn explore_permissions() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .allow_all(&["read", "grep", "glob", "ls", "web_fetch", "web_search"])
        .deny_all(&["write", "edit", "task", "parallel_task"])
        .allow("Bash(ls:*)")
        .allow("Bash(cat:*)")
        .allow("Bash(head:*)")
        .allow("Bash(tail:*)")
        .allow("Bash(find:*)")
        .allow("Bash(wc:*)")
        .deny("Bash(rm:*)")
        .deny("Bash(mv:*)")
        .deny("Bash(cp:*)");
    policy.default_decision = PermissionDecision::Deny;
    policy
}

/// Permission policy for general agent (full access except task)
fn general_permissions() -> PermissionPolicy {
    PermissionPolicy::new()
        .allow_all(&[
            "read",
            "write",
            "edit",
            "grep",
            "glob",
            "ls",
            "bash",
            "web_fetch",
            "web_search",
            "git",
            "patch",
            "batch",
            "generate_object",
        ])
        .deny("task")
        .deny("parallel_task")
}

/// Permission policy for plan agent (read-only)
fn plan_permissions() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .allow_all(&["read", "grep", "glob", "ls"])
        .deny_all(&["write", "edit", "bash", "task", "parallel_task"]);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

/// Permission policy for verification agent (read-heavy with runtime checks)
fn verification_permissions() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .allow_all(&[
            "read",
            "grep",
            "glob",
            "ls",
            "bash",
            "web_fetch",
            "web_search",
        ])
        .deny_all(&["write", "edit", "task", "parallel_task"]);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

/// Permission policy for review agent (read-heavy with optional lightweight checks)
fn review_permissions() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .allow_all(&[
            "read",
            "grep",
            "glob",
            "ls",
            "bash",
            "web_fetch",
            "web_search",
        ])
        .deny_all(&["write", "edit", "task", "parallel_task"]);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

// ============================================================================
// System Prompts for Built-in Agents
// ============================================================================

const EXPLORE_PROMPT: &str = crate::prompts::AGENT_EXPLORE;

const PLAN_PROMPT: &str = crate::prompts::AGENT_PLAN;

const VERIFICATION_PROMPT: &str = crate::prompts::AGENT_VERIFICATION;

const REVIEW_PROMPT: &str = crate::prompts::AGENT_CODE_REVIEW;

const LOOP_PLANNER_PROMPT: &str = "You are the planner in an engineered loop. Make the requested structured planning decision directly from the supplied goal and constraints. You have no tools and must not request tool calls. Do not collect evidence or execute the plan.";

const LOOP_CHECKER_PROMPT: &str = "You are the independent checker in an engineered loop. Evaluate only the supplied plan and maker evidence, then return the requested structured decision. You have no tools and must not request tool calls or gather new evidence.";

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "subagent/tests.rs"]
mod tests;
