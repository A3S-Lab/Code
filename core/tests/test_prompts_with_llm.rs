//! Prompt Integration Tests with Real LLM
//!
//! Run with:
//! ```bash
//! cd crates/code/core
//!
//! # Set environment variables for minmax model
//! export MINIMAX_API_KEY="sk-ZaH1YnkiGmcBt8qxKWfsBV5w9aInp4QuDUeq1HEIOAzEg5cT"
//! export MINIMAX_BASE_URL="http://35.220.164.252:3888/v1/"
//! export MINIMAX_MODEL="MiniMax-M2.7-highspeed"
//!
//! # Run tests (must use --ignored to run)
//! cargo test --test test_prompts_with_llm -- --ignored --test-threads=1 --nocapture
//! ```

use a3s_code_core::llm::{LlmClient, Message, OpenAiClient};
use a3s_code_core::{
    AGENT_VERIFICATION, PROMPT_SUGGESTION, SESSION_MEMORY_TEMPLATE, SUBAGENT_EXPLORE,
    SUBAGENT_PLAN, UNDERCOVER_INSTRUCTIONS,
};

/// Create LLM client from environment variables
fn create_minimax_client() -> Option<OpenAiClient> {
    let api_key = std::env::var("MINIMAX_API_KEY").ok()?;
    let base_url = std::env::var("MINIMAX_BASE_URL")
        .unwrap_or_else(|_| "http://35.220.164.252:3888/v1/".to_string());
    let model =
        std::env::var("MINIMAX_MODEL").unwrap_or_else(|_| "MiniMax-M2.7-highspeed".to_string());

    Some(OpenAiClient::new(api_key.into(), model).with_base_url(base_url))
}

#[test]
#[ignore]
fn test_verification_prompt_knows_its_role() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let messages = vec![Message::user("What is your role? Keep it to one sentence.")];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(AGENT_VERIFICATION), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_verification_prompt_knows_its_role] Response: {}",
        text
    );

    // Should mention verification, breaking, or adversarial
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("verification")
            || text_lower.contains("break")
            || text_lower.contains("adversarial"),
        "Response should mention verification role, got: {}",
        text
    );
}

#[test]
#[ignore]
fn test_undercover_instructions_sanitizes_output() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let messages = vec![Message::user(
        "Write a commit message for fixing a bug. Include Co-Authored-By if appropriate.",
    )];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(UNDERCOVER_INSTRUCTIONS), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_undercover_instructions_sanitizes_output] Response: {}",
        text
    );

    // Extract first code block content to check the actual commit message
    // The commit message is typically in the first ``` block
    let first_code_block = text
        .lines()
        .skip_while(|l| !l.trim().starts_with("```"))
        .skip(1)
        .take_while(|l| !l.trim().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");

    // Should NOT contain Co-Authored-By in the actual commit message
    assert!(
        !first_code_block.to_lowercase().contains("co-authored-by"),
        "Commit message should not contain Co-Authored-By, got: {}",
        first_code_block
    );
}

#[test]
#[ignore]
fn test_session_memory_template_has_sections() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let messages = vec![
        Message::user("Create a session memory entry for a user asking to build a REST API with FastAPI. Fill in realistic content."),
    ];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(SESSION_MEMORY_TEMPLATE), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_session_memory_template_has_sections] Response:\n{}",
        text
    );

    // Should have sections
    assert!(
        text.contains("Session Title") || text.contains("Current State"),
        "Should include required sections, got: {}",
        text
    );
}

#[test]
#[ignore]
fn test_prompt_suggestion_is_concise() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let messages = vec![Message::user(
        "User just ran 'cargo build' and it succeeded. What would they naturally type next?",
    )];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(PROMPT_SUGGESTION), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!("[test_prompt_suggestion_is_concise] Response: '{}'", text);

    // Should be short (2-12 words)
    let words: Vec<&str> = text.split_whitespace().collect();
    assert!(
        words.len() <= 15,
        "Suggestion should be short, got {} words: '{}'",
        words.len(),
        text
    );
}

#[test]
#[ignore]
fn test_explore_prompt_uses_read_only_tools() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let messages = vec![Message::user("What tools can you use? List them.")];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(SUBAGENT_EXPLORE), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_explore_prompt_uses_read_only_tools] Response:\n{}",
        text
    );

    // Should mention read, grep, glob (read-only tools)
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("read") || text_lower.contains("grep") || text_lower.contains("glob"),
        "Should mention read-only tools, got: {}",
        text
    );
}

#[test]
#[ignore]
fn test_plan_prompt_analyzes_before_implementing() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let messages = vec![Message::user(
        "User wants to add user authentication to their app. What should they consider?",
    )];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(SUBAGENT_PLAN), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_plan_prompt_analyzes_before_implementing] Response:\n{}",
        text
    );

    // Should show analysis/planning behavior
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("consider")
            || text_lower.contains("analyze")
            || text_lower.contains("approach")
            || text_lower.contains("plan"),
        "Should show planning mindset, got: {}",
        text
    );
}
