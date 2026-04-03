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
    AgentStyle, SystemPromptSlots, AGENT_VERIFICATION, PROMPT_SUGGESTION, SESSION_MEMORY_TEMPLATE,
    SUBAGENT_EXPLORE, SUBAGENT_PLAN, UNDERCOVER_INSTRUCTIONS,
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

    // Should have sections (flexible matching for template variations)
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("session") && text_lower.contains("title")
            || text_lower.contains("current") && text_lower.contains("state"),
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

// =============================================================================
// AgentStyle Integration Tests
// =============================================================================

#[test]
#[ignore]
fn test_agent_style_detection_plan_message() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    // Message contains "plan" -> should trigger Plan style
    let slots = SystemPromptSlots::default();
    let prompt = slots.build_with_message("Help me plan a new feature");

    println!(
        "[test_agent_style_detection_plan_message] Prompt:\n{}",
        prompt
    );

    let messages = vec![Message::user("What is your role? Answer in one sentence.")];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(&prompt), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_agent_style_detection_plan_message] Response: {}",
        text
    );

    // Should behave like a planning agent
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("plan") || text_lower.contains("analysis"),
        "Should respond as planning agent, got: {}",
        text
    );
}

#[test]
#[ignore]
fn test_agent_style_detection_explore_message() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    // Message contains "find" -> should trigger Explore style
    let slots = SystemPromptSlots::default();
    let prompt = slots.build_with_message("Find all files related to auth");

    println!(
        "[test_agent_style_detection_explore_message] Prompt:\n{}",
        prompt
    );

    let messages = vec![Message::user("What tools can you use? List them.")];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(&prompt), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_agent_style_detection_explore_message] Response: {}",
        text
    );

    // Should behave like an exploration agent
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("read") || text_lower.contains("grep") || text_lower.contains("glob"),
        "Should mention read-only tools, got: {}",
        text
    );
}

#[test]
#[ignore]
fn test_agent_style_detection_verification_message() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    // Message contains "verify" -> should trigger Verification style
    let slots = SystemPromptSlots::default();
    let prompt = slots.build_with_message("Verify that this code works correctly");

    println!(
        "[test_agent_style_detection_verification_message] Prompt:\n{}",
        prompt
    );

    let messages = vec![Message::user("What is your job? Answer in one sentence.")];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(&prompt), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_agent_style_detection_verification_message] Response: {}",
        text
    );

    // Should behave like a verification agent
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("verification")
            || text_lower.contains("break")
            || text_lower.contains("adversarial"),
        "Should respond as verification agent, got: {}",
        text
    );
}

#[test]
#[ignore]
fn test_agent_style_explicit_overrides_detection() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    // Even though message says "verify", we explicitly set Plan style
    let slots = SystemPromptSlots {
        style: Some(AgentStyle::Plan),
        ..Default::default()
    };
    let prompt = slots.build_with_message("Verify this code");

    println!(
        "[test_agent_style_explicit_overrides_detection] Prompt:\n{}",
        prompt
    );

    let messages = vec![Message::user("What is your role? Answer in one sentence.")];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(&prompt), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_agent_style_explicit_overrides_detection] Response: {}",
        text
    );

    // Should behave like a planning agent (not verification)
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("plan") || text_lower.contains("architecture"),
        "Should respond as planning agent despite verify keyword, got: {}",
        text
    );
}

#[test]
#[ignore]
fn test_agent_style_code_review_includes_guidelines() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let slots = SystemPromptSlots {
        style: Some(AgentStyle::CodeReview),
        ..Default::default()
    };
    let prompt = slots.build();

    println!(
        "[test_agent_style_code_review_includes_guidelines] Prompt:\n{}",
        prompt
    );

    // Verify the prompt contains code review guidelines (case-insensitive)
    let prompt_lower = prompt.to_lowercase();
    assert!(
        prompt_lower.contains("correctness") && prompt_lower.contains("security"),
        "Code review style should include guidelines"
    );

    let messages = vec![Message::user(
        "Review this code snippet:\nfn add(a: i32, b: i32) -> i32 { a + b }",
    )];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(client.complete(&messages, Some(&prompt), &[]));

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    println!(
        "[test_agent_style_code_review_includes_guidelines] Response:\n{}",
        text
    );

    // Should give a code review
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("correct") || text_lower.contains("review"),
        "Should provide code review, got: {}",
        text
    );
}

// =============================================================================
// LLM-Based Intent Classification Tests
// =============================================================================

#[test]
#[ignore]
fn test_llm_classify_intent_plan() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let style = rt.block_on(AgentStyle::detect_with_llm(
        &client as &dyn LlmClient,
        "Help me plan a new feature",
    ));

    assert!(
        style.is_ok(),
        "LLM classification failed: {:?}",
        style.err()
    );
    assert_eq!(style.unwrap(), AgentStyle::Plan);
    println!("[test_llm_classify_intent_plan] Classified as: Plan");
}

#[test]
#[ignore]
fn test_llm_classify_intent_explore() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let style = rt.block_on(AgentStyle::detect_with_llm(
        &client as &dyn LlmClient,
        "Where is the login function defined?",
    ));

    assert!(
        style.is_ok(),
        "LLM classification failed: {:?}",
        style.err()
    );
    assert_eq!(style.unwrap(), AgentStyle::Explore);
    println!("[test_llm_classify_intent_explore] Classified as: Explore");
}

#[test]
#[ignore]
fn test_llm_classify_intent_verification() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let style = rt.block_on(AgentStyle::detect_with_llm(
        &client as &dyn LlmClient,
        "Check if the API handles edge cases correctly",
    ));

    assert!(
        style.is_ok(),
        "LLM classification failed: {:?}",
        style.err()
    );
    assert_eq!(style.unwrap(), AgentStyle::Verification);
    println!("[test_llm_classify_intent_verification] Classified as: Verification");
}

#[test]
#[ignore]
fn test_llm_classify_intent_code_review() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let style = rt.block_on(AgentStyle::detect_with_llm(
        &client as &dyn LlmClient,
        "Look at this code and tell me if there are any issues",
    ));

    assert!(
        style.is_ok(),
        "LLM classification failed: {:?}",
        style.err()
    );
    assert_eq!(style.unwrap(), AgentStyle::CodeReview);
    println!("[test_llm_classify_intent_code_review] Classified as: CodeReview");
}

#[test]
#[ignore]
fn test_llm_classify_intent_general_purpose() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let style = rt.block_on(AgentStyle::detect_with_llm(
        &client as &dyn LlmClient,
        "Implement a REST API endpoint for user registration",
    ));

    assert!(
        style.is_ok(),
        "LLM classification failed: {:?}",
        style.err()
    );
    assert_eq!(style.unwrap(), AgentStyle::GeneralPurpose);
    println!("[test_llm_classify_intent_general_purpose] Classified as: GeneralPurpose");
}

#[test]
#[ignore]
fn test_llm_classify_intent_ambiguous_message() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    // Ambiguous message - could be plan or general purpose
    let rt = tokio::runtime::Runtime::new().unwrap();
    let style = rt.block_on(AgentStyle::detect_with_llm(
        &client as &dyn LlmClient,
        "We should probably look into fixing that performance issue",
    ));

    assert!(
        style.is_ok(),
        "LLM classification failed: {:?}",
        style.err()
    );
    // Either Plan or GeneralPurpose is acceptable for this ambiguous message
    let result = style.unwrap();
    assert!(
        result == AgentStyle::Plan || result == AgentStyle::GeneralPurpose,
        "Expected Plan or GeneralPurpose for ambiguous message, got {:?}",
        result
    );
    println!(
        "[test_llm_classify_intent_ambiguous_message] Classified as: {:?}",
        result
    );
}

#[test]
#[ignore]
fn test_llm_classify_intent_planning_mode_auto() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    // Low-confidence message: "build a new feature"
    // Keywords don't match any pattern -> Low confidence -> should use LLM
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Verify that for a low-confidence message, LLM classification returns a valid style
    let style = rt.block_on(AgentStyle::detect_with_llm(
        &client as &dyn LlmClient,
        "Build me a shopping cart module",
    ));

    assert!(
        style.is_ok(),
        "LLM classification failed: {:?}",
        style.err()
    );
    let result = style.unwrap();
    println!(
        "[test_llm_classify_intent_planning_mode_auto] Low-confidence message 'Build me a shopping cart module' -> {:?}",
        result
    );
}

#[test]
#[ignore]
fn test_intent_classification_prompt_is_lightweight() {
    let client = create_minimax_client().expect("MINIMAX_API_KEY not set");

    let rt = tokio::runtime::Runtime::new().unwrap();

    // The intent classification prompt should return a single word
    let messages = vec![a3s_code_core::llm::Message::user(
        "I want to add OAuth2 support to my app",
    )];
    let system = a3s_code_core::INTENT_CLASSIFY_SYSTEM;

    let start = std::time::Instant::now();
    let response = rt.block_on(client.complete(&messages, Some(system), &[]));
    let elapsed = start.elapsed();

    assert!(response.is_ok(), "LLM call failed: {:?}", response.err());
    let text = response.unwrap().text();
    let trimmed = text.trim();

    println!(
        "[test_intent_classification_prompt_is_lightweight] Response: '{}' (took {:?})",
        trimmed, elapsed
    );

    // Should be a single word (the intent category)
    let word_count = trimmed.split_whitespace().count();
    assert!(
        word_count <= 2,
        "Intent classification should return single word, got {} words: '{}'",
        word_count,
        trimmed
    );
}
