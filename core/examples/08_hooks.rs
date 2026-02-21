//! # Hooks — Lifecycle Event Interception
//!
//! Register hooks to intercept agent lifecycle events: pre/post tool use,
//! generate start/end, errors, etc. Hooks can log, modify, or block actions.
//!
//! ```bash
//! cargo run --example 08_hooks
//! ```

use a3s_code_core::hooks::{Hook, HookConfig, HookEventType, HookMatcher};
use a3s_code_core::{Agent, AgentEvent, SessionOptions};
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

    let opts = SessionOptions::new().with_permissive_policy();
    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;

    println!(
        "Hooks example — workspace: {}\n",
        workspace.path().display()
    );

    // ── Register hooks ──

    // 1. Pre-tool-use hook: log every tool invocation
    let pre_tool = Hook::new("audit_pre_tool", HookEventType::PreToolUse).with_config(HookConfig {
        priority: 10,
        ..Default::default()
    });
    session.register_hook(pre_tool);

    // 2. Post-tool-use hook: only for the "bash" tool
    let post_bash = Hook::new("audit_post_bash", HookEventType::PostToolUse)
        .with_matcher(HookMatcher::new().with_tool("bash".to_string()))
        .with_config(HookConfig {
            priority: 20,
            ..Default::default()
        });
    session.register_hook(post_bash);

    // 3. Generate-start hook: fires before each LLM call
    let gen_start = Hook::new("log_generate", HookEventType::GenerateStart);
    session.register_hook(gen_start);

    // 4. On-error hook
    let on_error = Hook::new("log_errors", HookEventType::OnError);
    session.register_hook(on_error);

    println!("Registered {} hooks", session.hook_count());
    println!("  • audit_pre_tool  (PreToolUse, priority=10)");
    println!("  • audit_post_bash (PostToolUse, bash only, priority=20)");
    println!("  • log_generate    (GenerateStart)");
    println!("  • log_errors      (OnError)\n");

    // ── Execute with hooks active ──
    let (mut rx, _handle) = session
        .stream(
            "Create a file called test.sh with `echo hello` and then run it with bash.",
            None,
        )
        .await?;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ToolStart { name, .. } => {
                println!("  [hook] 🔧 PreToolUse fired → {name}");
            }
            AgentEvent::ToolEnd {
                name, exit_code, ..
            } => {
                if name == "bash" {
                    println!("  [hook] 🔧 PostToolUse(bash) fired → exit={exit_code}");
                }
            }
            AgentEvent::TextDelta { .. } => {}
            AgentEvent::End { usage, .. } => {
                println!("\n■ Done ({} tokens)", usage.total_tokens);
                break;
            }
            AgentEvent::Error { message } => {
                println!("  [hook] ⚠ OnError fired → {message}");
                break;
            }
            _ => {}
        }
    }

    // ── Unregister a hook ──
    let removed = session.unregister_hook("audit_pre_tool");
    println!("\nUnregistered audit_pre_tool: {}", removed.is_some());
    println!("Remaining hooks: {}", session.hook_count());

    Ok(())
}
