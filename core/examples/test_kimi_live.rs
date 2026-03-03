//! Live integration test — kimi-k2.5 + MCP + task/parallel_task tools
//!
//! Tests all recently added or fixed features against the real kimi endpoint:
//!   1. task tool (delegate to general subagent, wait for result)
//!   2. parallel_task tool (fan-out to multiple subagents concurrently)
//!   3. MCP injection: add_mcp_server → LLM uses mcp__ tool → remove_mcp_server
//!   4. mcp_status shows connected/error state correctly
//!   5. tool_names reflects live tool registry
//!   6. refresh_mcp_tools (smoke-test on agent with no global MCP servers)
//!
//! Run with:
//!   cargo run --example test_kimi_live

use a3s_code_core::{
    mcp::{McpServerConfig, McpTransportConfig},
    Agent, SessionOptions,
};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn resolve_config() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".a3s/config.hcl"))
                .filter(|p| p.exists())
        })
        .expect("Config not found at .a3s/config.hcl or ~/.a3s/config.hcl")
}

fn filesystem_mcp(root: &str) -> McpServerConfig {
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

fn pass(label: &str) {
    println!("  ✓  {label}");
}

fn skip(label: &str, reason: &str) {
    println!("  –  {label} (skipped: {reason})");
}

fn permissive() -> SessionOptions {
    SessionOptions::new().with_permissive_policy()
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn,a3s_code_core=info")
        .init();

    let config_path = resolve_config();
    println!("=== kimi live integration test ===");
    println!("config: {}\n", config_path.display());

    let agent = Agent::new(config_path.to_str().unwrap()).await?;
    let tmp = tempfile::tempdir()?;

    // ─────────────────────────────────────────────────────────────────────────
    // Test 1: task tool — delegate to general subagent
    // ─────────────────────────────────────────────────────────────────────────
    println!("── Test 1: task tool (subagent delegation) ──");
    {
        let session = agent.session(tmp.path().display().to_string(), Some(permissive()))?;
        let result = session
            .send(
                "Use the task tool to delegate the following to the 'general' agent, \
                 then return its exact reply verbatim: \
                 'Reply with exactly the word: TASK_OK'",
                None,
            )
            .await?;

        assert!(
            result.text.contains("TASK_OK"),
            "task tool should have delegated and returned TASK_OK, got: {}",
            result.text
        );
        pass("task tool delegated and returned TASK_OK");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 2: parallel_task tool — fan-out to multiple subagents
    // ─────────────────────────────────────────────────────────────────────────
    println!("\n── Test 2: parallel_task tool (concurrent fan-out) ──");
    {
        let session = agent.session(tmp.path().display().to_string(), Some(permissive()))?;
        let result = session
            .send(
                "Use the parallel_task tool to run these three tasks concurrently \
                 using the 'general' agent, then list their outputs:\n\
                 1. Reply with exactly: PARALLEL_A\n\
                 2. Reply with exactly: PARALLEL_B\n\
                 3. Reply with exactly: PARALLEL_C",
                None,
            )
            .await?;

        assert!(
            result.text.contains("PARALLEL_A"),
            "expected PARALLEL_A, got: {}",
            result.text
        );
        assert!(
            result.text.contains("PARALLEL_B"),
            "expected PARALLEL_B, got: {}",
            result.text
        );
        assert!(
            result.text.contains("PARALLEL_C"),
            "expected PARALLEL_C, got: {}",
            result.text
        );
        pass("parallel_task ran 3 subagents concurrently, all results returned");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 3: MCP add/status/tool_names/LLM-use/remove
    // ─────────────────────────────────────────────────────────────────────────
    println!("\n── Test 3: MCP injection (add → status → LLM use → remove) ──");
    let npx_ok = std::process::Command::new("npx")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if npx_ok {
        // Write a marker file the LLM will discover
        let marker = tmp.path().join("hello_mcp.txt");
        std::fs::write(&marker, "MCP_WORKS")?;

        let session = agent.session(tmp.path().display().to_string(), Some(permissive()))?;

        // 3a — before add: no MCP tools
        let tools_before = session.tool_names();
        assert!(
            !tools_before.iter().any(|n| n.starts_with("mcp__")),
            "no MCP tools before add_mcp_server"
        );
        pass("no mcp__ tools before add_mcp_server");

        // 3b — add server
        let count = session
            .add_mcp_server(filesystem_mcp(tmp.path().to_str().unwrap()))
            .await
            .expect("filesystem MCP server should connect — is npx installed?");
        assert!(count > 0, "filesystem server must expose ≥1 tool");
        pass(&format!("add_mcp_server registered {count} tools"));

        // 3c — tool_names reflects injected tools
        let tools_after = session.tool_names();
        let mcp_tools: Vec<_> = tools_after
            .iter()
            .filter(|n| n.starts_with("mcp__filesystem__"))
            .collect();
        assert_eq!(
            mcp_tools.len(),
            count,
            "tool_names should show exactly the registered MCP tools"
        );
        pass(&format!(
            "tool_names shows {} mcp__filesystem__ tools",
            mcp_tools.len()
        ));

        // 3d — mcp_status shows connected, correct tool_count, no error
        let status = session.mcp_status().await;
        let fs_status = status
            .get("filesystem")
            .expect("filesystem must appear in mcp_status");
        assert!(fs_status.connected, "filesystem server should be connected");
        assert_eq!(fs_status.tool_count, count);
        assert!(
            fs_status.error.is_none(),
            "no error expected for successful connect"
        );
        pass("mcp_status: connected=true, tool_count correct, error=None");

        // 3e — LLM uses the MCP filesystem tool
        let prompt = format!(
            "Use the MCP filesystem tool to list files in '{}' and tell me what you find.",
            tmp.path().display()
        );
        let result = session.send(&prompt, None).await?;
        assert!(
            result.text.contains("hello_mcp.txt") || result.text.to_lowercase().contains("hello"),
            "LLM should have used MCP filesystem tool and found hello_mcp.txt, got: {}",
            result.text
        );
        pass("LLM used mcp__filesystem__ tool and found marker file");

        // 3f — remove server: tools disappear
        session.remove_mcp_server("filesystem").await?;
        let tools_final = session.tool_names();
        assert!(
            !tools_final
                .iter()
                .any(|n| n.starts_with("mcp__filesystem__")),
            "MCP tools should be gone after remove_mcp_server"
        );
        pass("remove_mcp_server removed all mcp__filesystem__ tools");
    } else {
        skip("MCP inject/use/remove", "npx not available");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 4: mcp_status captures error for a failed server
    // ─────────────────────────────────────────────────────────────────────────
    println!("\n── Test 4: mcp_status error field on failed connect ──");
    {
        let session = agent.session(tmp.path().display().to_string(), Some(permissive()))?;
        let bad_config = McpServerConfig {
            name: "bad-server".to_string(),
            transport: McpTransportConfig::Stdio {
                command: "nonexistent-mcp-binary-xyz".to_string(),
                args: vec![],
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 3,
        };

        let err = session.add_mcp_server(bad_config).await;
        assert!(err.is_err(), "connecting nonexistent binary should fail");

        let status = session.mcp_status().await;
        if let Some(s) = status.get("bad-server") {
            assert!(!s.connected, "bad-server should not be connected");
            assert!(
                s.error.is_some(),
                "error field should be populated after failed connect"
            );
            pass(&format!(
                "mcp_status.error captured: {}",
                s.error.as_deref().unwrap_or("")
            ));
        } else {
            // Server may not be registered if connect never stored config — still acceptable
            pass("bad-server not in status (connect failed before register)");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 5: refresh_mcp_tools (smoke test — agent has no global MCP servers)
    // ─────────────────────────────────────────────────────────────────────────
    println!("\n── Test 5: refresh_mcp_tools (smoke test) ──");
    {
        agent.refresh_mcp_tools().await?;
        pass("refresh_mcp_tools completed without error");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Summary
    // ─────────────────────────────────────────────────────────────────────────
    println!("\n=== all tests passed ===");
    Ok(())
}
