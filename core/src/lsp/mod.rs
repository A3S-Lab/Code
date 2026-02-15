//! LSP (Language Server Protocol) Support
//!
//! Provides integration with language servers for code intelligence features.
//!
//! ## Overview
//!
//! LSP is an open protocol for communication between code editors and language servers.
//! This module implements:
//!
//! - **Protocol types**: JSON-RPC messages, LSP types (hover, definition, references, etc.)
//! - **Client**: JSON-RPC client for language server communication
//! - **Manager**: Lifecycle management for multiple language servers
//! - **Tools**: LSP tools for agent use (lsp_hover, lsp_definition, etc.)
//!
//! ## Supported Language Servers
//!
//! | Language | Server | Command |
//! |----------|--------|---------|
//! | Rust | rust-analyzer | `rust-analyzer` |
//! | Go | gopls | `gopls` |
//! | TypeScript/JS | typescript-language-server | `typescript-language-server --stdio` |
//! | Python | pyright | `pyright-langserver --stdio` |
//! | C/C++ | clangd | `clangd` |
//!
//! ## Usage
//!
//! ```rust,ignore
//! use a3s_code::lsp::{LspManager, create_lsp_tools};
//!
//! // Create manager
//! let manager = Arc::new(LspManager::new());
//! manager.set_workspace("/workspace").await;
//!
//! // Start server for a language
//! manager.start_server("rust").await?;
//!
//! // Or auto-start based on file
//! let client = manager.ensure_server_for_file(Path::new("src/main.rs")).await?;
//!
//! // Use LSP features
//! let hover = client.hover("file:///workspace/src/main.rs", 10, 5).await?;
//! let definition = client.goto_definition("file:///workspace/src/main.rs", 10, 5).await?;
//! let references = client.find_references("file:///workspace/src/main.rs", 10, 5, false).await?;
//!
//! // Create tools for agent
//! let tools = create_lsp_tools(manager.clone());
//! ```
//!
//! ## LSP Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | `lsp_hover` | Get type info and documentation |
//! | `lsp_definition` | Jump to symbol definition |
//! | `lsp_references` | Find all usages of a symbol |
//! | `lsp_symbols` | Search workspace symbols |
//! | `lsp_diagnostics` | Get errors and warnings |

pub mod client;
pub mod manager;
pub mod protocol;
pub mod servers;
pub mod tools;

pub use client::LspClient;
pub use manager::{LspManager, LspServerStatus};
pub use protocol::{
    Diagnostic, DiagnosticSeverity, DocumentSymbol, GotoDefinitionResponse, Hover, Location,
    Position, Range, SymbolInformation, SymbolKind,
};
pub use servers::{
    builtin_servers, language_for_extension, language_id_for_extension, LanguageServerConfig,
};
pub use tools::create_lsp_tools;
