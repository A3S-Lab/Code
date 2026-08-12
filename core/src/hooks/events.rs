//! Hook Event Types
//!
//! Defines all event types that can trigger hooks.

use serde::{Deserialize, Serialize};

/// Hook event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEventType {
    /// Before tool execution
    PreToolUse,
    /// After tool execution
    PostToolUse,
    /// Before interactive permission is requested
    PermissionRequest,
    /// Before context compaction
    PreCompact,
    /// After context compaction
    PostCompact,
    /// Before LLM generation
    GenerateStart,
    /// After LLM generation
    GenerateEnd,
    /// When session is created
    SessionStart,
    /// When session is destroyed
    SessionEnd,
    /// When a skill is loaded
    SkillLoad,
    /// When a skill is unloaded
    SkillUnload,
    /// Before prompt augmentation (can modify prompt)
    PrePrompt,
    /// After LLM response is processed, before returning to user
    PostResponse,
    /// When an error occurs (tool failure, LLM error, etc.)
    OnError,
    // === New harness points ===
    /// Before context perception (model needs workspace knowledge)
    PreContextPerception,
    /// After context perception
    PostContextPerception,

    /// When an operation succeeds (mirrors OnError for success case)
    OnSuccess,

    /// Before memory recall (model needs to retrieve from memory)
    PreMemoryRecall,
    /// After memory recall completes
    PostMemoryRecall,

    /// Before task planning/decomposition
    PrePlanning,
    /// After planning completes
    PostPlanning,

    /// Before reasoning (CoT/ToT start)
    PreReasoning,
    /// After reasoning completes
    PostReasoning,

    /// When rate limit is triggered
    OnRateLimit,

    /// When user confirmation is needed
    OnConfirmation,

    /// Intent detection - detect user intent from prompt (blocking)
    IntentDetection,
}

impl std::fmt::Display for HookEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookEventType::PreToolUse => write!(f, "pre_tool_use"),
            HookEventType::PostToolUse => write!(f, "post_tool_use"),
            HookEventType::PermissionRequest => write!(f, "permission_request"),
            HookEventType::PreCompact => write!(f, "pre_compact"),
            HookEventType::PostCompact => write!(f, "post_compact"),
            HookEventType::GenerateStart => write!(f, "generate_start"),
            HookEventType::GenerateEnd => write!(f, "generate_end"),
            HookEventType::SessionStart => write!(f, "session_start"),
            HookEventType::SessionEnd => write!(f, "session_end"),
            HookEventType::SkillLoad => write!(f, "skill_load"),
            HookEventType::SkillUnload => write!(f, "skill_unload"),
            HookEventType::PrePrompt => write!(f, "pre_prompt"),
            HookEventType::PostResponse => write!(f, "post_response"),
            HookEventType::OnError => write!(f, "on_error"),
            // New harness points
            HookEventType::PreContextPerception => write!(f, "pre_context_perception"),
            HookEventType::PostContextPerception => write!(f, "post_context_perception"),
            HookEventType::OnSuccess => write!(f, "on_success"),
            HookEventType::PreMemoryRecall => write!(f, "pre_memory_recall"),
            HookEventType::PostMemoryRecall => write!(f, "post_memory_recall"),
            HookEventType::PrePlanning => write!(f, "pre_planning"),
            HookEventType::PostPlanning => write!(f, "post_planning"),
            HookEventType::PreReasoning => write!(f, "pre_reasoning"),
            HookEventType::PostReasoning => write!(f, "post_reasoning"),
            HookEventType::OnRateLimit => write!(f, "on_rate_limit"),
            HookEventType::OnConfirmation => write!(f, "on_confirmation"),
            HookEventType::IntentDetection => write!(f, "intent_detection"),
        }
    }
}

/// Tool execution result data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultData {
    /// Whether execution succeeded
    pub success: bool,
    /// Tool output
    pub output: String,
    /// Exit code (for shell commands)
    pub exit_code: Option<i32>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Pre-tool-use event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolUseEvent {
    /// Session ID
    pub session_id: String,
    /// Tool name
    pub tool: String,
    /// Tool arguments
    pub args: serde_json::Value,
    /// Working directory
    pub working_directory: String,
    /// Recent tools executed (for context)
    pub recent_tools: Vec<String>,
}

/// Post-tool-use event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolUseEvent {
    /// Session ID
    pub session_id: String,
    /// Tool name
    pub tool: String,
    /// Tool arguments
    pub args: serde_json::Value,
    /// Execution result
    pub result: ToolResultData,
}

/// Permission-request event payload, fired before the host confirmation UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequestEvent {
    pub session_id: String,
    pub tool_id: String,
    pub tool: String,
    pub args: serde_json::Value,
}

/// Pre-compaction event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreCompactEvent {
    pub session_id: String,
    pub message_count: usize,
    pub used_tokens: usize,
    pub max_tokens: usize,
}

/// Post-compaction event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostCompactEvent {
    pub session_id: String,
    pub message_count_before: usize,
    pub message_count_after: usize,
    pub summary_generated: bool,
}

/// Generate start event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateStartEvent {
    /// Session ID
    pub session_id: String,
    /// User prompt
    pub prompt: String,
    /// System prompt (if any)
    pub system_prompt: Option<String>,
    /// Model provider
    pub model_provider: String,
    /// Model name
    pub model_name: String,
    /// Available tools
    pub available_tools: Vec<String>,
}

/// Generate end event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateEndEvent {
    /// Session ID
    pub session_id: String,
    /// User prompt
    pub prompt: String,
    /// Response text
    pub response_text: String,
    /// Tool calls made
    pub tool_calls: Vec<ToolCallInfo>,
    /// Token usage
    pub usage: TokenUsageInfo,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Tool call information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    /// Tool name
    pub name: String,
    /// Tool arguments
    pub args: serde_json::Value,
}

/// Token usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageInfo {
    /// Prompt tokens
    pub prompt_tokens: i32,
    /// Completion tokens
    pub completion_tokens: i32,
    /// Total tokens
    pub total_tokens: i32,
}

/// Session start event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartEvent {
    /// Session ID
    pub session_id: String,
    /// System prompt (if any)
    pub system_prompt: Option<String>,
    /// Model configuration
    pub model_provider: String,
    pub model_name: String,
}

/// Session end event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEndEvent {
    /// Session ID
    pub session_id: String,
    /// Total token usage
    pub total_tokens: i32,
    /// Total tool calls
    pub total_tool_calls: i32,
    /// Session duration in milliseconds
    pub duration_ms: u64,
}

/// Skill load event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLoadEvent {
    /// Skill name
    pub skill_name: String,
    /// Tool names loaded from the skill
    pub tool_names: Vec<String>,
    /// Skill version (if available)
    pub version: Option<String>,
    /// Skill description (if available)
    pub description: Option<String>,
    /// Timestamp when skill was loaded (Unix milliseconds)
    pub loaded_at: i64,
}

/// Skill unload event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUnloadEvent {
    /// Skill name
    pub skill_name: String,
    /// Tool names that were unloaded
    pub tool_names: Vec<String>,
    /// How long the skill was loaded (milliseconds)
    pub duration_ms: u64,
}

/// Pre-prompt event payload (fired before prompt augmentation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrePromptEvent {
    /// Session ID
    pub session_id: String,
    /// User prompt text
    pub prompt: String,
    /// Current system prompt (if any)
    pub system_prompt: Option<String>,
    /// Number of messages in conversation history
    pub message_count: usize,
}

/// Post-response event payload (fired after LLM response is processed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostResponseEvent {
    /// Session ID
    pub session_id: String,
    /// Final response text
    pub response_text: String,
    /// Number of tool calls made during this turn
    pub tool_calls_count: usize,
    /// Token usage
    pub usage: TokenUsageInfo,
    /// Total duration in milliseconds
    pub duration_ms: u64,
}

/// Error type classification for OnError events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    /// Tool execution failed
    ToolFailure,
    /// LLM API call failed
    LlmFailure,
    /// Permission denied
    PermissionDenied,
    /// Timeout
    Timeout,
    /// Other error
    Other,
}

impl std::fmt::Display for ErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorType::ToolFailure => write!(f, "tool_failure"),
            ErrorType::LlmFailure => write!(f, "llm_failure"),
            ErrorType::PermissionDenied => write!(f, "permission_denied"),
            ErrorType::Timeout => write!(f, "timeout"),
            ErrorType::Other => write!(f, "other"),
        }
    }
}

/// On-error event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnErrorEvent {
    /// Session ID
    pub session_id: String,
    /// Error classification
    pub error_type: ErrorType,
    /// Error message
    pub error_message: String,
    /// Additional context (e.g., tool name, model name)
    pub context: serde_json::Value,
}

// ============================================================================
// New Driving Point Payloads
// ============================================================================

/// Pre-context-perception event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreContextPerceptionEvent {
    pub session_id: String,
    pub intent: String,
    pub target_type: String,
    pub target_name: String,
    pub domain: String,
    pub query: Option<String>,
    pub working_directory: String,
    pub urgency: String,
}

/// Post-context-perception event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostContextPerceptionEvent {
    pub session_id: String,
    pub intent: String,
    pub target_type: String,
    pub success: bool,
    pub facts_retrieved: usize,
    pub files_retrieved: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// On-success event payload (mirrors OnError for success)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnSuccessEvent {
    /// Session ID
    pub session_id: String,
    /// Action type that succeeded
    pub action_type: String,
    /// Summary of the successful action
    pub action_summary: String,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Pre-memory-recall event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreMemoryRecallEvent {
    pub session_id: String,
    /// Query or intent for recall
    pub query: String,
    /// Memory type (e.g., "semantic", "episodic", "working")
    pub memory_type: String,
    /// Maximum results to retrieve
    pub max_results: usize,
    /// Current working directory
    pub working_directory: String,
}

/// Post-memory-recall event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostMemoryRecallEvent {
    pub session_id: String,
    pub query: String,
    pub memory_type: String,
    pub facts_retrieved: usize,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Planning strategy type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningStrategy {
    None,
    StepByStep,
    TreeOfThoughts,
    GraphPlanning,
    Custom(String),
}

/// Pre-planning event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrePlanningEvent {
    pub session_id: String,
    /// Task description to plan
    pub task_description: String,
    /// Available planning strategies
    pub available_strategies: Vec<PlanningStrategy>,
    /// Constraints or requirements
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<serde_json::Value>,
}

/// Post-planning event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostPlanningEvent {
    pub session_id: String,
    pub task_description: String,
    pub strategy_used: PlanningStrategy,
    /// Generated subtasks or plan steps
    pub subtasks: Vec<String>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Reasoning type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningType {
    ChainOfThought,
    TreeOfThoughts,
    ReAct,
    Reflexion,
    Other(String),
}

/// Pre-reasoning event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreReasoningEvent {
    pub session_id: String,
    /// Type of reasoning being performed
    pub reasoning_type: ReasoningType,
    /// Problem or question to reason about
    pub problem_statement: String,
    /// Available hints or context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<String>>,
}

/// Post-reasoning event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostReasoningEvent {
    pub session_id: String,
    pub reasoning_type: ReasoningType,
    pub conclusion: String,
    pub steps_count: usize,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Rate limit type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitType {
    LlmTokenLimit,
    LlmRequestLimit,
    ApiRequestLimit,
    ToolExecutionLimit,
    Custom(String),
}

/// On-rate-limit event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnRateLimitEvent {
    pub session_id: String,
    /// Type of rate limit
    pub limit_type: RateLimitType,
    /// Retry after milliseconds (suggested)
    pub retry_after_ms: u64,
    /// Current usage information
    pub current_usage: String,
}

/// Confirmation type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationType {
    SafetyConfirm,
    UserConfirm,
    CostConfirm,
    Custom(String),
}

/// On-confirmation event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnConfirmationEvent {
    pub session_id: String,
    /// Type of confirmation needed
    pub confirmation_type: ConfirmationType,
    /// Message to show to user
    pub message: String,
    /// Options to present (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// Intent detection event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentDetectionEvent {
    pub session_id: String,
    pub prompt: String,
    pub workspace: String,
    /// Optional language hint auto-detected from input
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_hint: Option<String>,
}

/// Unified hook event enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", content = "payload")]
pub enum HookEvent {
    #[serde(rename = "pre_tool_use")]
    PreToolUse(PreToolUseEvent),
    #[serde(rename = "post_tool_use")]
    PostToolUse(PostToolUseEvent),
    #[serde(rename = "permission_request")]
    PermissionRequest(PermissionRequestEvent),
    #[serde(rename = "pre_compact")]
    PreCompact(PreCompactEvent),
    #[serde(rename = "post_compact")]
    PostCompact(PostCompactEvent),
    #[serde(rename = "generate_start")]
    GenerateStart(GenerateStartEvent),
    #[serde(rename = "generate_end")]
    GenerateEnd(GenerateEndEvent),
    #[serde(rename = "session_start")]
    SessionStart(SessionStartEvent),
    #[serde(rename = "session_end")]
    SessionEnd(SessionEndEvent),
    #[serde(rename = "skill_load")]
    SkillLoad(SkillLoadEvent),
    #[serde(rename = "skill_unload")]
    SkillUnload(SkillUnloadEvent),
    #[serde(rename = "pre_prompt")]
    PrePrompt(PrePromptEvent),
    #[serde(rename = "post_response")]
    PostResponse(PostResponseEvent),
    #[serde(rename = "on_error")]
    OnError(OnErrorEvent),
    // New harness points
    #[serde(rename = "pre_context_perception")]
    PreContextPerception(PreContextPerceptionEvent),
    #[serde(rename = "post_context_perception")]
    PostContextPerception(PostContextPerceptionEvent),
    #[serde(rename = "on_success")]
    OnSuccess(OnSuccessEvent),
    #[serde(rename = "pre_memory_recall")]
    PreMemoryRecall(PreMemoryRecallEvent),
    #[serde(rename = "post_memory_recall")]
    PostMemoryRecall(PostMemoryRecallEvent),
    #[serde(rename = "pre_planning")]
    PrePlanning(PrePlanningEvent),
    #[serde(rename = "post_planning")]
    PostPlanning(PostPlanningEvent),
    #[serde(rename = "pre_reasoning")]
    PreReasoning(PreReasoningEvent),
    #[serde(rename = "post_reasoning")]
    PostReasoning(PostReasoningEvent),
    #[serde(rename = "on_rate_limit")]
    OnRateLimit(OnRateLimitEvent),
    #[serde(rename = "on_confirmation")]
    OnConfirmation(OnConfirmationEvent),
    #[serde(rename = "intent_detection")]
    IntentDetection(IntentDetectionEvent),
}

impl HookEvent {
    /// Get the event type
    pub fn event_type(&self) -> HookEventType {
        match self {
            HookEvent::PreToolUse(_) => HookEventType::PreToolUse,
            HookEvent::PostToolUse(_) => HookEventType::PostToolUse,
            HookEvent::PermissionRequest(_) => HookEventType::PermissionRequest,
            HookEvent::PreCompact(_) => HookEventType::PreCompact,
            HookEvent::PostCompact(_) => HookEventType::PostCompact,
            HookEvent::GenerateStart(_) => HookEventType::GenerateStart,
            HookEvent::GenerateEnd(_) => HookEventType::GenerateEnd,
            HookEvent::SessionStart(_) => HookEventType::SessionStart,
            HookEvent::SessionEnd(_) => HookEventType::SessionEnd,
            HookEvent::SkillLoad(_) => HookEventType::SkillLoad,
            HookEvent::SkillUnload(_) => HookEventType::SkillUnload,
            HookEvent::PrePrompt(_) => HookEventType::PrePrompt,
            HookEvent::PostResponse(_) => HookEventType::PostResponse,
            HookEvent::OnError(_) => HookEventType::OnError,
            // New harness points
            HookEvent::PreContextPerception(_) => HookEventType::PreContextPerception,
            HookEvent::PostContextPerception(_) => HookEventType::PostContextPerception,
            HookEvent::OnSuccess(_) => HookEventType::OnSuccess,
            HookEvent::PreMemoryRecall(_) => HookEventType::PreMemoryRecall,
            HookEvent::PostMemoryRecall(_) => HookEventType::PostMemoryRecall,
            HookEvent::PrePlanning(_) => HookEventType::PrePlanning,
            HookEvent::PostPlanning(_) => HookEventType::PostPlanning,
            HookEvent::PreReasoning(_) => HookEventType::PreReasoning,
            HookEvent::PostReasoning(_) => HookEventType::PostReasoning,
            HookEvent::OnRateLimit(_) => HookEventType::OnRateLimit,
            HookEvent::OnConfirmation(_) => HookEventType::OnConfirmation,
            HookEvent::IntentDetection(_) => HookEventType::IntentDetection,
        }
    }

    /// Get the session ID (returns empty string for skill events which are global)
    pub fn session_id(&self) -> &str {
        match self {
            HookEvent::PreToolUse(e) => &e.session_id,
            HookEvent::PostToolUse(e) => &e.session_id,
            HookEvent::PermissionRequest(e) => &e.session_id,
            HookEvent::PreCompact(e) => &e.session_id,
            HookEvent::PostCompact(e) => &e.session_id,
            HookEvent::GenerateStart(e) => &e.session_id,
            HookEvent::GenerateEnd(e) => &e.session_id,
            HookEvent::SessionStart(e) => &e.session_id,
            HookEvent::SessionEnd(e) => &e.session_id,
            HookEvent::PrePrompt(e) => &e.session_id,
            HookEvent::PostResponse(e) => &e.session_id,
            HookEvent::OnError(e) => &e.session_id,
            // New harness points
            HookEvent::PreContextPerception(e) => &e.session_id,
            HookEvent::PostContextPerception(e) => &e.session_id,
            HookEvent::OnSuccess(e) => &e.session_id,
            HookEvent::PreMemoryRecall(e) => &e.session_id,
            HookEvent::PostMemoryRecall(e) => &e.session_id,
            HookEvent::PrePlanning(e) => &e.session_id,
            HookEvent::PostPlanning(e) => &e.session_id,
            HookEvent::PreReasoning(e) => &e.session_id,
            HookEvent::PostReasoning(e) => &e.session_id,
            HookEvent::OnRateLimit(e) => &e.session_id,
            HookEvent::OnConfirmation(e) => &e.session_id,
            HookEvent::IntentDetection(e) => &e.session_id,
            // Skill events are global (not session-specific)
            HookEvent::SkillLoad(_) => "",
            HookEvent::SkillUnload(_) => "",
        }
    }

    /// Get the tool name (for tool events)
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            HookEvent::PreToolUse(e) => Some(&e.tool),
            HookEvent::PostToolUse(e) => Some(&e.tool),
            _ => None,
        }
    }

    /// Get the tool args (for tool events)
    pub fn tool_args(&self) -> Option<&serde_json::Value> {
        match self {
            HookEvent::PreToolUse(e) => Some(&e.args),
            HookEvent::PostToolUse(e) => Some(&e.args),
            _ => None,
        }
    }

    /// Get the skill name (for skill events)
    pub fn skill_name(&self) -> Option<&str> {
        match self {
            HookEvent::SkillLoad(e) => Some(&e.skill_name),
            HookEvent::SkillUnload(e) => Some(&e.skill_name),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_type_display() {
        assert_eq!(HookEventType::PreToolUse.to_string(), "pre_tool_use");
        assert_eq!(HookEventType::PostToolUse.to_string(), "post_tool_use");
        assert_eq!(HookEventType::GenerateStart.to_string(), "generate_start");
        assert_eq!(HookEventType::GenerateEnd.to_string(), "generate_end");
        assert_eq!(HookEventType::SessionStart.to_string(), "session_start");
        assert_eq!(HookEventType::SessionEnd.to_string(), "session_end");
        assert_eq!(HookEventType::SkillLoad.to_string(), "skill_load");
        assert_eq!(HookEventType::SkillUnload.to_string(), "skill_unload");
    }

    #[test]
    fn test_pre_tool_use_event() {
        let event = PreToolUseEvent {
            session_id: "session-1".to_string(),
            tool: "Bash".to_string(),
            args: serde_json::json!({"command": "echo hello"}),
            working_directory: "/workspace".to_string(),
            recent_tools: vec!["Read".to_string()],
        };

        assert_eq!(event.session_id, "session-1");
        assert_eq!(event.tool, "Bash");
    }

    #[test]
    fn test_post_tool_use_event() {
        let event = PostToolUseEvent {
            session_id: "session-1".to_string(),
            tool: "Bash".to_string(),
            args: serde_json::json!({"command": "echo hello"}),
            result: ToolResultData {
                success: true,
                output: "hello\n".to_string(),
                exit_code: Some(0),
                duration_ms: 50,
            },
        };

        assert!(event.result.success);
        assert_eq!(event.result.exit_code, Some(0));
    }

    #[test]
    fn test_hook_event_type() {
        let pre_tool = HookEvent::PreToolUse(PreToolUseEvent {
            session_id: "s1".to_string(),
            tool: "Bash".to_string(),
            args: serde_json::json!({}),
            working_directory: "/".to_string(),
            recent_tools: vec![],
        });

        assert_eq!(pre_tool.event_type(), HookEventType::PreToolUse);
        assert_eq!(pre_tool.session_id(), "s1");
        assert_eq!(pre_tool.tool_name(), Some("Bash"));
    }

    #[test]
    fn test_hook_event_serialization() {
        let event = HookEvent::PreToolUse(PreToolUseEvent {
            session_id: "s1".to_string(),
            tool: "Bash".to_string(),
            args: serde_json::json!({"command": "ls"}),
            working_directory: "/workspace".to_string(),
            recent_tools: vec![],
        });

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("pre_tool_use"));
        assert!(json.contains("Bash"));

        // Deserialize back
        let parsed: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type(), HookEventType::PreToolUse);
    }

    #[test]
    fn test_generate_events() {
        let start = GenerateStartEvent {
            session_id: "s1".to_string(),
            prompt: "Hello".to_string(),
            system_prompt: Some("You are helpful".to_string()),
            model_provider: "anthropic".to_string(),
            model_name: "claude-3".to_string(),
            available_tools: vec!["Bash".to_string(), "Read".to_string()],
        };

        let end = GenerateEndEvent {
            session_id: "s1".to_string(),
            prompt: "Hello".to_string(),
            response_text: "Hi there!".to_string(),
            tool_calls: vec![],
            usage: TokenUsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            duration_ms: 500,
        };

        assert_eq!(start.prompt, "Hello");
        assert_eq!(end.response_text, "Hi there!");
        assert_eq!(end.usage.total_tokens, 15);
    }

    #[test]
    fn test_session_events() {
        let start = SessionStartEvent {
            session_id: "s1".to_string(),
            system_prompt: Some("System".to_string()),
            model_provider: "anthropic".to_string(),
            model_name: "claude-3".to_string(),
        };

        let end = SessionEndEvent {
            session_id: "s1".to_string(),
            total_tokens: 1000,
            total_tool_calls: 5,
            duration_ms: 60000,
        };

        let start_event = HookEvent::SessionStart(start);
        let end_event = HookEvent::SessionEnd(end);

        assert_eq!(start_event.event_type(), HookEventType::SessionStart);
        assert_eq!(end_event.event_type(), HookEventType::SessionEnd);
        assert!(start_event.tool_name().is_none());
    }

    #[test]
    fn test_skill_load_event() {
        let event = SkillLoadEvent {
            skill_name: "test-skill".to_string(),
            tool_names: vec!["tool1".to_string(), "tool2".to_string()],
            version: Some("1.0.0".to_string()),
            description: Some("A test skill".to_string()),
            loaded_at: 1234567890,
        };

        assert_eq!(event.skill_name, "test-skill");
        assert_eq!(event.tool_names.len(), 2);
        assert_eq!(event.version, Some("1.0.0".to_string()));
        assert_eq!(event.loaded_at, 1234567890);
    }

    #[test]
    fn test_skill_unload_event() {
        let event = SkillUnloadEvent {
            skill_name: "test-skill".to_string(),
            tool_names: vec!["tool1".to_string(), "tool2".to_string()],
            duration_ms: 60000,
        };

        assert_eq!(event.skill_name, "test-skill");
        assert_eq!(event.tool_names.len(), 2);
        assert_eq!(event.duration_ms, 60000);
    }

    #[test]
    fn test_hook_event_skill_name() {
        let load_event = HookEvent::SkillLoad(SkillLoadEvent {
            skill_name: "my-skill".to_string(),
            tool_names: vec!["tool1".to_string()],
            version: None,
            description: None,
            loaded_at: 0,
        });

        let unload_event = HookEvent::SkillUnload(SkillUnloadEvent {
            skill_name: "my-skill".to_string(),
            tool_names: vec!["tool1".to_string()],
            duration_ms: 1000,
        });

        assert_eq!(load_event.event_type(), HookEventType::SkillLoad);
        assert_eq!(load_event.skill_name(), Some("my-skill"));
        assert_eq!(load_event.session_id(), ""); // Skills are global

        assert_eq!(unload_event.event_type(), HookEventType::SkillUnload);
        assert_eq!(unload_event.skill_name(), Some("my-skill"));
        assert_eq!(unload_event.session_id(), ""); // Skills are global

        // Non-skill events return None for skill_name
        let pre_tool = HookEvent::PreToolUse(PreToolUseEvent {
            session_id: "s1".to_string(),
            tool: "Bash".to_string(),
            args: serde_json::json!({}),
            working_directory: "/".to_string(),
            recent_tools: vec![],
        });
        assert!(pre_tool.skill_name().is_none());
    }

    #[test]
    fn test_skill_event_serialization() {
        let event = HookEvent::SkillLoad(SkillLoadEvent {
            skill_name: "test-skill".to_string(),
            tool_names: vec!["tool1".to_string()],
            version: Some("1.0.0".to_string()),
            description: None,
            loaded_at: 1234567890,
        });

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("skill_load"));
        assert!(json.contains("test-skill"));
        assert!(json.contains("1.0.0"));

        let parsed: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type(), HookEventType::SkillLoad);
        assert_eq!(parsed.skill_name(), Some("test-skill"));
    }

    #[test]
    fn test_hook_event_type_display_new_variants() {
        assert_eq!(HookEventType::PrePrompt.to_string(), "pre_prompt");
        assert_eq!(HookEventType::PostResponse.to_string(), "post_response");
        assert_eq!(HookEventType::OnError.to_string(), "on_error");
    }

    #[test]
    fn test_pre_prompt_event() {
        let event = PrePromptEvent {
            session_id: "s1".to_string(),
            prompt: "Fix the bug".to_string(),
            system_prompt: Some("You are helpful".to_string()),
            message_count: 5,
        };

        assert_eq!(event.session_id, "s1");
        assert_eq!(event.prompt, "Fix the bug");
        assert_eq!(event.message_count, 5);

        let hook_event = HookEvent::PrePrompt(event);
        assert_eq!(hook_event.event_type(), HookEventType::PrePrompt);
        assert_eq!(hook_event.session_id(), "s1");
        assert!(hook_event.tool_name().is_none());
        assert!(hook_event.skill_name().is_none());
    }

    #[test]
    fn test_post_response_event() {
        let event = PostResponseEvent {
            session_id: "s1".to_string(),
            response_text: "Done!".to_string(),
            tool_calls_count: 3,
            usage: TokenUsageInfo {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            },
            duration_ms: 2000,
        };

        assert_eq!(event.response_text, "Done!");
        assert_eq!(event.tool_calls_count, 3);
        assert_eq!(event.usage.total_tokens, 150);

        let hook_event = HookEvent::PostResponse(event);
        assert_eq!(hook_event.event_type(), HookEventType::PostResponse);
        assert_eq!(hook_event.session_id(), "s1");
    }

    #[test]
    fn test_on_error_event() {
        let event = OnErrorEvent {
            session_id: "s1".to_string(),
            error_type: ErrorType::ToolFailure,
            error_message: "Command failed with exit code 1".to_string(),
            context: serde_json::json!({"tool": "Bash", "command": "false"}),
        };

        assert_eq!(event.error_type.to_string(), "tool_failure");
        assert_eq!(event.error_message, "Command failed with exit code 1");

        let hook_event = HookEvent::OnError(event);
        assert_eq!(hook_event.event_type(), HookEventType::OnError);
        assert_eq!(hook_event.session_id(), "s1");
    }

    #[test]
    fn test_error_type_display() {
        assert_eq!(ErrorType::ToolFailure.to_string(), "tool_failure");
        assert_eq!(ErrorType::LlmFailure.to_string(), "llm_failure");
        assert_eq!(ErrorType::PermissionDenied.to_string(), "permission_denied");
        assert_eq!(ErrorType::Timeout.to_string(), "timeout");
        assert_eq!(ErrorType::Other.to_string(), "other");
    }

    #[test]
    fn test_new_event_serialization() {
        // PrePrompt
        let event = HookEvent::PrePrompt(PrePromptEvent {
            session_id: "s1".to_string(),
            prompt: "Hello".to_string(),
            system_prompt: None,
            message_count: 0,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("pre_prompt"));
        let parsed: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type(), HookEventType::PrePrompt);

        // PostResponse
        let event = HookEvent::PostResponse(PostResponseEvent {
            session_id: "s1".to_string(),
            response_text: "Hi".to_string(),
            tool_calls_count: 0,
            usage: TokenUsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            duration_ms: 100,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("post_response"));
        let parsed: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type(), HookEventType::PostResponse);

        // OnError
        let event = HookEvent::OnError(OnErrorEvent {
            session_id: "s1".to_string(),
            error_type: ErrorType::LlmFailure,
            error_message: "API timeout".to_string(),
            context: serde_json::json!({}),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("on_error"));
        let parsed: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type(), HookEventType::OnError);
    }
}
