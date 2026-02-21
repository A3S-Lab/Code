//! # Model Switching — Use Different Providers Per Session
//!
//! Shows how to override the default model on a per-session basis.
//! Each session can target a different provider/model from the config.
//!
//! ```bash
//! cargo run --example 04_model_switching
//! ```

use a3s_code_core::{Agent, SessionOptions};
use std::path::PathBuf;
use tempfile::TempDir;

fn find_config() -> PathBuf {
    if let Ok(p) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().expect("no home dir");
    let candidates = [
        home.join(".a3s/config.hcl"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.a3s/config.hcl"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .expect("Config not found. Set A3S_CONFIG or create ~/.a3s/config.hcl")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = Agent::new(find_config().to_str().unwrap()).await?;

    let prompt = "What model are you? Reply in one short sentence.";

    // ── Session 1: Default model (from config's default_model) ──
    println!("─── Session 1: Default model ───");
    let ws1 = TempDir::new()?;
    let s1 = agent.session(ws1.path().to_str().unwrap(), None)?;
    let r1 = s1.send(prompt, None).await?;
    println!("  Response: {}", r1.text.trim());
    println!("  Tokens:   {}\n", r1.usage.total_tokens);

    // ── Session 2: Override to a specific model ──
    // Use the model specified by A3S_ALT_MODEL env var, or try "anthropic/claude-sonnet-4-20250514"
    let alt_model = std::env::var("A3S_ALT_MODEL")
        .unwrap_or_else(|_| "anthropic/claude-sonnet-4-20250514".to_string());

    println!("─── Session 2: Override to {alt_model} ───");
    let ws2 = TempDir::new()?;
    let opts = SessionOptions::new().with_model(&alt_model);
    match agent.session(ws2.path().to_str().unwrap(), Some(opts)) {
        Ok(s2) => {
            let r2 = s2.send(prompt, None).await?;
            println!("  Response: {}", r2.text.trim());
            println!("  Tokens:   {}\n", r2.usage.total_tokens);
        }
        Err(e) => {
            println!("  Skipped (model not in config): {e}\n");
        }
    }

    // ── Session 3: Override with temperature=0.0 (deterministic) ──
    println!("─── Session 3: With temperature=0.0 (deterministic) ───");
    // Use the default model but with temperature override
    // Note: temperature/thinking_budget require a model override to take effect
    let default_model =
        std::env::var("A3S_MODEL").unwrap_or_else(|_| "openai/kimi-k2.5".to_string());

    let ws3 = TempDir::new()?;
    let opts = SessionOptions::new()
        .with_model(&default_model)
        .with_temperature(0.0);
    match agent.session(ws3.path().to_str().unwrap(), Some(opts)) {
        Ok(s3) => {
            let r3 = s3
                .send("What is 2 + 2? Reply with just the number.", None)
                .await?;
            println!("  Response: {}", r3.text.trim());
            println!("  Tokens:   {}", r3.usage.total_tokens);
        }
        Err(e) => {
            println!("  Skipped: {e}");
        }
    }

    Ok(())
}
