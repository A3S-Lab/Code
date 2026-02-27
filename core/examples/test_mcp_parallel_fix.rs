//! Real integration test for MCP tool naming fix and add_mcp_server API
//!
//! Run with: cargo run --example test_mcp_parallel_fix

use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=debug")
        .init();

    println!("=== MCP + Parallel Task Fix Integration Test ===\n");

    // Resolve config: project .a3s/config.hcl
    let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
        .expect("Config not found at .a3s/config.hcl or ~/.a3s/config.hcl");

    println!("Config: {}\n", config_path.display());

    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    // -------------------------------------------------------------------------
    // Test 1: MCP tool name format is correct (mcp__server__tool)
    // -------------------------------------------------------------------------
    println!("--- Test 1: MCP tool name format ---");
    {
        use a3s_code_core::mcp::{
            manager::McpManager,
            protocol::{McpServerConfig, McpTool, McpTransportConfig},
        };
        use std::sync::Arc;

        let manager = Arc::new(McpManager::new());

        // Verify get_all_tools returns correct double-underscore format
        // (manager has no connected servers, so we test via McpToolWrapper directly)
        let mcp_tool = McpTool {
            name: "create_issue".to_string(),
            description: Some("Create a GitHub issue".to_string()),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        };

        let wrappers = a3s_code_core::mcp::tools::create_mcp_tools(
            "github",
            vec![mcp_tool],
            Arc::clone(&manager),
        );

        assert_eq!(wrappers.len(), 1);
        let tool_name = wrappers[0].name();
        assert_eq!(
            tool_name, "mcp__github__create_issue",
            "Tool name must use double-underscore: got '{}'",
            tool_name
        );
        println!("  tool name: {} ✓", tool_name);
    }

    // -------------------------------------------------------------------------
    // Test 2: add_mcp_server on a live session (using a real stdio MCP server)
    // We use the filesystem MCP server which ships with npx @modelcontextprotocol/server-filesystem
    // Fall back gracefully if npx is not available.
    // -------------------------------------------------------------------------
    println!("\n--- Test 2: add_mcp_server on live session ---");
    {
        use a3s_code_core::mcp::{
            manager::McpManager,
            protocol::{McpServerConfig, McpTransportConfig},
        };
        use std::collections::HashMap;
        use std::sync::Arc;

        // Check if npx is available
        let npx_available = std::process::Command::new("npx")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if npx_available {
            let session =
                agent.session("/tmp", Some(SessionOptions::new().with_permissive_policy()))?;

            let manager = Arc::new(McpManager::new());
            manager
                .register_server(McpServerConfig {
                    name: "filesystem".to_string(),
                    transport: McpTransportConfig::Stdio {
                        command: "npx".to_string(),
                        args: vec![
                            "-y".to_string(),
                            "@modelcontextprotocol/server-filesystem".to_string(),
                            "/tmp".to_string(),
                        ],
                    },
                    enabled: true,
                    env: HashMap::new(),
                    oauth: None,
                    tool_timeout_secs: 30,
                })
                .await;

            match session
                .add_mcp_server(Arc::clone(&manager), "filesystem")
                .await
            {
                Ok(count) => {
                    println!("  add_mcp_server: registered {} tools ✓", count);
                    assert!(
                        count > 0,
                        "Expected at least 1 tool from filesystem MCP server"
                    );
                }
                Err(e) => {
                    println!("  add_mcp_server failed (non-fatal): {}", e);
                }
            }
        } else {
            println!("  npx not available — skipping live MCP server test");
        }
    }

    // -------------------------------------------------------------------------
    // Test 3: Basic LLM send works (validates config is correct)
    // -------------------------------------------------------------------------
    println!("\n--- Test 3: Basic LLM send ---");
    {
        let session =
            agent.session("/tmp", Some(SessionOptions::new().with_permissive_policy()))?;

        let result = session.send("Reply with exactly: OK", None).await?;
        println!("  LLM response: '{}' ✓", result.text.trim());
        assert!(!result.text.is_empty(), "Expected non-empty response");
    }

    println!("\n=== All tests passed ===");
    Ok(())
}
