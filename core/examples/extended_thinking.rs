//! Extended Thinking Example
//!
//! Demonstrates how to use Claude's extended thinking feature (Anthropic-specific).
//!
//! Extended thinking allows the model to "think" before responding, using a separate
//! token budget for internal reasoning. This can improve response quality for complex tasks.
//!
//! ## Usage
//!
//! ```bash
//! export ANTHROPIC_API_KEY=sk-ant-...
//! cargo run --example extended_thinking
//! ```

use a3s_code_core::llm::{AnthropicClient, LlmClient, Message};
use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Get API key from environment
    let api_key =
        env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY environment variable must be set");

    // Create client with extended thinking enabled
    let client =
        AnthropicClient::new(api_key, "claude-opus-4-6".to_string()).with_thinking_budget(10_000); // 10k tokens for thinking

    println!("🧠 Extended Thinking Example");
    println!("============================\n");
    println!("Model: claude-opus-4-6");
    println!("Thinking budget: 10,000 tokens\n");

    // Example 1: Complex reasoning task
    println!("Example 1: Complex reasoning");
    println!("----------------------------");
    let messages = vec![Message::user(
        "Explain the halting problem in computer science. \
         Think carefully about the proof and its implications.",
    )];

    let response = client.complete(&messages, None, &[]).await?;
    println!("Response: {}\n", response.message.text());
    println!("Token usage: {:?}\n", response.usage);

    // Example 2: Multi-step problem solving
    println!("Example 2: Multi-step problem");
    println!("------------------------------");
    let messages = vec![Message::user(
        "A farmer has 17 sheep. All but 9 die. How many are left? \
         Think through this step by step.",
    )];

    let response = client.complete(&messages, None, &[]).await?;
    println!("Response: {}\n", response.message.text());
    println!("Token usage: {:?}\n", response.usage);

    println!("✅ Extended thinking examples completed");

    Ok(())
}
