//! Hook Engine
//!
//! Core engine responsible for managing and executing hooks.

use super::events::{HookEvent, HookEventType};
use super::matcher::HookMatcher;
use super::{HookAction, HookResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

/// Hook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Priority (lower values = higher priority)
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// Timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    /// Whether to execute asynchronously (fire-and-forget)
    #[serde(default)]
    pub async_execution: bool,

    /// Maximum retry attempts
    #[serde(default)]
    pub max_retries: u32,
}

fn default_priority() -> i32 {
    100
}

fn default_timeout() -> u64 {
    30000
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            priority: default_priority(),
            timeout_ms: default_timeout(),
            async_execution: false,
            max_retries: 0,
        }
    }
}

/// Hook definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    /// Unique hook identifier
    pub id: String,

    /// Event type that triggers this hook
    pub event_type: HookEventType,

    /// Event matcher (optional, None matches all events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<HookMatcher>,

    /// Hook configuration
    #[serde(default)]
    pub config: HookConfig,
}

impl Hook {
    /// Create a new hook
    pub fn new(id: impl Into<String>, event_type: HookEventType) -> Self {
        Self {
            id: id.into(),
            event_type,
            matcher: None,
            config: HookConfig::default(),
        }
    }

    /// Set the matcher
    pub fn with_matcher(mut self, matcher: HookMatcher) -> Self {
        self.matcher = Some(matcher);
        self
    }

    /// Set the configuration
    pub fn with_config(mut self, config: HookConfig) -> Self {
        self.config = config;
        self
    }

    /// Check if an event matches this hook
    pub fn matches(&self, event: &HookEvent) -> bool {
        // First check event type
        if event.event_type() != self.event_type {
            return false;
        }

        // If there's a matcher, check it
        if let Some(ref matcher) = self.matcher {
            matcher.matches(event)
        } else {
            true
        }
    }
}

/// Hook execution result
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Continue execution (with optional modified data)
    Continue(Option<serde_json::Value>),
    /// Block execution
    Block(String),
    /// Retry after delay (milliseconds)
    Retry(u64),
    /// Skip remaining hooks but continue execution
    Skip,
}

impl HookResult {
    /// Create a continue result
    pub fn continue_() -> Self {
        Self::Continue(None)
    }

    /// Create a continue result with modifications
    pub fn continue_with(modified: serde_json::Value) -> Self {
        Self::Continue(Some(modified))
    }

    /// Create a block result
    pub fn block(reason: impl Into<String>) -> Self {
        Self::Block(reason.into())
    }

    /// Create a retry result
    pub fn retry(delay_ms: u64) -> Self {
        Self::Retry(delay_ms)
    }

    /// Create a skip result
    pub fn skip() -> Self {
        Self::Skip
    }

    /// Check if this is a continue result
    pub fn is_continue(&self) -> bool {
        matches!(self, Self::Continue(_))
    }

    /// Check if this is a block result
    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block(_))
    }
}

/// Hook handler trait
pub trait HookHandler: Send + Sync {
    /// Handle a hook event
    fn handle(&self, event: &HookEvent) -> HookResponse;
}

/// Hook engine
pub struct HookEngine {
    /// Registered hooks
    hooks: Arc<RwLock<HashMap<String, Hook>>>,

    /// Hook handlers (registered by SDK)
    handlers: Arc<RwLock<HashMap<String, Arc<dyn HookHandler>>>>,

    /// Event sender channel (for SDK listeners)
    event_tx: Option<mpsc::Sender<HookEvent>>,
}

impl Default for HookEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HookEngine {
    /// Create a new hook engine
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(RwLock::new(HashMap::new())),
            handlers: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// Set the event sender channel
    pub fn with_event_channel(mut self, tx: mpsc::Sender<HookEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Register a hook
    pub fn register(&self, hook: Hook) {
        let mut hooks = self.hooks.write().unwrap();
        hooks.insert(hook.id.clone(), hook);
    }

    /// Unregister a hook
    pub fn unregister(&self, hook_id: &str) -> Option<Hook> {
        let mut hooks = self.hooks.write().unwrap();
        hooks.remove(hook_id)
    }

    /// Register a handler
    pub fn register_handler(&self, hook_id: &str, handler: Arc<dyn HookHandler>) {
        let mut handlers = self.handlers.write().unwrap();
        handlers.insert(hook_id.to_string(), handler);
    }

    /// Unregister a handler
    pub fn unregister_handler(&self, hook_id: &str) {
        let mut handlers = self.handlers.write().unwrap();
        handlers.remove(hook_id);
    }

    /// Get all hooks matching an event (sorted by priority)
    pub fn matching_hooks(&self, event: &HookEvent) -> Vec<Hook> {
        let hooks = self.hooks.read().unwrap();
        let mut matching: Vec<Hook> = hooks
            .values()
            .filter(|h| h.matches(event))
            .cloned()
            .collect();

        // Sort by priority (lower values = higher priority)
        matching.sort_by_key(|h| h.config.priority);
        matching
    }

    /// Fire an event and get the result
    pub async fn fire(&self, event: &HookEvent) -> HookResult {
        // Send event to channel if available
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event.clone()).await;
        }

        // Get matching hooks
        let matching_hooks = self.matching_hooks(event);

        if matching_hooks.is_empty() {
            return HookResult::continue_();
        }

        // Execute each hook
        for hook in matching_hooks {
            let result = self.execute_hook(&hook, event).await;

            match result {
                HookResult::Continue(modified) => {
                    // If modified, can apply to subsequent hooks
                    // For now, simple handling: continue to next hook
                    if modified.is_some() {
                        return HookResult::Continue(modified);
                    }
                }
                HookResult::Block(reason) => {
                    return HookResult::Block(reason);
                }
                HookResult::Retry(delay) => {
                    return HookResult::Retry(delay);
                }
                HookResult::Skip => {
                    return HookResult::Continue(None);
                }
            }
        }

        HookResult::continue_()
    }

    /// Execute a single hook
    async fn execute_hook(&self, hook: &Hook, event: &HookEvent) -> HookResult {
        // Find handler
        let handler = {
            let handlers = self.handlers.read().unwrap();
            handlers.get(&hook.id).cloned()
        };

        match handler {
            Some(h) => {
                // Handler found, execute it
                let response = if hook.config.async_execution {
                    // Async execution (fire-and-forget)
                    let h = h.clone();
                    let event = event.clone();
                    tokio::spawn(async move {
                        h.handle(&event);
                    });
                    HookResponse::continue_()
                } else {
                    // Sync execution (with timeout)
                    let timeout = std::time::Duration::from_millis(hook.config.timeout_ms);
                    let h = h.clone();
                    let event = event.clone();

                    match tokio::time::timeout(timeout, async move { h.handle(&event) }).await {
                        Ok(response) => response,
                        Err(_) => {
                            // Timeout, continue execution
                            HookResponse::continue_()
                        }
                    }
                };

                self.response_to_result(response)
            }
            None => {
                // No handler, continue execution
                HookResult::continue_()
            }
        }
    }

    /// Convert HookResponse to HookResult
    fn response_to_result(&self, response: HookResponse) -> HookResult {
        match response.action {
            HookAction::Continue => HookResult::Continue(response.modified),
            HookAction::Block => {
                HookResult::Block(response.reason.unwrap_or_else(|| "Blocked".to_string()))
            }
            HookAction::Retry => HookResult::Retry(response.retry_delay_ms.unwrap_or(1000)),
            HookAction::Skip => HookResult::Skip,
        }
    }

    /// Get the number of registered hooks
    pub fn hook_count(&self) -> usize {
        self.hooks.read().unwrap().len()
    }

    /// Get a hook by ID
    pub fn get_hook(&self, id: &str) -> Option<Hook> {
        self.hooks.read().unwrap().get(id).cloned()
    }

    /// Get all hooks
    pub fn all_hooks(&self) -> Vec<Hook> {
        self.hooks.read().unwrap().values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::events::PreToolUseEvent;

    fn make_pre_tool_event(session_id: &str, tool: &str) -> HookEvent {
        HookEvent::PreToolUse(PreToolUseEvent {
            session_id: session_id.to_string(),
            tool: tool.to_string(),
            args: serde_json::json!({}),
            working_directory: "/workspace".to_string(),
            recent_tools: vec![],
        })
    }

    #[test]
    fn test_hook_config_default() {
        let config = HookConfig::default();
        assert_eq!(config.priority, 100);
        assert_eq!(config.timeout_ms, 30000);
        assert!(!config.async_execution);
        assert_eq!(config.max_retries, 0);
    }

    #[test]
    fn test_hook_new() {
        let hook = Hook::new("test-hook", HookEventType::PreToolUse);
        assert_eq!(hook.id, "test-hook");
        assert_eq!(hook.event_type, HookEventType::PreToolUse);
        assert!(hook.matcher.is_none());
    }

    #[test]
    fn test_hook_with_matcher() {
        let hook = Hook::new("test-hook", HookEventType::PreToolUse)
            .with_matcher(HookMatcher::tool("Bash"));

        assert!(hook.matcher.is_some());
        assert_eq!(hook.matcher.unwrap().tool, Some("Bash".to_string()));
    }

    #[test]
    fn test_hook_matches_event_type() {
        let hook = Hook::new("test-hook", HookEventType::PreToolUse);

        let pre_event = make_pre_tool_event("s1", "Bash");
        assert!(hook.matches(&pre_event));

        // PostToolUse doesn't match
        let post_event = HookEvent::PostToolUse(crate::hooks::events::PostToolUseEvent {
            session_id: "s1".to_string(),
            tool: "Bash".to_string(),
            args: serde_json::json!({}),
            result: crate::hooks::events::ToolResultData {
                success: true,
                output: "".to_string(),
                exit_code: Some(0),
                duration_ms: 100,
            },
        });
        assert!(!hook.matches(&post_event));
    }

    #[test]
    fn test_hook_matches_with_matcher() {
        let hook = Hook::new("test-hook", HookEventType::PreToolUse)
            .with_matcher(HookMatcher::tool("Bash"));

        let bash_event = make_pre_tool_event("s1", "Bash");
        let read_event = make_pre_tool_event("s1", "Read");

        assert!(hook.matches(&bash_event));
        assert!(!hook.matches(&read_event));
    }

    #[test]
    fn test_hook_result_constructors() {
        let cont = HookResult::continue_();
        assert!(cont.is_continue());
        assert!(!cont.is_block());

        let cont_with = HookResult::continue_with(serde_json::json!({"key": "value"}));
        assert!(cont_with.is_continue());

        let block = HookResult::block("Blocked");
        assert!(block.is_block());
        assert!(!block.is_continue());

        let retry = HookResult::retry(1000);
        assert!(!retry.is_continue());
        assert!(!retry.is_block());

        let skip = HookResult::skip();
        assert!(!skip.is_continue());
        assert!(!skip.is_block());
    }

    #[test]
    fn test_engine_register_unregister() {
        let engine = HookEngine::new();

        let hook = Hook::new("test-hook", HookEventType::PreToolUse);
        engine.register(hook);

        assert_eq!(engine.hook_count(), 1);
        assert!(engine.get_hook("test-hook").is_some());

        let removed = engine.unregister("test-hook");
        assert!(removed.is_some());
        assert_eq!(engine.hook_count(), 0);
    }

    #[test]
    fn test_engine_matching_hooks() {
        let engine = HookEngine::new();

        // Register multiple hooks
        engine.register(
            Hook::new("hook-1", HookEventType::PreToolUse).with_config(HookConfig {
                priority: 10,
                ..Default::default()
            }),
        );
        engine.register(
            Hook::new("hook-2", HookEventType::PreToolUse)
                .with_matcher(HookMatcher::tool("Bash"))
                .with_config(HookConfig {
                    priority: 5,
                    ..Default::default()
                }),
        );
        engine.register(Hook::new("hook-3", HookEventType::PostToolUse));

        let event = make_pre_tool_event("s1", "Bash");
        let matching = engine.matching_hooks(&event);

        // Should match hook-1 and hook-2 (both are PreToolUse)
        assert_eq!(matching.len(), 2);

        // Sorted by priority, hook-2 (priority=5) should be first
        assert_eq!(matching[0].id, "hook-2");
        assert_eq!(matching[1].id, "hook-1");
    }

    #[tokio::test]
    async fn test_engine_fire_no_hooks() {
        let engine = HookEngine::new();
        let event = make_pre_tool_event("s1", "Bash");

        let result = engine.fire(&event).await;
        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn test_engine_fire_no_handler() {
        let engine = HookEngine::new();
        engine.register(Hook::new("test-hook", HookEventType::PreToolUse));

        let event = make_pre_tool_event("s1", "Bash");
        let result = engine.fire(&event).await;

        // No handler, should continue
        assert!(result.is_continue());
    }

    /// Test handler: always continue
    struct ContinueHandler;
    impl HookHandler for ContinueHandler {
        fn handle(&self, _event: &HookEvent) -> HookResponse {
            HookResponse::continue_()
        }
    }

    /// Test handler: always block
    struct BlockHandler {
        reason: String,
    }
    impl HookHandler for BlockHandler {
        fn handle(&self, _event: &HookEvent) -> HookResponse {
            HookResponse::block(&self.reason)
        }
    }

    #[tokio::test]
    async fn test_engine_fire_with_continue_handler() {
        let engine = HookEngine::new();
        engine.register(Hook::new("test-hook", HookEventType::PreToolUse));
        engine.register_handler("test-hook", Arc::new(ContinueHandler));

        let event = make_pre_tool_event("s1", "Bash");
        let result = engine.fire(&event).await;

        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn test_engine_fire_with_block_handler() {
        let engine = HookEngine::new();
        engine.register(Hook::new("test-hook", HookEventType::PreToolUse));
        engine.register_handler(
            "test-hook",
            Arc::new(BlockHandler {
                reason: "Dangerous command".to_string(),
            }),
        );

        let event = make_pre_tool_event("s1", "Bash");
        let result = engine.fire(&event).await;

        assert!(result.is_block());
        if let HookResult::Block(reason) = result {
            assert_eq!(reason, "Dangerous command");
        }
    }

    #[tokio::test]
    async fn test_engine_fire_priority_order() {
        let engine = HookEngine::new();

        // Register two hooks, lower priority one blocks
        engine.register(
            Hook::new("block-hook", HookEventType::PreToolUse).with_config(HookConfig {
                priority: 5, // Higher priority (executes first)
                ..Default::default()
            }),
        );
        engine.register(
            Hook::new("continue-hook", HookEventType::PreToolUse).with_config(HookConfig {
                priority: 10,
                ..Default::default()
            }),
        );

        engine.register_handler(
            "block-hook",
            Arc::new(BlockHandler {
                reason: "Blocked first".to_string(),
            }),
        );
        engine.register_handler("continue-hook", Arc::new(ContinueHandler));

        let event = make_pre_tool_event("s1", "Bash");
        let result = engine.fire(&event).await;

        // block-hook executes first, should block
        assert!(result.is_block());
    }

    #[test]
    fn test_hook_serialization() {
        let hook = Hook::new("test-hook", HookEventType::PreToolUse)
            .with_matcher(HookMatcher::tool("Bash"))
            .with_config(HookConfig {
                priority: 50,
                timeout_ms: 5000,
                async_execution: true,
                max_retries: 3,
            });

        let json = serde_json::to_string(&hook).unwrap();
        assert!(json.contains("test-hook"));
        assert!(json.contains("pre_tool_use"));
        assert!(json.contains("Bash"));

        let parsed: Hook = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-hook");
        assert_eq!(parsed.event_type, HookEventType::PreToolUse);
        assert_eq!(parsed.config.priority, 50);
    }

    #[test]
    fn test_all_hooks() {
        let engine = HookEngine::new();
        engine.register(Hook::new("hook-1", HookEventType::PreToolUse));
        engine.register(Hook::new("hook-2", HookEventType::PostToolUse));

        let all = engine.all_hooks();
        assert_eq!(all.len(), 2);
    }

    fn make_skill_load_event(skill_name: &str, tools: Vec<&str>) -> HookEvent {
        HookEvent::SkillLoad(crate::hooks::events::SkillLoadEvent {
            skill_name: skill_name.to_string(),
            tool_names: tools.iter().map(|s| s.to_string()).collect(),
            version: Some("1.0.0".to_string()),
            description: Some("Test skill".to_string()),
            loaded_at: 1234567890,
        })
    }

    fn make_skill_unload_event(skill_name: &str, tools: Vec<&str>) -> HookEvent {
        HookEvent::SkillUnload(crate::hooks::events::SkillUnloadEvent {
            skill_name: skill_name.to_string(),
            tool_names: tools.iter().map(|s| s.to_string()).collect(),
            duration_ms: 60000,
        })
    }

    #[tokio::test]
    async fn test_engine_fire_skill_load() {
        let engine = HookEngine::new();

        // Register a hook for skill load events
        engine.register(Hook::new("skill-load-hook", HookEventType::SkillLoad));
        engine.register_handler("skill-load-hook", Arc::new(ContinueHandler));

        let event = make_skill_load_event("my-skill", vec!["tool1", "tool2"]);
        let result = engine.fire(&event).await;

        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn test_engine_fire_skill_unload() {
        let engine = HookEngine::new();

        // Register a hook for skill unload events
        engine.register(Hook::new("skill-unload-hook", HookEventType::SkillUnload));
        engine.register_handler("skill-unload-hook", Arc::new(ContinueHandler));

        let event = make_skill_unload_event("my-skill", vec!["tool1", "tool2"]);
        let result = engine.fire(&event).await;

        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn test_engine_skill_hook_with_matcher() {
        let engine = HookEngine::new();

        // Register a hook that only matches specific skill
        engine.register(
            Hook::new("specific-skill-hook", HookEventType::SkillLoad)
                .with_matcher(HookMatcher::skill("my-skill")),
        );
        engine.register_handler(
            "specific-skill-hook",
            Arc::new(BlockHandler {
                reason: "Skill blocked".to_string(),
            }),
        );

        // Should match and block
        let matching_event = make_skill_load_event("my-skill", vec!["tool1"]);
        let result = engine.fire(&matching_event).await;
        assert!(result.is_block());

        // Should not match (no hooks match, so continue)
        let non_matching_event = make_skill_load_event("other-skill", vec!["tool1"]);
        let result = engine.fire(&non_matching_event).await;
        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn test_engine_skill_hook_pattern_matcher() {
        let engine = HookEngine::new();

        // Register a hook with glob pattern
        engine.register(
            Hook::new("test-skill-hook", HookEventType::SkillLoad)
                .with_matcher(HookMatcher::skill("test-*")),
        );
        engine.register_handler(
            "test-skill-hook",
            Arc::new(BlockHandler {
                reason: "Test skill blocked".to_string(),
            }),
        );

        // Should match pattern
        let test_skill = make_skill_load_event("test-alpha", vec!["tool1"]);
        let result = engine.fire(&test_skill).await;
        assert!(result.is_block());

        let test_skill2 = make_skill_load_event("test-beta", vec!["tool1"]);
        let result = engine.fire(&test_skill2).await;
        assert!(result.is_block());

        // Should not match pattern
        let prod_skill = make_skill_load_event("prod-skill", vec!["tool1"]);
        let result = engine.fire(&prod_skill).await;
        assert!(result.is_continue());
    }
}
