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
    let hook =
        Hook::new("test-hook", HookEventType::PreToolUse).with_matcher(HookMatcher::tool("Bash"));

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
    let hook =
        Hook::new("test-hook", HookEventType::PreToolUse).with_matcher(HookMatcher::tool("Bash"));

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

struct ErrorHandler;

impl HookHandler for ErrorHandler {
    fn handle(&self, _event: &HookEvent) -> HookResponse {
        HookResponse::continue_()
    }

    fn try_handle(&self, _event: &HookEvent) -> Result<HookResponse, String> {
        Err("callback failed".to_string())
    }
}

struct PanicHandler;

impl HookHandler for PanicHandler {
    fn handle(&self, _event: &HookEvent) -> HookResponse {
        panic!("callback panicked")
    }
}

struct SlowBlockHandler;

impl HookHandler for SlowBlockHandler {
    fn handle(&self, _event: &HookEvent) -> HookResponse {
        std::thread::sleep(std::time::Duration::from_millis(80));
        HookResponse::block("late block")
    }
}

fn make_post_tool_event() -> HookEvent {
    HookEvent::PostToolUse(crate::hooks::events::PostToolUseEvent {
        session_id: "s1".to_string(),
        tool: "Bash".to_string(),
        args: serde_json::json!({}),
        result: crate::hooks::events::ToolResultData {
            success: true,
            output: String::new(),
            exit_code: Some(0),
            duration_ms: 1,
        },
    })
}

#[tokio::test]
async fn handler_error_is_scoped_to_gate_role() {
    let gate = HookEngine::new();
    gate.register(Hook::new("gate", HookEventType::PreToolUse));
    gate.register_handler("gate", Arc::new(ErrorHandler));
    let result = gate.fire(&make_pre_tool_event("s1", "Bash")).await;
    assert!(matches!(result, HookResult::Block(reason) if reason.contains("callback failed")));

    let observer = HookEngine::new();
    observer.register(Hook::new("observer", HookEventType::PostToolUse));
    observer.register_handler("observer", Arc::new(ErrorHandler));
    assert!(observer.fire(&make_post_tool_event()).await.is_continue());
}

#[tokio::test]
async fn pre_tool_handler_panic_fails_closed_before_tool_side_effect() {
    let engine = HookEngine::new();
    engine.register(Hook::new("panic-gate", HookEventType::PreToolUse));
    engine.register_handler("panic-gate", Arc::new(PanicHandler));

    let result = engine.fire(&make_pre_tool_event("s1", "Bash")).await;
    assert!(
        matches!(result, HookResult::Block(reason) if reason.contains("terminated unexpectedly"))
    );
}

#[tokio::test]
async fn pre_tool_timeout_fails_closed_before_tool_side_effect() {
    let engine = HookEngine::new();
    engine.register(
        Hook::new("slow-gate", HookEventType::PreToolUse).with_config(HookConfig {
            timeout_ms: 5,
            ..Default::default()
        }),
    );
    engine.register_handler("slow-gate", Arc::new(SlowBlockHandler));

    let result = engine.fire(&make_pre_tool_event("s1", "Bash")).await;
    assert!(matches!(result, HookResult::Block(reason) if reason.contains("timed out")));
}

#[tokio::test]
async fn async_pre_tool_config_still_awaits_gate_before_tool_side_effect() {
    let engine = HookEngine::new();
    engine.register(
        Hook::new("async-gate", HookEventType::PreToolUse).with_config(HookConfig {
            async_execution: true,
            ..Default::default()
        }),
    );
    engine.register_handler(
        "async-gate",
        Arc::new(BlockHandler {
            reason: "blocked synchronously".to_string(),
        }),
    );

    let result = engine.fire(&make_pre_tool_event("s1", "Bash")).await;
    assert!(matches!(result, HookResult::Block(reason) if reason == "blocked synchronously"));
}

#[tokio::test]
async fn test_engine_observational_hook_without_handler_is_best_effort() {
    let engine = HookEngine::new();
    engine.register(
        Hook::new("observer", HookEventType::PostToolUse).with_config(HookConfig {
            async_execution: true,
            ..Default::default()
        }),
    );

    assert!(engine.fire(&make_post_tool_event()).await.is_continue());
}
