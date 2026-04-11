//! Context Perception Integration Tests with Real LLM (Minimax)
//!
//! Run with:
//! ```bash
//! cd crates/code/core
//! export MINIMAX_API_KEY="your-api-key"
//! export MINIMAX_BASE_URL="https://your-endpoint/v1/"  # optional
//! export MINIMAX_MODEL="MiniMax-M2.7-highspeed"  # optional
//! cargo test --features ahp --test test_context_perception_with_llm -- --ignored --test-threads=1 --nocapture
//! ```

#![cfg(feature = "ahp")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use a3s_code_core::agent::{AgentConfig, AgentLoop};
use a3s_code_core::ahp::{AhpHookExecutor, AhpTransport};
use a3s_code_core::llm::OpenAiClient;
use a3s_code_core::tools::ToolExecutor;

/// Create a ToolContext for tests
fn test_tool_context() -> a3s_code_core::tools::ToolContext {
    a3s_code_core::tools::ToolContext::new(PathBuf::from("/tmp"))
}

/// Get test config from environment variables
fn get_test_config() -> (String, String, String) {
    let api_key =
        std::env::var("MINIMAX_API_KEY").expect("MINIMAX_API_KEY environment variable not set");
    let base_url = std::env::var("MINIMAX_BASE_URL")
        .unwrap_or_else(|_| "https://api.minimax.io/v1/".to_string());
    let model =
        std::env::var("MINIMAX_MODEL").unwrap_or_else(|_| "MiniMax-M2.7-highspeed".to_string());
    (api_key, base_url, model)
}

/// Test: Benchmark intent detection performance
/// This test doesn't need a real LLM - it just tests the intent detection logic
#[test]
#[ignore]
fn test_intent_detection_performance() {
    // We need to create an AgentLoop, but we can use a dummy LLM client
    // For testing intent detection, we just need any LLM client
    let (api_key, base_url, model) = get_test_config();
    let client = OpenAiClient::new(api_key.into(), model).with_base_url(base_url);
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let config = AgentConfig::default();

    let agent = AgentLoop::new(Arc::new(client), tool_executor, test_tool_context(), config);

    let prompt = "Where is the main function in auth.rs? Explain how the login works and verify the test cases.";

    // Warm up
    for _ in 0..100 {
        let _ = agent.detect_context_perception_intent(prompt, "test", "/workspace");
    }

    // Benchmark
    let iterations = 1000;
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = agent.detect_context_perception_intent(prompt, "test", "/workspace");
    }
    let elapsed = start.elapsed();

    println!("\n=== Intent Detection Performance ===");
    println!("Prompt: {}", prompt);
    println!("Iterations: {}", iterations);
    println!("Total time: {:?}", elapsed);
    println!(
        "Average: {:.3} µs/op",
        elapsed.as_micros() as f64 / iterations as f64
    );
}

/// Test: Full agent with context perception (requires AHP harness server)
#[test]
#[ignore]
fn test_context_perception_with_ahp_harness() {
    let (api_key, base_url, model) = get_test_config();
    let client = OpenAiClient::new(api_key.into(), model).with_base_url(base_url);
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create AHP executor with stdio transport
    let rt = tokio::runtime::Runtime::new().unwrap();

    println!("Creating AHP transport...");
    let transport = AhpTransport::Stdio {
        program: "echo".to_string(),
        args: vec![],
    };
    let transport_result: Result<AhpTransport, Box<dyn std::error::Error + Send + Sync>> =
        Ok(transport);

    let hook_engine: Option<Arc<dyn a3s_code_core::hooks::HookExecutor>> = match transport_result {
        Ok(transport) => {
            println!("Transport created, connecting to harness...");
            match rt.block_on(AhpHookExecutor::new_with_config(transport, 5000)) {
                Ok(ahp) => {
                    println!("Connected to harness!");
                    Some(Arc::new(ahp) as Arc<dyn a3s_code_core::hooks::HookExecutor>)
                }
                Err(e) => {
                    println!("Failed to create AHP executor: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            println!("Failed to create transport: {}", e);
            None
        }
    };

    let has_harness = hook_engine.is_some();

    let config = AgentConfig {
        hook_engine,
        ..Default::default()
    };

    let agent = AgentLoop::new(Arc::new(client), tool_executor, test_tool_context(), config);

    // Run a prompt that triggers context perception
    let prompt = "Where is the main function defined? Explain how the auth module works.";

    println!("\n=== Context Perception Test ===");
    println!("Prompt: {}", prompt);

    // Test intent detection first
    let intent = agent.detect_context_perception_intent(prompt, "test-session", "/workspace");
    println!("Detected intent: {:?}", intent.map(|i| i.intent));

    // If we have a harness, run the full agent
    if has_harness {
        println!("Running full agent with harness...");
        let start = Instant::now();
        let result = rt.block_on(agent.execute_with_session(&[], prompt, Some("test"), None, None));
        let elapsed = start.elapsed();

        match result {
            Ok(r) => {
                let preview = if r.text.len() > 200 {
                    format!("{}...", &r.text[..200])
                } else {
                    r.text.clone()
                };
                println!("Success!");
                println!("Response: {}", preview);
                println!("Tokens: {:?}", r.usage);
                println!("Total time: {:?}", elapsed);
            }
            Err(e) => println!("Error: {}", e),
        }
    } else {
        println!("No harness configured, skipping full test");
    }
}

/// Test: Compare with and without context providers (performance)
#[test]
#[ignore]
fn test_performance_comparison() {
    let (api_key, base_url, model) = get_test_config();
    let client1 =
        OpenAiClient::new(api_key.clone().into(), model.clone()).with_base_url(base_url.clone());
    let _client2 = OpenAiClient::new(api_key.into(), model).with_base_url(base_url);
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let prompt = "Explain the codebase structure of this project.";

    // Without context providers
    let config_no_ctx = AgentConfig::default();

    let agent_no_ctx = AgentLoop::new(
        Arc::new(client1),
        tool_executor.clone(),
        test_tool_context(),
        config_no_ctx,
    );

    let rt = tokio::runtime::Runtime::new().unwrap();

    println!("\n=== Performance Comparison ===");
    println!("Prompt: {}", prompt);
    println!("\nRunning WITHOUT context providers...");

    let start = Instant::now();
    let result =
        rt.block_on(agent_no_ctx.execute_with_session(&[], prompt, Some("test"), None, None));
    let without_time = start.elapsed();

    match result {
        Ok(r) => {
            let preview = if r.text.len() > 100 {
                format!("{}...", &r.text[..100])
            } else {
                r.text.clone()
            };
            println!("Time: {:?}", without_time);
            println!("Tokens: {:?}", r.usage);
            println!("Response preview: {}", preview);
        }
        Err(e) => println!("Error: {}", e),
    }
}

/// Test: Verify all intent types are detected correctly
#[test]
#[ignore]
fn test_all_intent_types() {
    let (api_key, base_url, model) = get_test_config();
    let client = OpenAiClient::new(api_key.into(), model).with_base_url(base_url);
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let agent = AgentLoop::new(Arc::new(client), tool_executor, test_tool_context(), config);

    let test_cases = vec![
        // Locate
        ("Where is the main function?", "locate"),
        ("Find all files related to auth", "locate"),
        ("Locate the config file", "locate"),
        // Understand
        ("How does authentication work?", "understand"),
        ("What does this code do?", "understand"),
        ("Explain the login flow", "understand"),
        // Retrieve
        ("Remember what we discussed earlier?", "retrieve"),
        ("What was the previous approach?", "retrieve"),
        // Explore
        ("What files are in this project?", "explore"),
        ("Show me the project structure", "explore"),
        // Reason
        ("Why did the build fail?", "reason"),
        ("Why is this code structured this way?", "reason"),
        // Validate
        ("Verify this code is correct", "validate"),
        ("Check if the tests pass", "validate"),
        // Compare
        ("What's the difference between A and B?", "compare"),
        ("Compare these two approaches", "compare"),
        // Track
        ("Show me the status of the task", "track"),
        ("What's the progress?", "track"),
    ];

    println!("\n=== Intent Detection Test ===");
    let mut passed = 0;
    let mut failed = 0;

    for (prompt, expected) in test_cases {
        let intent = agent.detect_context_perception_intent(prompt, "test-session", "/workspace");
        match intent {
            Some(i) if i.intent == expected => {
                println!("OK: '{}' -> {}", &prompt[..prompt.len().min(40)], expected);
                passed += 1;
            }
            Some(i) => {
                println!(
                    "FAIL: '{}' -> expected '{}', got '{}'",
                    &prompt[..prompt.len().min(40)],
                    expected,
                    i.intent
                );
                failed += 1;
            }
            None => {
                println!(
                    "FAIL: '{}' -> expected '{}', got None",
                    &prompt[..prompt.len().min(40)],
                    expected
                );
                failed += 1;
            }
        }
    }

    println!("\nResults: {} passed, {} failed", passed, failed);
    assert_eq!(failed, 0, "Some intent detection tests failed");
}
