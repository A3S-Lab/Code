//! MCP (Model Context Protocol) Support
//!
//! Provides integration with MCP servers for extending A3S Code with external tools.
//!
//! ## Overview
//!
//! MCP is an open protocol for connecting AI assistants to external tools and data sources.
//! This module implements:
//!
//! - **Protocol types**: JSON-RPC messages, tool definitions, resources
//! - **Transport layer**: stdio (local processes), HTTP+SSE (remote servers)
//! - **Client**: High-level API for MCP server communication
//! - **Manager**: Lifecycle management for multiple MCP servers
//! - **Tools integration**: Automatic registration of MCP tools to ToolRegistry
//!
//! ## Usage
//!
//! ```rust,ignore
//! use a3s_code::mcp::{McpManager, McpServerConfig, McpTransportConfig};
//!
//! // Create manager
//! let manager = McpManager::new();
//!
//! // Register server
//! let config = McpServerConfig {
//!     name: "github".to_string(),
//!     transport: McpTransportConfig::Stdio {
//!         command: "npx".to_string(),
//!         args: vec!["-y".to_string(), "@modelcontextprotocol/server-github".to_string()],
//!     },
//!     enabled: true,
//!     env: [("GITHUB_TOKEN".to_string(), "...".to_string())].into(),
//!     oauth: None,
//! };
//! manager.register_server(config).await;
//!
//! // Connect
//! manager.connect("github").await?;
//!
//! // Get tools
//! let tools = manager.get_all_tools().await;
//! // tools: [("mcp__github__create_issue", McpTool { ... }), ...]
//!
//! // Call tool
//! let result = manager.call_tool("mcp__github__create_issue", Some(json!({
//!     "title": "Bug report",
//!     "body": "Description..."
//! }))).await?;
//! ```
//!
//! ## Tool Naming Convention
//!
//! MCP tools are registered with the prefix `mcp__<server>__<tool>`:
//!
//! | Full Name | Server | Tool |
//! |-----------|--------|------|
//! | `mcp__github__create_issue` | github | create_issue |
//! | `mcp__postgres__query` | postgres | query |

pub mod client;
pub mod manager;
pub mod protocol;
pub mod tools;
pub mod transport;

pub use client::McpClient;
pub use manager::{tool_result_to_string, McpManager, McpServerStatus};
pub use protocol::{
    CallToolResult, McpNotification, McpResource, McpServerConfig, McpTool, McpTransportConfig,
    OAuthConfig, ServerCapabilities, ToolContent,
};
pub use tools::{create_mcp_tools, McpToolWrapper};
