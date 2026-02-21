//! # Basic Send — Non-Streaming Agent Execution
//!
//! The simplest possible A3S Code example: create an agent from config,
//! bind to a workspace, send a prompt, and print the result.
//!
//! ```bash
//! cd crates/code
//! cargo run --example 01_basic_send
//! ```

use a3s_code_core::Agent;
use std::path::PathBuf;
use tempfile::TempDir;

fn find_config() -> PathBuf {
    if let Ok(p) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().expect("no home dir");
    let home_cfg = home.join(".a3s/config.hcl");
    if home_cfg.exists() {
        return home_cfg;
    }
    // Project root
    let project_cfg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.a3s/config.hcl");
    if project_cfg.exists() {
        return project_cfg;
    }
    panic!("Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("a3s_code_core=info")
        .init();

    let config = find_config();
    println!("Config: {}", config.display());

    // 1. Create agent from config
    let agent = Agent::new(config.to_str().unwrap()).await?;
    println!("Agent created ✓");

    // 2. Create a temp workspace
    let workspace = TempDir::new()?;
    let session = agent.session(workspace.path().to_str().unwrap(), None)?;
    println!("Session bound to: {}\n", workspace.path().display());

    // 3. Send a prompt (non-streaming)
    let result = session
        .send(
            "Create a file called hello.rs with a main function that prints 'Hello, A3S!'.\n\
             Then read the file back and confirm it looks correct.",
            None,
        )
        .await?;

    println!("─── Result ───");
    println!("Text:       {}", truncate(&result.text, 200));
    println!("Tool calls: {}", result.tool_calls_count);
    println!("Tokens:     {} total", result.usage.total_tokens);

    // 4. Verify the file
    let hello = workspace.path().join("hello.rs");
    if hello.exists() {
        println!(
            "\n✓ hello.rs created ({} bytes)",
            std::fs::metadata(&hello)?.len()
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
