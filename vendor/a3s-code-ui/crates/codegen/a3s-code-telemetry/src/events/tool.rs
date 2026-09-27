//! Tool-call and model-response product telemetry events.

use serde::Serialize;

/// Host invocation id. Only a UUID is representable, so a provider call id or a path cannot be written here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InvocationId(String);

impl InvocationId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }

    pub fn from_host(id: &str) -> Option<Self> {
        uuid::Uuid::parse_str(id).ok().map(|_| Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Qualified registry id, or the one opaque class for unknown, custom, and dynamic names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CanonicalToolId(String);

impl CanonicalToolId {
    pub const OPAQUE: &'static str = "opaque";

    pub fn opaque() -> Self {
        Self(Self::OPAQUE.to_owned())
    }

    /// `Namespace:id` from a finalized registration. Rejects aliases, MCP names, and paths.
    pub fn from_qualified(id: &str) -> Option<Self> {
        let (namespace, tool) = id.split_once(':')?;
        if namespace.is_empty()
            || tool.is_empty()
            || namespace == "MCP"
            || tool.contains(':')
            || id.contains("__")
            || id.contains('/')
            || id.contains('\\')
            || id.contains(' ')
            || !namespace.chars().all(|c| c.is_ascii_alphanumeric())
            || !tool.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return None;
        }
        Some(Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Managed behavior version. Anything else is omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ToolContractVersion(&'static str);

impl ToolContractVersion {
    pub const ALLOWED: &'static [&'static str] = &["current", "legacy-0.4.10"];

    pub fn from_registered(version: &str) -> Option<Self> {
        Self::ALLOWED
            .iter()
            .find(|known| **known == version)
            .map(|known| Self(known))
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// Requested model that product events already treat as a a3s model id. Custom names stay absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ProductModelId(String);

impl ProductModelId {
    pub fn from_requested(model: &str) -> Option<Self> {
        if !is_approved_model_id(model) || crate::redact_common::redact_owned(model).is_some() {
            return None;
        }
        Some(Self(model.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_approved_model_id(model: &str) -> bool {
    if model == "grok" {
        return true;
    }
    let Some(rest) = model.strip_prefix("grok-") else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 64
        && !rest.starts_with('-')
        && !rest.ends_with('-')
        && !rest.contains("--")
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ToolSourceStatus {
    Unknown,
    Succeeded,
    Empty,
    Failed,
    Partial,
}

impl ToolSourceStatus {
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
pub enum ToolSourceReason {
    #[serde(rename = "not_instrumented")]
    #[strum(serialize = "not_instrumented")]
    NotInstrumented,
    #[serde(rename = "search.unclassified_exit")]
    #[strum(serialize = "search.unclassified_exit")]
    SearchUnclassifiedExit,
    #[serde(rename = "read.not_found")]
    #[strum(serialize = "read.not_found")]
    ReadNotFound,
    #[serde(rename = "read.directory")]
    #[strum(serialize = "read.directory")]
    ReadDirectory,
    #[serde(rename = "read.denied")]
    #[strum(serialize = "read.denied")]
    ReadDenied,
    #[serde(rename = "read.ignored")]
    #[strum(serialize = "read.ignored")]
    ReadIgnored,
    #[serde(rename = "read.binary")]
    #[strum(serialize = "read.binary")]
    ReadBinary,
    #[serde(rename = "read.token_limit")]
    #[strum(serialize = "read.token_limit")]
    ReadTokenLimit,
    #[serde(rename = "read.io")]
    #[strum(serialize = "read.io")]
    ReadIo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum InvocationSource {
    Model,
    UserDirect,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ToolOutputLimit {
    Unobserved,
    NotLimited,
    Limited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ReadFileRole {
    SkillEntry,
    SkillSupport,
    Instruction,
    Memory,
    Ordinary,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ReadSkillMatch {
    Registered,
    Unregistered,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ReadSkillSource {
    Local,
    Repo,
    User,
    Server,
    Bundled,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ReadSelection {
    Full,
    ModelWindow,
    DefaultWindow,
    SkillFullRead,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ReadLimitKind {
    None,
    Lines,
    Bytes,
    Tokens,
    Multiple,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CapApplicability {
    Applies,
    NotApplicable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CapDisposition {
    Unobserved,
    WithinLimit,
    Truncated,
    Rejected,
    Exempt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadProfile {
    pub read_file_role: ReadFileRole,
    pub read_skill_match: ReadSkillMatch,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_skill_source: Option<ReadSkillSource>,
    pub read_selection: ReadSelection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_source_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_returned_lines: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_returned_bytes: Option<i64>,
    pub read_limit_kind: ReadLimitKind,
    pub read_lines_applicability: CapApplicability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_lines_limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_lines_observed: Option<i64>,
    pub read_lines_disposition: CapDisposition,
    pub read_bytes_applicability: CapApplicability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_bytes_limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_bytes_observed: Option<i64>,
    pub read_bytes_disposition: CapDisposition,
    pub read_tokens_applicability: CapApplicability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_tokens_limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_tokens_observed: Option<i64>,
    pub read_tokens_disposition: CapDisposition,
}

/// Content-free location class. The path itself is not a product field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum PathScope {
    Workspace,
    Tmp,
    Home,
    Other,
}

#[doc(hidden)]
pub fn completed_for_test(tool_name: &str, model: &str) -> ToolCallCompleted {
    ToolCallCompleted {
        tool_name: tool_name.to_owned(),
        outcome: a3s_code_session_events::types::ToolOutcome::Success,
        hook_rewrote: false,
        duration_ms: 1,
        tool_result_size_bytes: None,
        model_id: ProductModelId::from_requested(model),
        invocation_id: InvocationId::from_host("018f6b6c-7b3a-7c3a-8c3a-000000000001")
            .expect("fixed invocation id"),
        tool_id: CanonicalToolId::opaque(),
        tool_version: None,
        source_status: ToolSourceStatus::Unknown,
        source_reason: Some(ToolSourceReason::NotInstrumented),
        path_scope: None,
        invocation_source: None,
        output_limit: None,
        read: None,
        external_model_id: model.to_owned(),
        file_path: None,
        parameters: None,
        tool_use_id: None,
        tool_output: None,
        error_message: None,
    }
}

#[derive(Serialize)]
pub struct ToolCallCompleted {
    pub tool_name: String,
    pub outcome: a3s_code_session_events::types::ToolOutcome,
    /// Content-free: the hook name is kept out of OTLP and product events and rides only the session-event row.
    pub hook_rewrote: bool,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_size_bytes: Option<u64>,
    /// Snapshotted requested model, only when it is an approved a3s model id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ProductModelId>,
    pub invocation_id: InvocationId,
    pub tool_id: CanonicalToolId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<ToolContractVersion>,
    pub source_status: ToolSourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_reason: Option<ToolSourceReason>,
    /// Registered read, search-replace, and write ids. A client-facing rename still qualifies.
    /// Absent for an unknown id or a missing path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_scope: Option<PathScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation_source: Option<InvocationSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_limit: Option<ToolOutputLimit>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub read: Option<ReadProfile>,
    /// Raw requested model for the external stream. Not a product field.
    #[serde(skip)]
    pub external_model_id: String,
    /// Primary file path of the call, for the external stream only (`#[serde(skip)]`: never serialized to product events/analytics).
    /// Always reduced to `file_extension`; the full path rides the `OTEL_LOG_TOOL_DETAILS` gate.
    #[serde(skip)]
    pub file_path: Option<String>,
    /// Tool parameters for the external stream's `OTEL_LOG_TOOL_DETAILS`
    /// 4 KB preview **and** `OTEL_LOG_TOOL_CONTENT` full `tool_input`
    /// (`#[serde(skip)]`; reduced / capped at emit time).
    #[serde(skip)]
    pub parameters: Option<serde_json::Value>,
    /// Tool-call id for the external stream (`#[serde(skip)]`; always-on
    /// join key, not content).
    #[serde(skip)]
    pub tool_use_id: Option<String>,
    /// Tool result body for `OTEL_LOG_TOOL_CONTENT` (`#[serde(skip)]`).
    #[serde(skip)]
    pub tool_output: Option<String>,
    /// Failure text for `OTEL_LOG_TOOL_CONTENT` (`#[serde(skip)]`).
    #[serde(skip)]
    pub error_message: Option<String>,
}

#[derive(Serialize)]
pub struct ModelResponseReceived {
    pub model_id: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_prompt_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u32>,
    /// USD ticks (1e10 ticks = $1); `None` when unpriced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd_ticks: Option<i64>,
}

/// Emitted once per turn via [`crate::external::emit`] (not [`crate::session_ctx::log_event`]), so Mixpanel never
/// receives `assistant_response`. Thinking/tool-use blocks are excluded at the source. `response_length` always exports
/// on the external event; `response_text` is `#[serde(skip)]` and gated by `OTEL_LOG_ASSISTANT_RESPONSES`.
#[derive(Serialize)]
pub struct AssistantResponse {
    /// Char/byte count of the assembled text blocks (always-on, like
    /// `prompt_length`). Zero on tool-only turns.
    pub response_length: usize,
    /// Gated by `OTEL_LOG_ASSISTANT_RESPONSES`; omitted when length is 0.
    #[serde(skip)]
    pub response_text: Option<String>,
}
