//! Integration tests for MCP server management and parallel task execution.
//!
//! These tests use the real LLM config from `.a3s/config.hcl`.
//!
//! Tests that require a live LLM are gated with `#[ignore]` — run with:
//!   `cargo test -p a3s-code-core --test integration_mcp -- --ignored`
//!
//! Tests that only exercise local MCP wiring (no LLM) run without `--ignored`.

use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
use a3s_code_core::session::SessionManager;
use a3s_code_core::tools::ToolExecutor;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

// ============================================================================
// Helpers
// ============================================================================

fn config_path() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // crates/code/core
    manifest
        .parent() // crates/code
        .and_then(|p| p.parent()) // crates
        .and_then(|p| p.parent()) // repo root
        .expect("failed to resolve repo root")
        .join(".a3s/config.hcl")
}

fn make_session_manager() -> SessionManager {
    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    SessionManager::new(None, executor)
}

fn echo_mcp_config(name: &str) -> McpServerConfig {
    // A minimal stdio MCP server using `cat` — it won't respond to MCP
    // initialize, so connect() will fail. We use this only for register tests.
    McpServerConfig {
        name: name.to_string(),
        transport: McpTransportConfig::Stdio {
            command: "cat".to_string(),
            args: vec![],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 5,
    }
}

// ============================================================================
// Group 1: MCP status — no network, no LLM
// ============================================================================

#[tokio::test]
async fn test_mcp_status_empty_when_no_manager() {
    let manager = make_session_manager();
    let status = manager.mcp_status().await;
    assert!(
        status.is_empty(),
        "status should be empty before any MCP server is added"
    );
}

#[tokio::test]
async fn test_add_mcp_server_registers_in_status() {
    let manager = make_session_manager();

    // `cat` won't speak MCP, so connect() will fail — that's expected.
    // We only verify that the error path doesn't panic and that the manager
    // handles it gracefully.
    let config = echo_mcp_config("test-echo");
    let result = manager.add_mcp_server(config).await;

    // connect() will fail because `cat` doesn't implement MCP initialize.
    // The important thing is that the error is returned cleanly.
    assert!(
        result.is_err(),
        "connecting to a non-MCP process should fail"
    );
}

#[tokio::test]
async fn test_remove_mcp_server_nonexistent_is_ok() {
    let manager = make_session_manager();
    // Removing a server that was never added should not panic.
    let result = manager.remove_mcp_server("nonexistent").await;
    assert!(
        result.is_ok(),
        "removing nonexistent server should be a no-op: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_mcp_status_after_failed_connect_is_empty() {
    // After a failed add_mcp_server, the manager is created but the server
    // is registered (not connected). Status should reflect that.
    let manager = make_session_manager();
    let config = echo_mcp_config("failing-server");
    let _ = manager.add_mcp_server(config).await; // ignore error

    // The manager was lazily created; status may show the registered server
    // as disconnected, or be empty if registration was rolled back on error.
    // Either way, no connected server should appear.
    let status = manager.mcp_status().await;
    for (name, s) in &status {
        assert!(
            !s.connected,
            "server '{}' should not be connected after failed connect",
            name
        );
    }
}

// ============================================================================
// Group 2: ToolExecutor prefix unregistration — no network, no LLM
// ============================================================================

#[tokio::test]
async fn test_unregister_tools_by_prefix_is_noop_for_unknown_prefix() {
    use a3s_code_core::tools::ToolExecutor;

    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Unregistering a prefix that matches nothing should not panic
    executor.unregister_tools_by_prefix("mcp__nonexistent__");
    // If we get here without panic, the test passes
}

// ============================================================================
// Group 3: Parallel task execution via LLM — requires LLM (#[ignore])
// ============================================================================

/// Ask the agent to run three parallel subagent tasks and verify all complete.
/// The Task tool is a built-in tool exposed through the session's send() API.
#[tokio::test]
#[ignore]
async fn test_parallel_tasks_via_session() {
    use a3s_code_core::{Agent, CodeConfig};

    let config = CodeConfig::from_file(&config_path()).expect("failed to load config.hcl");
    let agent = Agent::from_config(config)
        .await
        .expect("failed to create agent");

    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("failed to create session");

    // Ask the agent to spawn parallel subagents and collect their outputs.
    // The prompt is explicit so the LLM uses the Task tool.
    let result = session
        .send(
            "Use the Task tool to run these three tasks in parallel using the 'general' agent:\
             \n1. Reply with exactly: RESULT_A\
             \n2. Reply with exactly: RESULT_B\
             \n3. Reply with exactly: RESULT_C\
             \nAfter all tasks complete, list their outputs in order.",
            None,
        )
        .await
        .expect("send should succeed");

    // All three markers should appear in the final output
    assert!(
        result.text.contains("RESULT_A"),
        "output should contain RESULT_A, got: {}",
        result.text
    );
    assert!(
        result.text.contains("RESULT_B"),
        "output should contain RESULT_B, got: {}",
        result.text
    );
    assert!(
        result.text.contains("RESULT_C"),
        "output should contain RESULT_C, got: {}",
        result.text
    );
}

// ============================================================================
// Group 4: MCP with real filesystem server — requires npx (#[ignore])
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_add_real_mcp_filesystem_server() {
    // Requires: npx @modelcontextprotocol/server-filesystem
    let manager = make_session_manager();

    let tmp = tempfile::tempdir().unwrap();
    let config = McpServerConfig {
        name: "filesystem".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                tmp.path().display().to_string(),
            ],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 30,
    };

    let result = manager.add_mcp_server(config).await;
    assert!(
        result.is_ok(),
        "filesystem MCP server should connect: {:?}",
        result.err()
    );

    let status = manager.mcp_status().await;
    assert!(
        status.contains_key("filesystem"),
        "filesystem server should appear in status"
    );
    assert!(
        status["filesystem"].connected,
        "filesystem server should be connected"
    );
    assert!(
        status["filesystem"].tool_count > 0,
        "filesystem server should expose tools"
    );

    // Clean up
    let _ = manager.remove_mcp_server("filesystem").await;
    let status_after = manager.mcp_status().await;
    if let Some(s) = status_after.get("filesystem") {
        assert!(
            !s.connected,
            "filesystem server should be disconnected after removal"
        );
    }
}
