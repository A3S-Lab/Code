//! # Skills & Security — Built-in Skills + Output Sanitization
//!
//! Demonstrates enabling built-in skills (code-review, find-bugs, etc.)
//! and the default security provider (taint tracking + output sanitization).
//!
//! ```bash
//! cargo run --example 06_skills_security
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
    let workspace = TempDir::new()?;

    // Pre-create a file with intentional issues
    std::fs::write(
        workspace.path().join("app.py"),
        r#"import os
import sys

def process_data(data):
    result = []
    for i in range(len(data)):
        if data[i] != None:
            result.append(data[i] * 2)
    return result

def read_file(path):
    f = open(path, 'r')
    content = f.read()
    return content

API_KEY = "sk-1234567890abcdef"
DB_PASSWORD = "super_secret_123"
"#,
    )?;

    // ── Part 1: Skills-augmented code review ──
    println!("═══ Part 1: Skills-Augmented Code Review ═══\n");

    // List built-in skills
    let skills = a3s_code_core::builtin_skills();
    println!("Built-in skills ({}):", skills.len());
    for s in &skills {
        println!("  • {} — {}", s.name, s.description);
    }
    println!();

    let opts = SessionOptions::new()
        .with_builtin_skills()
        .with_default_security()
        .with_permissive_policy();

    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;

    let result = session
        .send(
            "Review `app.py` for code quality issues and fix them:\n\
             - Pythonic style (is not None, enumerate)\n\
             - Resource management (context managers)\n\
             - Security issues (hardcoded secrets)\n\
             Apply fixes by editing the file.",
            None,
        )
        .await?;

    println!("Tool calls: {}", result.tool_calls_count);
    println!("Tokens:     {}", result.usage.total_tokens);

    // Check improvements
    let content = std::fs::read_to_string(workspace.path().join("app.py"))?;
    println!("\nImprovements:");
    println!("  context manager: {}", content.contains("with open"));
    println!("  enumerate:       {}", content.contains("enumerate"));
    println!(
        "  no hardcoded key:{}",
        !content.contains("sk-1234567890abcdef")
    );

    // ── Part 2: Security provider in action ──
    println!("\n═══ Part 2: Security Provider ═══\n");
    println!("The DefaultSecurityProvider automatically:");
    println!("  • Tracks tainted input (detects PII patterns)");
    println!("  • Sanitizes output (redacts sensitive data)");
    println!("  • These run transparently inside execute_loop");

    let r2 = session
        .send(
            "Read app.py and tell me what security issues you found. \
             Do NOT include any actual secret values in your response.",
            None,
        )
        .await?;
    println!("\nResponse: {}", truncate(&r2.text, 300));

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
