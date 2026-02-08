//! Session Manager Example
//!
//! Demonstrates creating and managing agent sessions with:
//! - Session lifecycle (create, configure, destroy)
//! - HITL confirmation policies
//! - Permission policies per session
//! - Memory system
//!
//! Run with:
//!   cargo run --example session_manager

use a3s_box_code::hitl::{ConfirmationPolicy, SessionLane, TimeoutAction};
use a3s_box_code::memory::{AgentMemory, MemoryItem, MemoryType};
use a3s_box_code::permissions::{PermissionDecision, PermissionPolicy};
use a3s_box_code::session::{SessionConfig, SessionManager};
use a3s_box_code::tools::ToolExecutor;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("a3s_box_code=info")
        .init();

    println!("╔══════════════════════════════════════════════════╗");
    println!("║       A3S Code - Session Manager Demo            ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    // Create workspace and tool executor
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace = temp_dir.path().to_string_lossy().to_string();
    println!("📁 Workspace: {}\n", workspace);

    let tool_executor = Arc::new(ToolExecutor::new(workspace.clone()));

    // Create session manager (no LLM client for this demo)
    let session_manager = Arc::new(SessionManager::new(None, tool_executor.clone()));

    // =========================================================================
    // 1. Create a basic session
    // =========================================================================
    println!("━━━ 1. Basic Session ━━━");
    let basic_config = SessionConfig {
        name: "basic-session".to_string(),
        workspace: workspace.clone(),
        system_prompt: Some("You are a helpful coding assistant.".to_string()),
        max_context_length: 200_000,
        auto_compact: true,
        ..Default::default()
    };

    let session_id = session_manager
        .create_session("session-basic".to_string(), basic_config)
        .await
        .expect("Failed to create basic session");

    println!("  ✅ Created session: {}", session_id);

    // Get session info
    let session_lock = session_manager
        .get_session("session-basic")
        .await
        .expect("Session not found");
    {
        let session = session_lock.read().await;
        println!("  ID:     {}", session.id);
        println!("  State:  {:?}", session.state);
        println!("  Tools:  {} available", session.tools.len());
        println!(
            "  Prompt: {:?}",
            session
                .config
                .system_prompt
                .as_deref()
                .map(|s| truncate(s, 40))
        );
    }
    println!();

    // =========================================================================
    // 2. Session with HITL confirmation policy
    // =========================================================================
    println!("━━━ 2. Session with HITL Policy ━━━");
    let hitl_policy = ConfirmationPolicy::enabled()
        // Auto-approve read-only operations (Query lane)
        .with_yolo_lanes([SessionLane::Query])
        // Auto-approve specific safe tools
        .with_auto_approve_tools(["grep".to_string(), "glob".to_string(), "ls".to_string()])
        // Always require confirmation for these
        .with_require_confirm_tools(["bash".to_string(), "write".to_string()])
        // 60 second timeout, reject if no response
        .with_timeout(60_000, TimeoutAction::Reject);

    let hitl_config = SessionConfig {
        name: "hitl-session".to_string(),
        workspace: workspace.clone(),
        confirmation_policy: Some(hitl_policy.clone()),
        ..Default::default()
    };

    session_manager
        .create_session("session-hitl".to_string(), hitl_config)
        .await
        .expect("Failed to create HITL session");

    println!("  ✅ Created 'hitl-session'");
    println!("  HITL Policy:");
    println!("    Enabled:         {}", hitl_policy.enabled);
    println!("    YOLO Lanes:      {:?}", hitl_policy.yolo_lanes);
    println!("    Auto-approve:    {:?}", hitl_policy.auto_approve_tools);
    println!(
        "    Require confirm: {:?}",
        hitl_policy.require_confirm_tools
    );
    println!("    Timeout:         {}ms", hitl_policy.default_timeout_ms);
    println!("    Timeout Action:  {:?}", hitl_policy.timeout_action);
    println!();

    // =========================================================================
    // 3. Session with permission policy
    // =========================================================================
    println!("━━━ 3. Session with Permission Policy ━━━");
    let permission_policy = PermissionPolicy::new()
        .allow("Read(*)")
        .allow("Grep(*)")
        .allow("Glob(*)")
        .allow("ls(*)")
        .allow("Bash(cargo:*)")
        .allow("Bash(git:*)")
        .deny("Bash(rm -rf:*)")
        .deny("Bash(sudo:*)")
        .ask("Write(*)")
        .ask("Edit(*)");

    let perm_config = SessionConfig {
        name: "secure-session".to_string(),
        workspace: workspace.clone(),
        permission_policy: Some(permission_policy),
        system_prompt: Some(
            "You are a secure coding assistant. You can read and search code freely, \
             but file modifications require user approval."
                .to_string(),
        ),
        ..Default::default()
    };

    session_manager
        .create_session("session-secure".to_string(), perm_config)
        .await
        .expect("Failed to create secure session");

    println!("  ✅ Created 'secure-session' with permission policy");

    // Test permission checks
    let session_lock = session_manager
        .get_session("session-secure")
        .await
        .expect("Session not found");
    {
        let session = session_lock.read().await;
        let checks: Vec<(&str, serde_json::Value, &str)> = vec![
            (
                "Read",
                serde_json::json!({"file_path": "src/main.rs"}),
                "Read source",
            ),
            (
                "Bash",
                serde_json::json!({"command": "cargo test"}),
                "cargo test",
            ),
            (
                "Bash",
                serde_json::json!({"command": "rm -rf /tmp"}),
                "rm -rf",
            ),
            (
                "Write",
                serde_json::json!({"file_path": "out.txt", "content": "..."}),
                "Write file",
            ),
        ];

        for (tool, args, desc) in checks {
            let decision = session.check_permission(tool, &args).await;
            let icon = match decision {
                PermissionDecision::Allow => "✅",
                PermissionDecision::Deny => "🚫",
                PermissionDecision::Ask => "❓",
            };
            println!("    {} {:?} ← {} ({})", icon, decision, desc, tool);
        }
    }
    println!();

    // =========================================================================
    // 4. Subagent session (child of another session)
    // =========================================================================
    println!("━━━ 4. Subagent Session ━━━");
    let subagent_config = SessionConfig {
        name: "test-runner".to_string(),
        workspace: workspace.clone(),
        parent_id: Some("session-basic".to_string()),
        system_prompt: Some("You are a test runner. Execute tests and report results.".to_string()),
        ..Default::default()
    };

    session_manager
        .create_session("session-subagent".to_string(), subagent_config)
        .await
        .expect("Failed to create subagent session");

    let session_lock = session_manager
        .get_session("session-subagent")
        .await
        .expect("Session not found");
    {
        let session = session_lock.read().await;
        println!("  ✅ Created subagent session");
        println!("  ID:        {}", session.id);
        println!("  Parent:    {:?}", session.parent_id);
        println!("  Is child:  {}", session.is_child_session());
    }
    println!();

    // =========================================================================
    // 5. List all sessions
    // =========================================================================
    println!("━━━ 5. List All Sessions ━━━");
    let sessions = session_manager.list_sessions().await;
    println!("  Active sessions ({}):", sessions.len());
    for sid in &sessions {
        let session_lock = session_manager.get_session(sid).await.unwrap();
        let session = session_lock.read().await;
        let parent_info = if session.is_child_session() {
            format!(
                " (child of {})",
                session.parent_id.as_deref().unwrap_or("?")
            )
        } else {
            String::new()
        };
        println!(
            "    • {} [{}] {:?}{}",
            session.config.name, session.id, session.state, parent_info
        );
    }
    println!();

    // =========================================================================
    // 6. Memory system demo
    // =========================================================================
    println!("━━━ 6. Memory System ━━━");
    let memory = AgentMemory::in_memory();

    // Store different types of memories
    memory
        .remember(
            MemoryItem::new("User prefers Rust for backend services")
                .with_type(MemoryType::Semantic)
                .with_importance(0.9)
                .with_tag("preference")
                .with_tag("rust"),
        )
        .await
        .unwrap();

    memory
        .remember(
            MemoryItem::new("Successfully refactored auth module using async/await pattern")
                .with_type(MemoryType::Episodic)
                .with_importance(0.7)
                .with_tag("success")
                .with_tag("refactor"),
        )
        .await
        .unwrap();

    memory
        .remember_success(
            "Implemented REST API with Axum",
            &["write".to_string(), "bash".to_string()],
            "API endpoints created and tested",
        )
        .await
        .unwrap();

    memory
        .remember_failure(
            "Deploy to production",
            "Permission denied: missing SSH key",
            &["bash".to_string()],
        )
        .await
        .unwrap();

    // Query memories
    let stats = memory.stats().await.unwrap();
    println!("  Memory Stats:");
    println!("    Long-term:  {} items", stats.long_term_count);
    println!("    Short-term: {} items", stats.short_term_count);
    println!("    Working:    {} items", stats.working_count);

    // Search by similarity
    let results = memory.recall_similar("Rust backend", 3).await.unwrap();
    println!("\n  🔍 Search 'Rust backend' ({} results):", results.len());
    for item in &results {
        println!(
            "    • [{}] {}",
            format!("{:?}", item.memory_type),
            truncate(&item.content, 50)
        );
    }

    // Search by tags
    let failures = memory
        .recall_by_tags(&["failure".to_string()], 5)
        .await
        .unwrap();
    println!("\n  🏷️  Tag 'failure' ({} results):", failures.len());
    for item in &failures {
        println!("    • {}", truncate(&item.content, 60));
    }
    println!();

    // =========================================================================
    // 7. Destroy sessions
    // =========================================================================
    println!("━━━ 7. Cleanup ━━━");
    let sessions = session_manager.list_sessions().await;
    for sid in &sessions {
        session_manager
            .destroy_session(sid)
            .await
            .expect("Failed to destroy session");
        println!("  🗑️  Destroyed {}", sid);
    }

    let remaining = session_manager.list_sessions().await;
    println!("  Remaining sessions: {}", remaining.len());
    println!();

    println!("✅ Session manager demo complete!");
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max])
    } else {
        s.to_string()
    }
}
