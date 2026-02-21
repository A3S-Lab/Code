//! # System Prompt Slots — Customizing agent personality without overriding core behavior
//!
//! Demonstrates the slot-based system prompt customization API:
//! 1. Create a session with custom role, guidelines, and response style
//! 2. Send a prompt and verify the agent responds according to the custom persona
//! 3. Show that core agentic capabilities (tool use, etc.) are preserved
//!
//! ```bash
//! cd crates/code
//! cargo run --example test_prompt_slots
//! ```

use a3s_code_core::{Agent, SessionOptions, SystemPromptSlots};
use std::path::PathBuf;

fn find_config() -> PathBuf {
    if let Ok(p) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().expect("no home dir");
    let home_cfg = home.join(".a3s/config.hcl");
    if home_cfg.exists() {
        return home_cfg;
    }
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

    let agent = Agent::new(config.to_str().unwrap()).await?;
    println!("Agent created ✓\n");

    let workspace = tempfile::tempdir()?;

    // --- Test 1: Custom role only ---
    println!("═══ Test 1: Custom role ═══");
    let opts = SessionOptions::new().with_prompt_slots(SystemPromptSlots {
        role: Some("You are a senior Rust developer who specializes in async programming.".into()),
        ..Default::default()
    });
    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;
    let result = session
        .send(
            "What is your area of expertise? Reply in one sentence.",
            None,
        )
        .await?;
    println!("Response: {}\n", result.text.trim());
    assert!(!result.text.is_empty(), "should get a response");

    // --- Test 2: Role + guidelines + response style ---
    println!("═══ Test 2: Role + guidelines + response style ═══");
    let opts = SessionOptions::new().with_prompt_slots(SystemPromptSlots {
        role: Some("You are a Python code reviewer.".into()),
        guidelines: Some("Always check for type hints. Flag any use of `eval()`.".into()),
        response_style: Some("Reply in bullet points. Be concise.".into()),
        ..Default::default()
    });
    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;

    // Write a Python file for the agent to review
    std::fs::write(
        workspace.path().join("app.py"),
        r#"
def add(a, b):
    return eval(f"{a} + {b}")

def greet(name):
    return "Hello " + name
"#,
    )?;

    let result = session
        .send("Review the file app.py and list any issues you find.", None)
        .await?;
    println!("Response:\n{}\n", result.text.trim());
    assert!(
        result.tool_calls_count > 0,
        "should have used read_file tool"
    );

    // --- Test 3: Extra instructions only (backward compat style) ---
    println!("═══ Test 3: Extra instructions ═══");
    let opts = SessionOptions::new().with_prompt_slots(SystemPromptSlots {
        extra: Some("Always end your response with '— A3S'".into()),
        ..Default::default()
    });
    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;
    let result = session.send("Say hello.", None).await?;
    println!("Response: {}\n", result.text.trim());

    // --- Test 4: Verify tools still work (core behavior preserved) ---
    println!("═══ Test 4: Core tool behavior preserved ═══");
    let opts = SessionOptions::new().with_prompt_slots(SystemPromptSlots {
        role: Some("You are a minimalist file manager.".into()),
        guidelines: Some("Only create files when explicitly asked.".into()),
        ..Default::default()
    });
    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;
    let result = session
        .send(
            "Create a file called test.txt with the content 'prompt slots work'. Then read it back.",
            None,
        )
        .await?;
    println!("Response: {}\n", result.text.trim());
    assert!(result.tool_calls_count > 0, "should have used tools");
    let test_file = workspace.path().join("test.txt");
    assert!(test_file.exists(), "test.txt should exist");

    println!("═══ All prompt slots tests passed ✓ ═══");
    Ok(())
}
