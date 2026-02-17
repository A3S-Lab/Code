//! LLM chat example — sends a prompt and streams the response.
//!
//! Demonstrates:
//! - Loading config from `.a3s/config.hcl`
//! - Creating an agent via `Agent::create()`
//! - Non-streaming `session.send()` call
//! - Streaming `session.stream()` with event handling
//! - Model override via `SessionOptions`
//! - Conversation history: auto-accumulated & explicit query
//! - Multi-turn conversation reusing session history
//! - Custom history override via `send(prompt, Some(&history))`
//!
//! Requires network access and valid API keys in config.
//!
//! ## Usage
//!
//! ```bash
//! # From the repo root (a3s/)
//! cd crates/code && cargo run --example sdk_chat
//!
//! # With a custom config
//! A3S_CONFIG=/path/to/config.hcl cargo run --example sdk_chat
//!
//! # With a custom prompt
//! cd crates/code && cargo run --example sdk_chat -- "Explain Rust's ownership model in 3 sentences"
//! ```

use a3s_code_core::{Agent, AgentEvent, ContentBlock, Message, SessionOptions};
use std::path::PathBuf;

fn resolve_config() -> PathBuf {
    if let Ok(env_path) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(env_path);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("failed to resolve repo root")
        .join(".a3s/config.hcl")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse optional prompt from CLI args
    let args: Vec<String> = std::env::args().collect();
    let prompt = if args.len() > 1 {
        args[1..].join(" ")
    } else {
        "What is 2+2? Reply with just the number and nothing else.".to_string()
    };

    let config_path = resolve_config();
    println!("=== A3S Code SDK Chat Example ===\n");
    println!("Config: {}", config_path.display());
    println!("Prompt: {}\n", prompt);

    // ── 1. Create agent via Agent::create() ──────────────────────────────
    let agent = Agent::create(config_path.display().to_string()).await?;
    println!("[ok] Agent created\n");

    // ── 2. Non-streaming call ────────────────────────────────────────────
    println!("--- Non-streaming (session.send) ---");
    let tmp = tempfile::tempdir()?;
    let session = agent.session(tmp.path().display().to_string(), None)?;

    let result = session.send(&prompt, None).await?;
    println!("  Response: {}", result.text.trim());
    println!(
        "  Tokens: {} in / {} out",
        result.usage.prompt_tokens, result.usage.completion_tokens
    );

    // ── 3. Streaming call ────────────────────────────────────────────────
    println!("\n--- Streaming (session.stream) ---");
    let tmp2 = tempfile::tempdir()?;
    let session2 = agent.session(tmp2.path().display().to_string(), None)?;

    let (mut rx, handle) = session2.stream(&prompt, None).await?;

    print!("  Response: ");
    let mut collected = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::TextDelta { text } => {
                print!("{}", text);
                collected.push_str(&text);
            }
            AgentEvent::End { usage, .. } => {
                println!();
                println!(
                    "  Tokens: {} in / {} out",
                    usage.prompt_tokens, usage.completion_tokens
                );
                break;
            }
            _ => {}
        }
    }
    handle.abort();

    // ── 4. History API — check auto-accumulated history ──────────────────
    println!("\n--- History API (auto-accumulated) ---");
    let history = session.history();
    println!("  Messages after send(): {}", history.len());
    for (i, msg) in history.iter().enumerate() {
        let preview = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => {
                    let trimmed = text.trim();
                    if trimmed.len() > 60 {
                        Some(format!("{}...", &trimmed[..60]))
                    } else {
                        Some(trimmed.to_string())
                    }
                }
                ContentBlock::ToolUse { name, .. } => Some(format!("[tool_use: {}]", name)),
                ContentBlock::ToolResult { content, .. } => {
                    Some(format!("[tool_result: {}]", &content[..content.len().min(40)]))
                }
            })
            .collect::<Vec<_>>()
            .join(" | ");
        println!("  [{}] {}: {}", i, msg.role, preview);
    }

    // ── 5. Multi-turn conversation (auto-accumulated) ─────────────────────
    println!("\n--- Multi-turn conversation ---");
    let follow_up = "Now multiply that result by 10. Reply with just the number.";
    println!("  Follow-up: {}", follow_up);
    let result2 = session.send(follow_up, None).await?;
    println!("  Response: {}", result2.text.trim());
    println!(
        "  Tokens: {} in / {} out",
        result2.usage.prompt_tokens, result2.usage.completion_tokens
    );

    let history2 = session.history();
    println!("  Messages after 2 turns: {}", history2.len());

    // ── 6. Custom history override ────────────────────────────────────────
    println!("\n--- Custom history override ---");
    let custom_history = vec![
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "Remember: the secret number is 42.".to_string(),
            }],
            reasoning_content: None,
        },
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Got it, the secret number is 42.".to_string(),
            }],
            reasoning_content: None,
        },
    ];
    let custom_prompt = "What is the secret number? Reply with just the number.";
    println!("  Custom history: {} messages injected", custom_history.len());
    println!("  Prompt: {}", custom_prompt);
    let result3 = session.send(custom_prompt, Some(&custom_history)).await?;
    println!("  Response: {}", result3.text.trim());

    // Verify internal history was NOT modified by the custom-history call
    let history3 = session.history();
    println!(
        "  Internal history unchanged: {} messages (same as before custom call: {})",
        history3.len(),
        history2.len()
    );

    // ── 7. Model override session ─────────────────────────────────────────
    println!("\n--- Session with model override ---");
    let tmp3 = tempfile::tempdir()?;
    let opts = SessionOptions::new().with_model("anthropic/claude-sonnet-4-20250514");
    match agent.session(tmp3.path().display().to_string(), Some(opts)) {
        Ok(override_session) => {
            println!("[ok] Session with anthropic/claude-sonnet-4-20250514 created");
            match override_session.send(&prompt, None).await {
                Ok(result) => {
                    println!("  Response: {}", result.text.trim());
                    println!(
                        "  Tokens: {} in / {} out",
                        result.usage.prompt_tokens, result.usage.completion_tokens
                    );
                }
                Err(e) => {
                    println!("[skip] LLM call failed (rate limit / API error): {}", e);
                }
            }
        }
        Err(e) => {
            println!(
                "[skip] Model override failed (expected if model not in config): {}",
                e
            );
        }
    }

    // ── 8. Per-session skill_dirs / agent_dirs ────────────────────────────
    println!("\n--- Session with per-session skill_dirs ---");
    let tmp4 = tempfile::tempdir()?;
    let opts2 = SessionOptions::new()
        .with_skill_dir("/tmp/my-skills")
        .with_agent_dir("/tmp/my-agents");
    match agent.session(tmp4.path().display().to_string(), Some(opts2)) {
        Ok(_skill_session) => {
            println!("[ok] Session created with custom skill_dirs and agent_dirs");
        }
        Err(e) => {
            println!("[skip] Session with custom dirs failed: {}", e);
        }
    }

    println!("\n=== Done ===");
    Ok(())
}
