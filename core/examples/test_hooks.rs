//! Hooks Example
//!
//! Demonstrates lifecycle hooks: intercepting PreToolUse and PostToolUse
//! events to audit, log, or block tool execution.
//!
//! Run with: cargo run --example test_hooks

use a3s_code_core::hooks::{Hook, HookEvent, HookEventType, HookHandler, HookResponse};
use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn find_config() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .expect("Config not found. Create ~/.a3s/config.hcl")
}

/// A hook handler that logs every tool call
struct AuditHandler {
    log: Arc<Mutex<Vec<String>>>,
}

impl HookHandler for AuditHandler {
    fn handle(&self, event: &HookEvent) -> HookResponse {
        let tool_name = match event {
            HookEvent::PreToolUse(e) => e.tool.clone(),
            HookEvent::PostToolUse(e) => e.tool.clone(),
            _ => "unknown".to_string(),
        };
        println!("  → [hook] intercepted: {}", tool_name);
        self.log.lock().unwrap().push(tool_name);
        HookResponse::continue_()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("A3S Code — Hooks Example");
    println!("{}", "=".repeat(50));

    let agent = Agent::new(find_config().to_str().unwrap()).await?;
    let opts = SessionOptions::new().with_permissive_policy();
    let session = agent.session(std::env::temp_dir().to_str().unwrap(), Some(opts))?;

    // Register a pre_tool_use hook
    let tool_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let hook = Hook::new("audit-tools", HookEventType::PreToolUse);
    session.register_hook(hook);
    session.register_hook_handler(
        "audit-tools",
        Arc::new(AuditHandler {
            log: Arc::clone(&tool_log),
        }),
    );

    println!("✓ Registered pre_tool_use hook");
    println!("Hook count: {}\n", session.hook_count());

    let result = session
        .send("What is 2 + 2? Just answer directly.", None)
        .await?;
    println!("\nResponse: {}", result.text.trim());

    let logged = tool_log.lock().unwrap();
    println!("\nTools intercepted by hook: {:?}", *logged);
    drop(logged);

    session.unregister_hook("audit-tools");
    println!("Hook count after unregister: {}", session.hook_count());

    println!("\n✓ Hooks example complete");
    Ok(())
}
