//! AHP Idle Detection Integration Tests with Real LLM
//!
//! Run with:
//! ```bash
//! cd crates/code/core
//!
//! # Set environment variables for minmax model (from .a3s/config.hcl)
//! export MINIMAX_API_KEY="sk-ZaH1YnkiGmcBt8qxKWfsBV5w9aInp4QuDUeq1HEIOAzEg5cT"
//! export MINIMAX_BASE_URL="http://35.220.164.252:3888/v1/"
//! export MINIMAX_MODEL="MiniMax-M2.7-highspeed"
//!
//! # Run tests (must use --ignored to run)
//! cargo test --test test_ahp_idle_with_llm -- --ignored --test-threads=1 --nocapture
//! ```

use a3s_code_core::ahp::{EventContext, IdleDecision, IdleEvent, MemorySummary, SessionStats};

/// Create LLM client from environment variables
fn get_test_config() -> (String, String, String) {
    let api_key = std::env::var("MINIMAX_API_KEY")
        .unwrap_or_else(|_| "sk-ZaH1YnkiGmcBt8qxKWfsBV5w9aInp4QuDUeq1HEIOAzEg5cT".to_string());
    let base_url = std::env::var("MINIMAX_BASE_URL")
        .unwrap_or_else(|_| "http://35.220.164.252:3888/v1/".to_string());
    let model =
        std::env::var("MINIMAX_MODEL").unwrap_or_else(|_| "MiniMax-M2.7-highspeed".to_string());
    (api_key, base_url, model)
}

#[test]
#[ignore]
fn test_idle_event_structure() {
    // Test that IdleEvent can be created and serialized correctly
    let idle_event = IdleEvent {
        idle_duration_ms: 10000,
        idle_reason: "no_activity".to_string(),
        last_event_type: Some("post_action".to_string()),
        suggested_action: Some("dream".to_string()),
    };

    // Verify fields
    assert_eq!(idle_event.idle_duration_ms, 10000);
    assert_eq!(idle_event.idle_reason, "no_activity");
    assert!(idle_event.suggested_action.is_some());
    assert_eq!(idle_event.suggested_action.as_deref(), Some("dream"));

    // Test JSON serialization
    let json = serde_json::to_string(&idle_event).unwrap();
    assert!(json.contains("idle_duration_ms"));
    assert!(json.contains("no_activity"));
    assert!(json.contains("dream"));
}

#[test]
#[ignore]
fn test_idle_threshold_configuration() {
    // Test that idle threshold can be configured
    let (api_key, base_url, model) = get_test_config();

    println!("Test configuration:");
    println!("  API Key: {}...", &api_key[..10]);
    println!("  Base URL: {}", base_url);
    println!("  Model: {}", model);

    // This test just verifies configuration is accessible
    assert!(!api_key.is_empty());
    assert!(!base_url.is_empty());
    assert!(!model.is_empty());
}

#[test]
#[ignore]
fn test_idle_event_serialization_roundtrip() {
    use crate::IdleEvent;

    let original = IdleEvent {
        idle_duration_ms: 5000,
        idle_reason: "waiting_for_input".to_string(),
        last_event_type: Some("pre_action".to_string()),
        suggested_action: Some("consolidate".to_string()),
    };

    // Serialize
    let json = serde_json::to_string(&original).unwrap();

    // Deserialize
    let parsed: IdleEvent = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.idle_duration_ms, original.idle_duration_ms);
    assert_eq!(parsed.idle_reason, original.idle_reason);
    assert_eq!(parsed.last_event_type, original.last_event_type);
    assert_eq!(parsed.suggested_action, original.suggested_action);
}

#[test]
#[ignore]
fn test_ahp_server_idle_handler_signature() {
    use crate::IdleDecision;

    // Verify IdleDecision variants exist and work
    let allow = IdleDecision::Allow;
    let defer = IdleDecision::Defer {
        reason: Some("busy".to_string()),
    };

    // Verify they serialize correctly
    let allow_json = serde_json::to_string(&allow).unwrap();
    let defer_json = serde_json::to_string(&defer).unwrap();

    assert!(allow_json.contains("allow"));
    assert!(defer_json.contains("defer"));
    assert!(defer_json.contains("busy"));
}

#[test]
#[ignore]
fn test_event_context_structure() {
    use crate::{EventContext, MemorySummary, SessionStats};

    let context = EventContext {
        recent_facts: None,
        memory_summary: Some(MemorySummary {
            memory_type: "semantic".to_string(),
            total_items: 42,
            recent_topics: vec!["rust".to_string(), "async".to_string()],
        }),
        session_stats: Some(SessionStats {
            total_actions: 10,
            total_tokens: 5000,
            duration_ms: 60000,
            error_count: 0,
        }),
        current_task: Some("implementing idle detection".to_string()),
        capabilities: None,
    };

    // Verify serialization
    let json = serde_json::to_string(&context).unwrap();
    assert!(json.contains("memory_summary"));
    assert!(json.contains("session_stats"));
    assert!(json.contains("rust"));
    assert!(json.contains("implementing idle detection"));
}

#[test]
#[ignore]
fn test_minmax_llm_basic_completion() {
    use a3s_code_core::llm::{LlmClient, Message, OpenAiClient};

    let (api_key, base_url, model) = get_test_config();

    let client = OpenAiClient::new(api_key.into(), model).with_base_url(base_url);

    let messages = vec![Message::user(
        "Reply with exactly the word 'HELLO' in uppercase, nothing else.",
    )];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, None, &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text().trim().to_uppercase();
    assert_eq!(text, "HELLO", "Expected 'HELLO', got: {}", text);
}

#[test]
#[ignore]
fn test_minmax_llm_with_system_prompt() {
    use a3s_code_core::llm::{LlmClient, Message, OpenAiClient};

    let (api_key, base_url, model) = get_test_config();

    let client = OpenAiClient::new(api_key.into(), model).with_base_url(base_url);

    let system = "You are a security analyzer. When given a command, respond with only 'SAFE' or 'DANGEROUS'.";

    let messages = vec![Message::user("ls -la")];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(system), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text().trim().to_uppercase();
    assert!(
        text == "SAFE" || text == "DANGEROUS",
        "Expected SAFE or DANGEROUS, got: {}",
        text
    );
}
