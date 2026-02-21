//! # Multi-Turn Conversation — Context Preservation
//!
//! Demonstrates that the session preserves conversation history across
//! multiple send() calls, so the LLM remembers what happened earlier.
//!
//! ```bash
//! cargo run --example 03_multi_turn
//! ```

use a3s_code_core::Agent;
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
    let workspace = TempDir::new()?;
    let session = agent.session(workspace.path().to_str().unwrap(), None)?;

    println!(
        "Multi-turn example — workspace: {}\n",
        workspace.path().display()
    );

    // ── Turn 1: Create a config file ──
    println!("[Turn 1] Create a config file");
    let r1 = session
        .send(
            "Create a file called `config.toml` with:\n\
             [server]\nhost = \"0.0.0.0\"\nport = 8080\n\n\
             [database]\nurl = \"postgres://localhost/mydb\"\npool_size = 5",
            None,
        )
        .await?;
    println!(
        "  Tools: {}, Tokens: {}",
        r1.tool_calls_count, r1.usage.total_tokens
    );

    // ── Turn 2: Ask about the file (tests context memory) ──
    println!("\n[Turn 2] Ask about the file");
    let r2 = session
        .send(
            "What port is the server configured to use? Read the config file to confirm.",
            None,
        )
        .await?;
    println!(
        "  Tools: {}, Tokens: {}",
        r2.tool_calls_count, r2.usage.total_tokens
    );
    println!("  Answer: {}", truncate(&r2.text, 120));

    // ── Turn 3: Modify based on context ──
    println!("\n[Turn 3] Modify based on previous context");
    let r3 = session
        .send(
            "Change the server port to 3000 and increase pool_size to 10.",
            None,
        )
        .await?;
    println!(
        "  Tools: {}, Tokens: {}",
        r3.tool_calls_count, r3.usage.total_tokens
    );

    // ── Verify ──
    let history = session.history();
    println!("\nHistory: {} messages across 3 turns", history.len());

    let config_path = workspace.path().join("config.toml");
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        println!(
            "✓ config.toml: port=3000? {} pool=10? {}",
            content.contains("3000"),
            content.contains("10")
        );
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
