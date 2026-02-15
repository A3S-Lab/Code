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
}

impl std::fmt::Display for HookEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookEventType::PreToolUse => write!(f, "pre_tool_use"),
            HookEventType::PostToolUse => write!(f, "post_tool_use"),
            HookEventType::GenerateStart => write!(f, "generate_start"),
            HookEventType::GenerateEnd => write!(f, "generate_end"),
            HookEventType::SessionStart => write!(f, "session_start"),
            HookEventType::SessionEnd => write!(f, "session_end"),
            HookEventType::SkillLoad => write!(f, "skill_load"),
            HookEventType::SkillUnload => write!(f, "skill_unload"),
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

/// Unified hook event enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", content = "payload")]
pub enum HookEvent {
    #[serde(rename = "pre_tool_use")]
    PreToolUse(PreToolUseEvent),
    #[serde(rename = "post_tool_use")]
    PostToolUse(PostToolUseEvent),
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
}

impl HookEvent {
    /// Get the event type
    pub fn event_type(&self) -> HookEventType {
        match self {
            HookEvent::PreToolUse(_) => HookEventType::PreToolUse,
            HookEvent::PostToolUse(_) => HookEventType::PostToolUse,
            HookEvent::GenerateStart(_) => HookEventType::GenerateStart,
            HookEvent::GenerateEnd(_) => HookEventType::GenerateEnd,
            HookEvent::SessionStart(_) => HookEventType::SessionStart,
            HookEvent::SessionEnd(_) => HookEventType::SessionEnd,
            HookEvent::SkillLoad(_) => HookEventType::SkillLoad,
            HookEvent::SkillUnload(_) => HookEventType::SkillUnload,
        }
    }

    /// Get the session ID (returns empty string for skill events which are global)
    pub fn session_id(&self) -> &str {
        match self {
            HookEvent::PreToolUse(e) => &e.session_id,
            HookEvent::PostToolUse(e) => &e.session_id,
            HookEvent::GenerateStart(e) => &e.session_id,
            HookEvent::GenerateEnd(e) => &e.session_id,
            HookEvent::SessionStart(e) => &e.session_id,
            HookEvent::SessionEnd(e) => &e.session_id,
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
}
