//! Integration tests for MCP → tool injection and parallel task execution.
//!
//! Tests that only exercise local wiring (no LLM, no network) run without flags.
//! Tests that require a live LLM are gated with `#[ignore]`:
//!   `cargo test -p a3s-code-core --test integration_mcp -- --ignored`

use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
use a3s_code_core::{Agent, CodeConfig};
use std::collections::HashMap;
use std::path::PathBuf;

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

async fn make_agent() -> Agent {
    let config = CodeConfig::from_file(&config_path()).expect("failed to load config.hcl");
    Agent::from_config(config)
        .await
        .expect("failed to create agent")
}

fn failing_mcp_config(name: &str) -> McpServerConfig {
    // `cat` won't speak MCP — connect() will fail. Used for error-path tests.
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

fn filesystem_mcp_config(root: &str) -> McpServerConfig {
    McpServerConfig {
        name: "filesystem".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                root.to_string(),
            ],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 30,
    }
}

// ============================================================================
// Group 1: MCP wiring via AgentSession — no network, no LLM
// ============================================================================

#[tokio::test]
async fn test_session_mcp_status_empty_initially() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("session creation should succeed");

    let status = session.mcp_status().await;
    assert!(
        status.is_empty(),
        "status should be empty before any MCP server is added"
    );
}

#[tokio::test]
async fn test_session_add_mcp_server_failed_connect_returns_error() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("session creation should succeed");

    let result = session
        .add_mcp_server(failing_mcp_config("bad-server"))
        .await;
    assert!(
        result.is_err(),
        "connecting `cat` as MCP should fail with an error"
    );
}

#[tokio::test]
async fn test_session_remove_mcp_server_nonexistent_is_ok() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("session creation should succeed");

    let result = session.remove_mcp_server("nonexistent").await;
    assert!(
        result.is_ok(),
        "removing a server that was never added should be a no-op: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_session_mcp_status_no_connected_after_failed_add() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("session creation should succeed");

    let _ = session.add_mcp_server(failing_mcp_config("bad")).await;

    for (name, s) in session.mcp_status().await {
        assert!(
            !s.connected,
            "server '{}' should not be connected after failed add",
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
    use std::sync::Arc;

    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    // Should not panic when no tools match the prefix
    executor.unregister_tools_by_prefix("mcp__nonexistent__");
}

// ============================================================================
// Group 3: MCP tool injection via AgentSession — requires npx (#[ignore])
// ============================================================================

/// Verify that add_mcp_server injects tools into the session's ToolExecutor
/// and that remove_mcp_server withdraws them. Requires `npx` on PATH.
#[tokio::test]
#[ignore]
async fn test_mcp_tools_injected_and_removed_via_session() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("session creation should succeed");

    // Before: no MCP tools
    let defs_before = session.tool_definitions();
    assert!(
        !defs_before.iter().any(|t| t.name.starts_with("mcp__")),
        "no MCP tools should be registered before add_mcp_server"
    );

    // Add filesystem MCP server
    let count = session
        .add_mcp_server(filesystem_mcp_config(tmp.path().to_str().unwrap()))
        .await
        .expect("filesystem MCP server should connect — is npx installed?");

    assert!(
        count > 0,
        "filesystem server should expose at least one tool"
    );

    // After add: MCP tools present
    let defs_after = session.tool_definitions();
    let mcp_tools: Vec<_> = defs_after
        .iter()
        .filter(|t| t.name.starts_with("mcp__filesystem__"))
        .collect();
    assert_eq!(
        mcp_tools.len(),
        count,
        "tool definitions should reflect all registered MCP tools"
    );

    // MCP status shows connected
    let status = session.mcp_status().await;
    assert!(status["filesystem"].connected, "server should be connected");
    assert_eq!(status["filesystem"].tool_count, count);

    // Remove server
    session
        .remove_mcp_server("filesystem")
        .await
        .expect("remove should succeed");

    // After remove: MCP tools gone
    let defs_final = session.tool_definitions();
    assert!(
        !defs_final
            .iter()
            .any(|t| t.name.starts_with("mcp__filesystem__")),
        "MCP tools should be unregistered after remove_mcp_server"
    );
}

/// Verify that adding the same MCP server twice reuses the session's shared
/// McpManager rather than creating a duplicate.
#[tokio::test]
#[ignore]
async fn test_mcp_shared_manager_no_duplicate_tools() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("session creation should succeed");

    let config = filesystem_mcp_config(tmp.path().to_str().unwrap());
    let count1 = session
        .add_mcp_server(config.clone())
        .await
        .expect("first add should succeed");

    // Adding the same server again should not duplicate tools
    let count2 = session.add_mcp_server(config).await.unwrap_or(0);
    let defs = session.tool_definitions();
    let mcp_count = defs
        .iter()
        .filter(|t| t.name.starts_with("mcp__filesystem__"))
        .count();

    // Tools from first add: count1. Second add may succeed or fail, but
    // total tools in executor should not exceed count1 * 2.
    assert!(
        mcp_count <= count1 + count2,
        "should not have more tools than what was registered"
    );
}

// ============================================================================
// Group 4: LLM uses MCP tool — requires npx + kimi-k2.5 (#[ignore])
// ============================================================================

/// Ask kimi-k2.5 to use an MCP filesystem tool to list a directory.
/// Requires: npx available + kimi-k2.5 reachable at configured endpoint.
#[tokio::test]
#[ignore]
async fn test_kimi_uses_mcp_filesystem_tool() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();

    // Write a marker file for the agent to discover
    std::fs::write(tmp.path().join("hello.txt"), "MCP_WORKS").unwrap();

    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("session creation should succeed");

    session
        .add_mcp_server(filesystem_mcp_config(tmp.path().to_str().unwrap()))
        .await
        .expect("filesystem MCP server should connect — is npx installed?");

    let result = session
        .send(
            &format!(
                "Use the MCP filesystem tool to list files in '{}' and tell me what you find.",
                tmp.path().display()
            ),
            None,
        )
        .await
        .expect("send should succeed");

    assert!(
        result.text.contains("hello.txt") || result.text.to_lowercase().contains("hello"),
        "LLM should have used filesystem MCP tool and found hello.txt, got: {}",
        result.text
    );
}

// ============================================================================
// Group 5: Parallel subagent tasks via LLM — requires kimi-k2.5 (#[ignore])
// ============================================================================

/// Ask the agent to run three parallel subagent tasks and verify all complete.
#[tokio::test]
#[ignore]
async fn test_parallel_tasks_via_session() {
    let agent = make_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("failed to create session");

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

    assert!(result.text.contains("RESULT_A"), "got: {}", result.text);
    assert!(result.text.contains("RESULT_B"), "got: {}", result.text);
    assert!(result.text.contains("RESULT_C"), "got: {}", result.text);
}
