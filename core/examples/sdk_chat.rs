//! LLM chat example — sends a prompt and streams the response.
//!
//! Demonstrates:
//! - Loading config from `.a3s/config.hcl`
//! - Creating an agent via `Agent::create()`
//! - Non-streaming `session.send()` call
//! - Streaming `session.stream()` with event handling
//! - Model override via `SessionOptions`
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

use a3s_code_core::{Agent, AgentEvent, SessionOptions};
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

    let result = session.send(&prompt).await?;
    println!("  Response: {}", result.text.trim());
    println!(
        "  Tokens: {} in / {} out",
        result.usage.prompt_tokens, result.usage.completion_tokens
    );

    // ── 3. Streaming call ────────────────────────────────────────────────
    println!("\n--- Streaming (session.stream) ---");
    let tmp2 = tempfile::tempdir()?;
    let session2 = agent.session(tmp2.path().display().to_string(), None)?;

    let (mut rx, handle) = session2.stream(&prompt).await?;

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

    // ── 4. Model override session ────────────────────────────────────────
    println!("\n--- Session with model override ---");
    let tmp3 = tempfile::tempdir()?;
    let opts = SessionOptions::new().with_model("anthropic/claude-sonnet-4-20250514");
    match agent.session(tmp3.path().display().to_string(), Some(opts)) {
        Ok(override_session) => {
            println!("[ok] Session with anthropic/claude-sonnet-4-20250514 created");
            match override_session.send(&prompt).await {
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
            println!("[skip] Model override failed (expected if model not in config): {}", e);
        }
    }

    println!("\n=== Done ===");
    Ok(())
}
