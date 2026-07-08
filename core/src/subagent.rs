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

        if config.permission_checker.is_none() && self.has_defined_permissions() {
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

        // Confirmation inheritance: resolve Ask decisions in child runs.
        if config.confirmation_manager.is_none() {
            let inheritance = self.confirmation_inheritance.clone().unwrap_or_else(|| {
                if self.has_defined_permissions() {
                    ConfirmationInheritance::AutoApprove
                } else {
                    ConfirmationInheritance::DenyOnAsk
                }
            });
            match inheritance {
                ConfirmationInheritance::AutoApprove => {
                    config.confirmation_manager =
                        Some(Arc::new(crate::hitl::AutoApproveConfirmation));
                }
                ConfirmationInheritance::DenyOnAsk => { /* leave None — safety_gate denies */ }
                ConfirmationInheritance::InheritParent => { /* caller passes parent's manager */ }
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

/// Parse an agent definition from YAML content.
///
/// The YAML can describe either a full [`AgentDefinition`] or a cattle-style
/// [`WorkerAgentSpec`] by including a `kind` field.
pub fn parse_agent_yaml(content: &str) -> anyhow::Result<AgentDefinition> {
    let value: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|e| anyhow::anyhow!("Failed to parse agent YAML: {}", e))?;

    parse_agent_yaml_value(value, "agent YAML")
}

fn parse_agent_yaml_value(
    value: serde_yaml::Value,
    context: &str,
) -> anyhow::Result<AgentDefinition> {
    let tools = yaml_get_any(&value, &["tools", "allowedTools", "allowed_tools"])
        .map(parse_tools_field)
        .unwrap_or_default();
    let disallowed_tools = yaml_get_any(
        &value,
        &["disallowedTools", "disallowed-tools", "disallowed_tools"],
    )
    .map(parse_tools_field)
    .unwrap_or_default();

    if yaml_value_has_key(&value, "kind") {
        let mut spec: WorkerAgentSpec = serde_yaml::from_value(value)
            .map_err(|e| anyhow::anyhow!("Failed to parse worker {}: {}", context, e))?;
        validate_agent_name(&spec.name)?;
        apply_claude_style_tools_to_spec(&mut spec, &tools, &disallowed_tools);
        return Ok(spec.into_agent_definition());
    }

    let mut agent: AgentDefinition = serde_yaml::from_value(value)
        .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", context, e))?;
    validate_agent_name(&agent.name)?;
    apply_claude_style_tools_to_agent(&mut agent, &tools, &disallowed_tools);
    Ok(agent)
}

fn apply_claude_style_tools_to_agent(
    agent: &mut AgentDefinition,
    tools: &[String],
    disallowed_tools: &[String],
) {
    if !tools.is_empty() {
        agent.permissions = allow_only_permission_policy(tools);
    }
    if !disallowed_tools.is_empty() {
        let base = std::mem::take(&mut agent.permissions);
        agent.permissions = add_denied_tools(base, disallowed_tools);
    }
    if (!tools.is_empty() || !disallowed_tools.is_empty())
        && agent.confirmation_inheritance.is_none()
    {
        agent.confirmation_inheritance = Some(ConfirmationInheritance::AutoApprove);
    }
}

fn apply_claude_style_tools_to_spec(
    spec: &mut WorkerAgentSpec,
    tools: &[String],
    disallowed_tools: &[String],
) {
    if tools.is_empty() && disallowed_tools.is_empty() {
        return;
    }

    let base = if tools.is_empty() {
        spec.permissions
            .clone()
            .unwrap_or_else(|| spec.kind.default_permissions())
    } else {
        allow_only_permission_policy(tools)
    };
    spec.permissions = Some(add_denied_tools(base, disallowed_tools));
    if spec.confirmation_inheritance.is_none() {
        spec.confirmation_inheritance = Some(ConfirmationInheritance::AutoApprove);
    }
}

fn parse_worker_yaml_value(
    value: serde_yaml::Value,
    context: &str,
) -> anyhow::Result<WorkerAgentSpec> {
    let spec: WorkerAgentSpec = serde_yaml::from_value(value)
        .map_err(|e| anyhow::anyhow!("Failed to parse worker {}: {}", context, e))?;
    validate_agent_name(&spec.name)?;
    Ok(spec)
}

fn yaml_value_has_key(value: &serde_yaml::Value, key: &str) -> bool {
    value
        .as_mapping()
        .map(|mapping| mapping.contains_key(serde_yaml::Value::String(key.to_string())))
        .unwrap_or(false)
}

fn yaml_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(serde_yaml::Value::String(key.to_string())))
}

fn yaml_get_any<'a>(value: &'a serde_yaml::Value, keys: &[&str]) -> Option<&'a serde_yaml::Value> {
    keys.iter().find_map(|key| yaml_get(value, key))
}

fn parse_tools_field(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(str::to_string)
            .collect(),
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn tool_name_to_permission(tool: &str) -> String {
    let normalized = tool.trim();
    match normalized.to_ascii_lowercase().as_str() {
        "*" => "*".to_string(),
        "read" => "read(*)".to_string(),
        "write" => "write(*)".to_string(),
        "edit" => "edit(*)".to_string(),
        "grep" => "grep(*)".to_string(),
        "glob" => "glob(*)".to_string(),
        "ls" => "ls(*)".to_string(),
        "bash" => "bash(*)".to_string(),
        "task" => "task(*)".to_string(),
        "parallel_task" | "parallel-task" => "parallel_task(*)".to_string(),
        _ if normalized.contains('(') => normalized.to_string(),
        _ => format!("{normalized}(*)"),
    }
}

fn permission_policy_from_tools(tools: &[String]) -> PermissionPolicy {
    tools.iter().fold(PermissionPolicy::new(), |policy, tool| {
        policy.allow(&tool_name_to_permission(tool))
    })
}

fn allow_only_permission_policy(tools: &[String]) -> PermissionPolicy {
    let mut policy = permission_policy_from_tools(tools);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

fn add_denied_tools(mut policy: PermissionPolicy, tools: &[String]) -> PermissionPolicy {
    for tool in tools {
        policy = policy.deny(&tool_name_to_permission(tool));
    }
    policy
}

fn validate_agent_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        return Err(anyhow::anyhow!("Agent name is required"));
    }
    Ok(())
}

/// Parse an agent definition from Markdown with YAML frontmatter
///
/// The frontmatter contains agent metadata, and the body becomes the prompt.
pub fn parse_agent_md(content: &str) -> anyhow::Result<AgentDefinition> {
    // Parse frontmatter (YAML between --- markers)
    let parts: Vec<&str> = content.splitn(3, "---").collect();

    if parts.len() < 3 {
        return Err(anyhow::anyhow!(
            "Invalid markdown format: missing YAML frontmatter"
        ));
    }

    let frontmatter = parts[1].trim();
    let body = parts[2].trim();

    // Parse the frontmatter as YAML. A `kind` field selects WorkerAgentSpec.
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter)
        .map_err(|e| anyhow::anyhow!("Failed to parse agent frontmatter: {}", e))?;

    if yaml_value_has_key(&value, "kind") {
        let tools = yaml_get_any(&value, &["tools", "allowedTools", "allowed_tools"])
            .map(parse_tools_field)
            .unwrap_or_default();
        let disallowed_tools = yaml_get_any(
            &value,
            &["disallowedTools", "disallowed-tools", "disallowed_tools"],
        )
        .map(parse_tools_field)
        .unwrap_or_default();
        let mut spec = parse_worker_yaml_value(value, "frontmatter")?;
        if spec.prompt.is_none() && !body.is_empty() {
            spec.prompt = Some(body.to_string());
        }
        apply_claude_style_tools_to_spec(&mut spec, &tools, &disallowed_tools);
        return Ok(spec.into_agent_definition());
    }

    let mut agent = parse_agent_yaml_value(value, "agent frontmatter")?;

    // Use body as prompt if not already set in frontmatter.
    if agent.prompt.is_none() && !body.is_empty() {
        agent.prompt = Some(body.to_string());
    }

    Ok(agent)
}

/// Load all agent definitions from a directory
///
/// Scans for *.yaml and *.md files and parses them as agent definitions.
/// Invalid files are logged and skipped.
pub fn load_agents_from_dir(dir: &Path) -> Vec<AgentDefinition> {
    let mut agents = Vec::new();
    load_agents_from_dir_inner(dir, &mut agents);
    agents
}

fn load_agents_from_dir_inner(dir: &Path, agents: &mut Vec<AgentDefinition>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::warn!("Failed to read agent directory: {}", dir.display());
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            load_agents_from_dir_inner(&path, agents);
            continue;
        }
        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        // Read file content
        let Ok(content) = std::fs::read_to_string(&path) else {
            tracing::warn!("Failed to read agent file: {}", path.display());
            continue;
        };

        // Parse based on extension
        let result = match ext {
            "yaml" | "yml" => parse_agent_yaml(&content),
            "md" => parse_agent_md(&content),
            _ => continue,
        };

        match result {
            Ok(agent) => {
                tracing::debug!("Loaded agent '{}' from {}", agent.name, path.display());
                agents.push(agent);
            }
            Err(e) => {
                tracing::warn!("Failed to parse agent file {}: {}", path.display(), e);
            }
        }
    }
}

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
    }

    #[test]
    fn test_agent_registry_new() {
        let registry = AgentRegistry::new();

        // Should have built-in agents
        assert!(registry.exists("explore"));
        assert!(registry.exists("general"));
        assert!(registry.exists("plan"));
        assert!(registry.exists("verification"));
        assert!(registry.exists("review"));
        assert!(registry.exists("general-purpose"));
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn test_agent_registry_get() {
        let registry = AgentRegistry::new();

        let explore = registry.get("explore").unwrap();
        assert_eq!(explore.name, "explore");
        assert!(explore.native);
        assert!(!explore.hidden);

        let general = registry.get("general-purpose").unwrap();
        assert_eq!(general.name, "general");

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

        assert_eq!(visible.len(), all.len());
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
        assert!(names.contains(&"verification"));
        assert!(names.contains(&"review"));

        // Check explore is read-only (has deny rules for write)
        let explore = agents.iter().find(|a| a.name == "explore").unwrap();
        assert!(!explore.permissions.deny.is_empty());
    }

    // ========================================================================
    // Agent File Loading Tests
    // ========================================================================

    #[test]
    fn test_parse_agent_yaml() {
        let yaml = r#"
name: test-agent
description: A test agent
hidden: false
max_steps: 20
"#;
        let agent = parse_agent_yaml(yaml).unwrap();
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert!(!agent.hidden);
        assert_eq!(agent.max_steps, Some(20));
    }

    #[test]
    fn test_parse_agent_yaml_with_permissions() {
        let yaml = r#"
name: restricted-agent
description: Agent with permissions
permissions:
  allow:
    - rule: read
    - rule: grep
  deny:
    - rule: write
"#;
        let agent = parse_agent_yaml(yaml).unwrap();
        assert_eq!(agent.name, "restricted-agent");
        assert_eq!(agent.permissions.allow.len(), 2);
        assert_eq!(agent.permissions.deny.len(), 1);
        // Verify that deserialized rules actually match (tool_name populated)
        assert!(agent.permissions.allow[0].matches("read", &serde_json::json!({})));
        assert!(agent.permissions.allow[1].matches("grep", &serde_json::json!({})));
        assert!(agent.permissions.deny[0].matches("write", &serde_json::json!({})));
    }

    #[test]
    fn test_parse_agent_yaml_with_plain_string_permissions() {
        // Users naturally write plain strings in allow/deny lists
        let yaml = r#"
name: plain-agent
description: Agent with plain string permissions
permissions:
  allow:
    - read
    - grep
    - "Bash(cargo:*)"
  deny:
    - write
"#;
        let agent = parse_agent_yaml(yaml).unwrap();
        assert_eq!(agent.name, "plain-agent");
        assert_eq!(agent.permissions.allow.len(), 3);
        assert_eq!(agent.permissions.deny.len(), 1);
        // Verify rules are functional
        assert!(agent.permissions.allow[0].matches("read", &serde_json::json!({})));
        assert!(agent.permissions.allow[1].matches("grep", &serde_json::json!({})));
        assert!(agent.permissions.allow[2]
            .matches("Bash", &serde_json::json!({"command": "cargo build"})));
        assert!(agent.permissions.deny[0].matches("write", &serde_json::json!({})));
    }

    #[test]
    fn test_parse_claude_style_agent_md_tools_field() {
        let md = r#"---
name: code-reviewer
description: Use proactively after code changes to review quality
tools: Read, Grep, Glob, Bash
---
Review the changed code and return prioritized findings.
"#;
        let agent = parse_agent_md(md).unwrap();

        assert_eq!(agent.name, "code-reviewer");
        assert_eq!(
            agent.confirmation_inheritance,
            Some(ConfirmationInheritance::AutoApprove)
        );
        assert!(agent
            .permissions
            .allow
            .iter()
            .any(|r| r.matches("read", &serde_json::json!({}))));
        assert!(agent
            .permissions
            .allow
            .iter()
            .any(|r| r.matches("grep", &serde_json::json!({}))));
        assert!(agent
            .permissions
            .allow
            .iter()
            .any(|r| r.matches("bash", &serde_json::json!({}))));
        assert_eq!(
            agent
                .permissions
                .check("write", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Deny
        );
        assert!(agent
            .prompt
            .as_deref()
            .unwrap_or_default()
            .contains("prioritized findings"));
    }

    #[test]
    fn test_parse_claude_style_agent_md_disallowed_tools_field() {
        let md = r#"---
name: shell-checker
description: Use proactively to run safe shell checks
tools:
  - Read
  - Bash
disallowedTools:
  - Bash(rm:*)
  - Write
---
Run safe checks only.
"#;
        let agent = parse_agent_md(md).unwrap();

        assert_eq!(agent.name, "shell-checker");
        assert_eq!(
            agent
                .permissions
                .check("bash", &serde_json::json!({"command": "rm -rf target"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            agent
                .permissions
                .check("bash", &serde_json::json!({"command": "cargo test"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            agent
                .permissions
                .check("write", &serde_json::json!({"file_path": "x"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn test_parse_worker_agent_md_supports_claude_tools_fields() {
        let md = r#"---
name: planner-worker
description: Plan work
kind: planner
tools: Read, Grep
disallowedTools: Grep(secret:*)
---
Plan without editing.
"#;
        let agent = parse_agent_md(md).unwrap();

        assert_eq!(agent.name, "planner-worker");
        assert_eq!(
            agent
                .permissions
                .check("read", &serde_json::json!({"file_path": "src/lib.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            agent.permissions.check(
                "grep",
                &serde_json::json!({"pattern": "secret", "path": "src"})
            ),
            PermissionDecision::Deny
        );
        assert_eq!(
            agent
                .permissions
                .check("bash", &serde_json::json!({"command": "echo no"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn test_builtin_agent_permissions_are_bounded() {
        let registry = AgentRegistry::new();
        let explore = registry.get("explore").unwrap();
        let general = registry.get("general-purpose").unwrap();
        let verification = registry.get("verification").unwrap();
        let review = registry.get("review").unwrap();

        assert_eq!(
            explore
                .permissions
                .check("bash", &serde_json::json!({"command": "cargo test"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            explore
                .permissions
                .check("bash", &serde_json::json!({"command": "ls src"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            explore
                .permissions
                .check("web_search", &serde_json::json!({"query": "a3s"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            explore.permissions.check(
                "web_fetch",
                &serde_json::json!({"url": "https://example.com"})
            ),
            PermissionDecision::Allow
        );
        assert_eq!(
            explore
                .permissions
                .check("write", &serde_json::json!({"file_path": "x"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            general
                .permissions
                .check("parallel_task", &serde_json::json!({})),
            PermissionDecision::Deny
        );
        for agent in [verification, review] {
            assert_eq!(
                agent
                    .permissions
                    .check("bash", &serde_json::json!({"command": "cargo test"})),
                PermissionDecision::Allow,
                "{} should allow runtime checks",
                agent.name
            );
            assert_eq!(
                agent
                    .permissions
                    .check("web_search", &serde_json::json!({"query": "a3s"})),
                PermissionDecision::Allow,
                "{} should allow evidence search",
                agent.name
            );
            assert_eq!(
                agent.permissions.check(
                    "web_fetch",
                    &serde_json::json!({"url": "https://example.com"})
                ),
                PermissionDecision::Allow,
                "{} should allow source fetches",
                agent.name
            );
            assert_eq!(
                agent
                    .permissions
                    .check("write", &serde_json::json!({"file_path": "x"})),
                PermissionDecision::Deny,
                "{} should stay read-only for workspace writes",
                agent.name
            );
            assert_eq!(
                agent
                    .permissions
                    .check("parallel_task", &serde_json::json!({})),
                PermissionDecision::Deny,
                "{} should not recurse into more subagents",
                agent.name
            );
        }
    }

    #[test]
    fn test_parse_worker_agent_yaml_uses_cattle_defaults() {
        let yaml = r#"
name: frontend-fixer
description: Disposable frontend implementer
kind: implementer
max_steps: 7
"#;
        let agent = parse_agent_yaml(yaml).unwrap();

        assert_eq!(agent.name, "frontend-fixer");
        assert_eq!(agent.max_steps, Some(7));
        assert!(agent
            .permissions
            .allow
            .iter()
            .any(|r| r.matches("write", &serde_json::json!({}))));
        assert!(agent
            .permissions
            .deny
            .iter()
            .any(|r| r.matches("task", &serde_json::json!({}))));
    }

    #[test]
    fn test_parse_agent_yaml_missing_name() {
        let yaml = r#"
description: Agent without name
"#;
        let result = parse_agent_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_agent_md() {
        let md = r#"---
name: md-agent
description: Agent from markdown
max_steps: 15
---
# System Prompt

You are a helpful agent.
Do your best work.
"#;
        let agent = parse_agent_md(md).unwrap();
        assert_eq!(agent.name, "md-agent");
        assert_eq!(agent.description, "Agent from markdown");
        assert_eq!(agent.max_steps, Some(15));
        assert!(agent.prompt.is_some());
        assert!(agent.prompt.unwrap().contains("helpful agent"));
    }

    #[test]
    fn test_parse_agent_md_with_prompt_in_frontmatter() {
        let md = r#"---
name: prompt-agent
description: Agent with prompt in frontmatter
prompt: "Frontmatter prompt"
---
Body content that should be ignored
"#;
        let agent = parse_agent_md(md).unwrap();
        assert_eq!(agent.prompt.unwrap(), "Frontmatter prompt");
    }

    #[test]
    fn test_parse_worker_agent_md_uses_body_prompt() {
        let md = r#"---
name: review-cow
description: Disposable review worker
kind: reviewer
---
Review only the staged diff and return prioritized findings.
"#;
        let agent = parse_agent_md(md).unwrap();

        assert_eq!(agent.name, "review-cow");
        assert_eq!(
            agent.prompt.as_deref(),
            Some("Review only the staged diff and return prioritized findings.")
        );
        assert!(agent
            .permissions
            .deny
            .iter()
            .any(|r| r.matches("write", &serde_json::json!({}))));
    }

    #[test]
    fn test_parse_agent_md_missing_frontmatter() {
        let md = "Just markdown without frontmatter";
        let result = parse_agent_md(md);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_agents_from_dir() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Create a YAML agent file
        std::fs::write(
            temp_dir.path().join("agent1.yaml"),
            r#"
name: yaml-agent
description: Agent from YAML file
"#,
        )
        .unwrap();

        // Create a Markdown agent file
        std::fs::write(
            temp_dir.path().join("agent2.md"),
            r#"---
name: md-agent
description: Agent from Markdown file
---
System prompt here
"#,
        )
        .unwrap();

        // Create an invalid file (should be skipped)
        std::fs::write(temp_dir.path().join("invalid.yaml"), "not: valid: yaml: [").unwrap();

        // Create a nested agent file (Claude-style directories are recursive)
        std::fs::create_dir_all(temp_dir.path().join("nested")).unwrap();
        std::fs::write(
            temp_dir.path().join("nested").join("agent3.md"),
            r#"---
name: nested-agent
description: Agent from nested Markdown file
---
Nested prompt
"#,
        )
        .unwrap();

        // Create a non-agent file (should be skipped)
        std::fs::write(temp_dir.path().join("readme.txt"), "Just a text file").unwrap();

        let agents = load_agents_from_dir(temp_dir.path());
        assert_eq!(agents.len(), 3);

        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"yaml-agent"));
        assert!(names.contains(&"md-agent"));
        assert!(names.contains(&"nested-agent"));
    }

    #[test]
    fn test_load_agents_from_nonexistent_dir() {
        let agents = load_agents_from_dir(std::path::Path::new("/nonexistent/dir"));
        assert!(agents.is_empty());
    }

    #[test]
    fn test_registry_with_config() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Create an agent file
        std::fs::write(
            temp_dir.path().join("custom.yaml"),
            r#"
name: custom-agent
description: Custom agent from config
"#,
        )
        .unwrap();

        let config = CodeConfig::new().add_agent_dir(temp_dir.path());
        let registry = AgentRegistry::with_config(&config);

        // Should have built-in agents plus custom agent
        assert!(registry.exists("explore"));
        assert!(registry.exists("custom-agent"));
        assert_eq!(registry.len(), 6); // 5 built-in + 1 custom
    }

    #[test]
    fn test_agent_definition_with_model() {
        let model = ModelConfig {
            model: "claude-3-5-sonnet".to_string(),
            provider: Some("anthropic".to_string()),
        };
        let agent = AgentDefinition::new("test", "Test").with_model(model);
        assert!(agent.model.is_some());
        assert_eq!(agent.model.unwrap().provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_model_config_from_model_ref() {
        let model = ModelConfig::from_model_ref("openai/gpt-4o");
        assert_eq!(model.provider.as_deref(), Some("openai"));
        assert_eq!(model.model, "gpt-4o");
        assert_eq!(model.model_ref(), "openai/gpt-4o");

        let inherited = ModelConfig::from_model_ref("claude-sonnet");
        assert_eq!(inherited.provider, None);
        assert_eq!(inherited.model_ref(), "claude-sonnet");
    }

    #[test]
    fn test_worker_agent_kind_from_str_accepts_aliases() {
        assert_eq!(
            "explore".parse::<WorkerAgentKind>().unwrap(),
            WorkerAgentKind::ReadOnly
        );
        assert_eq!(
            "general".parse::<WorkerAgentKind>().unwrap(),
            WorkerAgentKind::Implementer
        );
        assert!("unknown".parse::<WorkerAgentKind>().is_err());
    }

    #[test]
    fn worker_spec_implementer_creates_cattle_agent_definition() {
        let agent = WorkerAgentSpec::implementer("frontend-fixer", "Fix frontend issues")
            .with_prompt("Focus on small, verified patches.")
            .with_provider_model("anthropic", "claude-sonnet")
            .with_max_steps(12)
            .into_agent_definition();

        assert_eq!(agent.name, "frontend-fixer");
        assert_eq!(agent.max_steps, Some(12));
        assert_eq!(
            agent.prompt.as_deref(),
            Some("Focus on small, verified patches.")
        );
        assert_eq!(agent.model.unwrap().provider.as_deref(), Some("anthropic"));
        assert!(agent
            .permissions
            .allow
            .iter()
            .any(|r| r.matches("write", &serde_json::json!({}))));
        assert!(agent
            .permissions
            .deny
            .iter()
            .any(|r| r.matches("task", &serde_json::json!({}))));
    }

    #[test]
    fn worker_spec_read_only_uses_safe_defaults() {
        let agent = WorkerAgentSpec::read_only("scanner", "Scan repository")
            .hidden(true)
            .into_agent_definition();

        assert!(agent.hidden);
        assert_eq!(agent.max_steps, Some(20));
        assert!(agent.prompt.is_some());
        assert!(agent
            .permissions
            .allow
            .iter()
            .any(|r| r.matches("read", &serde_json::json!({}))));
        assert!(agent
            .permissions
            .deny
            .iter()
            .any(|r| r.matches("write", &serde_json::json!({}))));
    }

    #[test]
    fn registry_register_worker_returns_and_stores_definition() {
        let registry = AgentRegistry::new();
        let agent =
            registry.register_worker(WorkerAgentSpec::custom("strict-worker", "Strict worker"));

        assert_eq!(agent.name, "strict-worker");
        assert!(registry.exists("strict-worker"));
        assert_eq!(
            agent
                .permissions
                .check("bash", &serde_json::json!({"command":"echo hi"})),
            crate::permissions::PermissionDecision::Ask
        );
    }

    #[test]
    fn registry_register_workers_batches_cattle_specs() {
        let registry = AgentRegistry::new();
        let agents = registry.register_workers([
            WorkerAgentSpec::planner("planner-cow", "Plan work"),
            WorkerAgentSpec::verifier("verify-cow", "Verify work"),
        ]);

        assert_eq!(agents.len(), 2);
        assert!(registry.exists("planner-cow"));
        assert!(registry.exists("verify-cow"));
    }

    #[test]
    fn test_agent_registry_default() {
        let registry = AgentRegistry::default();
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn test_agent_registry_is_empty() {
        let registry = AgentRegistry {
            agents: RwLock::new(HashMap::new()),
        };
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_apply_to_sets_permissions() {
        use crate::agent::AgentConfig;
        use crate::permissions::PermissionDecision;

        let def = AgentDefinition::new("writer", "Write files")
            .with_permissions(PermissionPolicy::new().allow("write(*)"));

        let mut config = AgentConfig::default();
        assert!(config.permission_checker.is_none());

        def.apply_to(&mut config);

        assert!(config.permission_checker.is_some());
        assert!(config.permission_policy.is_some());
        let checker = config.permission_checker.unwrap();
        assert_eq!(
            checker.check(
                "write",
                &serde_json::json!({"file_path": "x.txt", "content": "hi"})
            ),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn test_apply_to_sets_prompt() {
        use crate::agent::AgentConfig;

        let def = AgentDefinition::new("helper", "Help").with_prompt("Be helpful.");
        let mut config = AgentConfig::default();

        def.apply_to(&mut config);

        assert_eq!(config.prompt_slots.extra.as_deref(), Some("Be helpful."));
    }

    #[test]
    fn test_apply_to_sets_max_steps() {
        use crate::agent::AgentConfig;

        let def = AgentDefinition::new("fast", "Fast agent").with_max_steps(7);
        let mut config = AgentConfig::default();

        def.apply_to(&mut config);

        assert_eq!(config.max_tool_rounds, 7);
    }

    #[test]
    fn test_apply_to_respects_host_overrides() {
        use crate::agent::AgentConfig;

        let def = AgentDefinition::new("agent", "Agent")
            .with_permissions(PermissionPolicy::new().allow("bash(*)"))
            .with_prompt("Agent prompt.")
            .with_max_steps(10);

        let mut config = AgentConfig::default();
        config.prompt_slots.extra = Some("Host prompt.".to_string());
        config.max_tool_rounds = 25;
        config.permission_checker = Some(std::sync::Arc::new(PermissionPolicy::new().allow("*")));

        def.apply_to(&mut config);

        // Host overrides should be preserved
        assert_eq!(config.prompt_slots.extra.as_deref(), Some("Host prompt."));
        assert_eq!(config.max_tool_rounds, 25);
    }

    #[test]
    fn test_apply_to_skips_empty_permissions() {
        use crate::agent::AgentConfig;

        let def = AgentDefinition::new("empty", "No permissions");
        let mut config = AgentConfig::default();

        def.apply_to(&mut config);

        assert!(config.permission_checker.is_none());
        assert!(config.permission_policy.is_none());
    }
}
