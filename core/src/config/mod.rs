//! Configuration module for A3S Code
//!
//! Provides configuration for:
//! - LLM providers and models (defaultModel in "provider/model" format, providers)
//! - Queue configuration (a3s-lane integration)
//! - Search configuration (a3s-search integration)
//! - Directories for dynamic skill and agent loading
//!
//! Configuration is loaded from ACL-compatible files or strings.
//! Existing `.acl` config filenames are still accepted for compatibility.
//! JSON support has been removed.

mod acl_render;
mod editor;
mod goal_control;
mod loader;
#[cfg(test)]
mod loader_tests;
mod provider;
mod search;
#[cfg(test)]
mod tests;
mod verification_contract;
mod workspace_trust;

pub use editor::{rewrite_acl_sections, ConfigSection};
pub(crate) use loader::parse_mcp_server_block;
pub use loader::{
    host_authoritative_block, withhold_untrusted_host_authoritative_blocks, LayerAcl,
    HOST_AUTHORITATIVE_BLOCKS,
};
pub use provider::{ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
pub use search::{
    BrowserBackend, DocumentCacheConfig, DocumentParserConfig, HeadlessConfig, SearchCascadeTier,
    SearchConfig, SearchEngineConfig, SearchHealthConfig,
};
pub use verification_contract::WorkspaceVerificationContract;
pub use workspace_trust::{
    admit_untrusted_layers, workspace_mcp_server_id, workspace_mcp_trust_path,
    WorkspaceLayerAdmission, WorkspaceMcpDeclaration, WorkspaceMcpTrust, WORKSPACE_MCP_TRUST_FILE,
};

use crate::memory::MemoryConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// Storage Configuration
// ============================================================================

/// Session storage backend type
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// In-memory storage (no persistence)
    Memory,
    /// File-based storage (JSON files)
    #[default]
    File,
    /// Custom external storage (Redis, PostgreSQL, etc.)
    ///
    /// Requires a `SessionStore` implementation registered on `AgentSession` options.
    /// Use `storage_url` in config to pass connection details.
    Custom,
}

// ============================================================================
// Main Configuration
// ============================================================================

/// Automatic subagent delegation controls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AutoDelegationConfig {
    /// Enable runtime-driven automatic child agent delegation.
    pub enabled: bool,
    /// Allow automatic delegation to launch multiple child agents in parallel.
    ///
    /// Manual `task` fan-out remains available when this is false.
    #[serde(alias = "auto_parallel")]
    pub auto_parallel: bool,
    /// Allow the model-visible `task` tool (including multi-item fan-out).
    ///
    /// Set this to false for cost control or debugging when child-agent tools
    /// should be absent from the session tool surface. This is not a security
    /// sandbox: the parent agent may still have other tools such as `bash`,
    /// MCP tools, or skills.
    #[serde(alias = "allow_manual_delegation")]
    pub allow_manual_delegation: bool,
    /// Minimum local confidence required to auto-delegate a child task.
    pub min_confidence: f32,
    /// Maximum number of automatic child tasks per user request.
    pub max_tasks: usize,
}

impl Default for AutoDelegationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_parallel: true,
            allow_manual_delegation: true,
            min_confidence: 0.72,
            max_tasks: 4,
        }
    }
}

/// Optional platform endpoint used by hosts for account login.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct OsConfig {
    /// Base address of the configured platform instance.
    #[serde(alias = "url", alias = "baseUrl", alias = "base_url")]
    pub address: String,
}

/// Configuration for A3S Code
/// Parsed `workspace_retrieval { enabled, local_cpu {} }` block: the
/// host-facing semantic-retrieval declaration. The feature itself is
/// host-injected (`WorkspaceRetrievalOptions` needs an embedding
/// provider no ACL can construct); sessions do not silently enable it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceRetrievalConfig {
    pub enabled: bool,
    pub local_cpu: bool,
}

/// Parsed `notifications { run_completion, min_seconds }` block: the
/// host-facing completion-signal preference. `bell` writes a terminal
/// bell to the agent's stderr when a run completes after the minimum
/// duration; focus detection and OS-level notifications stay with the
/// terminal host (WC-NOTIFY1 boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NotificationsConfig {
    pub run_completion: RunCompletionSignal,
    pub min_seconds: u64,
}

/// Closed set of completion signals the agent itself can emit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunCompletionSignal {
    /// A terminal bell (`\x07`) on stderr when a run completes after
    /// the minimum duration.
    Bell,
    /// No signal (the default posture when the block is absent).
    #[default]
    Off,
}

impl RunCompletionSignal {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bell" => Ok(Self::Bell),
            "off" => Ok(Self::Off),
            other => anyhow::bail!(
                "notifications run_completion must be one of the closed set \
                 [bell, off]; got {other:?}"
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeConfig {
    /// Host-selected finite cross-Run bounds; absence disables automatic work.
    /// Workspace-discovered ACL cannot supply or override this policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_control: Option<crate::goal_control::GoalContinuationPolicyV1>,
    /// Host-authoritative, workspace-scoped suite verification declarations.
    /// ACP resolves these against the actual workspace at session creation.
    /// Automatically discovered workspace ACL cannot supply them.
    #[serde(default)]
    pub verification_contracts: Vec<WorkspaceVerificationContract>,
    /// Default model in "provider/model" format (e.g., "anthropic/claude-sonnet-4-20250514")
    #[serde(default, alias = "default_model")]
    pub default_model: Option<String>,

    /// Provider configurations
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Parsed `workspace_retrieval` block, when declared.
    #[serde(default)]
    pub workspace_retrieval: Option<WorkspaceRetrievalConfig>,
    /// Parsed `notifications` block, when declared; absent = off.
    #[serde(default)]
    pub notifications: Option<NotificationsConfig>,

    /// Session storage backend
    #[serde(default)]
    pub storage_backend: StorageBackend,

    /// Sessions directory (for file backend)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_dir: Option<PathBuf>,

    /// Memory directory for the default file-backed memory store.
    ///
    /// If unset, sessions use `<workspace>/.a3s/memory` unless the host passes
    /// an explicit memory store or file memory directory.
    #[serde(default, alias = "memoryDir", skip_serializing_if = "Option::is_none")]
    pub memory_dir: Option<PathBuf>,

    /// Connection URL for custom storage backend (e.g., "redis://localhost:6379", "postgres://user:pass@localhost/a3s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_url: Option<String>,

    /// Directories to scan for skill files (*.md with tool definitions)
    #[serde(default, alias = "skill_dirs")]
    pub skill_dirs: Vec<PathBuf>,

    /// Directories to scan for agent files (*.yaml or *.md)
    #[serde(default, alias = "agent_dirs")]
    pub agent_dirs: Vec<PathBuf>,

    /// Directory containing personal instructions loaded before project files.
    ///
    /// When unset, instruction discovery checks `~/.a3s/AGENTS.override.md`
    /// and then `~/.a3s/AGENTS.md`, using the first non-empty regular file.
    #[serde(
        default,
        alias = "user_instructions_dir",
        alias = "globalInstructionsDir",
        alias = "global_instructions_dir",
        skip_serializing_if = "Option::is_none"
    )]
    pub user_instructions_dir: Option<PathBuf>,

    /// Maximum combined bytes loaded from personal and project instruction files.
    ///
    /// When unset, instruction discovery uses the Codex-compatible 32 KiB
    /// default. A value of zero disables all automatic instruction loading.
    #[serde(default, alias = "project_doc_max_bytes")]
    pub project_doc_max_bytes: Option<usize>,

    /// Ordered fallback filenames checked after `AGENTS.override.md` and
    /// `AGENTS.md` in every directory from the project root to the workspace.
    #[serde(default, alias = "project_doc_fallback_filenames")]
    pub project_doc_fallback_filenames: Vec<String>,

    /// Maximum tool execution rounds per turn (default: 50)
    #[serde(default, alias = "max_tool_rounds")]
    pub max_tool_rounds: Option<usize>,

    /// Maximum sibling branches/tools to run concurrently in bounded parallel fan-out paths.
    #[serde(default, alias = "max_parallel_tasks")]
    pub max_parallel_tasks: Option<usize>,

    /// Agent-wide priority and concurrency admission shared by all sessions.
    #[serde(default, alias = "task_scheduler")]
    pub task_scheduler: crate::task_scheduler::TaskSchedulerConfig,

    /// Global automatic child-agent delegation settings.
    #[serde(default, alias = "auto_delegation")]
    pub auto_delegation: AutoDelegationConfig,

    /// Convenience global kill switch for automatic parallel child-agent fan-out.
    ///
    /// When set, overrides `auto_delegation.auto_parallel`.
    #[serde(default, alias = "auto_parallel")]
    pub auto_parallel: Option<bool>,

    /// Thinking/reasoning budget in tokens
    #[serde(default, alias = "thinking_budget")]
    pub thinking_budget: Option<usize>,

    /// Per-model API HTTP timeout in milliseconds. Separate from tool execution
    /// timeouts so provider/network deadlines do not constrain local tools.
    #[serde(
        default,
        alias = "llm_api_timeout_ms",
        alias = "api_timeout_ms",
        alias = "model_api_timeout_ms"
    )]
    pub llm_api_timeout_ms: Option<u64>,

    /// Memory system configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryConfig>,

    /// Queue configuration (a3s-lane integration)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<crate::queue::SessionQueueConfig>,

    /// Search configuration (a3s-search integration)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchConfig>,

    /// Network posture of the default local sandbox (ACL `sandbox` block:
    /// `network = "deny" | "mediated"`, `network_allow = ["host[:port]", …]`).
    /// Deny is the fail-closed default; mediated requires a non-empty
    /// allowlist and is an explicit host opt-in.
    #[serde(default)]
    pub sandbox_network: crate::sandbox::SandboxNetworkMode,
    #[serde(default)]
    pub sandbox_network_allow: Vec<String>,

    /// Optional platform endpoint. When set, hosts may enable account login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsConfig>,

    /// Built-in document context extraction configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_parser: Option<DocumentParserConfig>,

    /// MCP server configurations
    #[serde(default, alias = "mcp_servers")]
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,
}
