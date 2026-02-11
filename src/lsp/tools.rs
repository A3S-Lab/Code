//! LSP Tools
//!
//! Tool implementations for LSP features.

use crate::lsp::manager::LspManager;
use crate::lsp::protocol::*;
use crate::lsp::servers::language_id_for_extension;
use crate::tools::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

/// LSP hover tool
pub struct LspHoverTool {
    manager: Arc<LspManager>,
}

impl LspHoverTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
struct HoverParams {
    file_path: String,
    line: u32,
    column: u32,
}

#[async_trait]
impl Tool for LspHoverTool {
    fn name(&self) -> &str {
        "lsp_hover"
    }

    fn description(&self) -> &str {
        "Get type information and documentation for a symbol at a specific position"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (0-indexed)"
                },
                "column": {
                    "type": "integer",
                    "description": "Column number (0-indexed)"
                }
            },
            "required": ["file_path", "line", "column"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: HoverParams = serde_json::from_value(args.clone())?;

        // Resolve path
        let path = ctx.resolve_path(&params.file_path)?;
        let uri = format!("file://{}", path.display());

        // Get client
        let client = match self.manager.ensure_server_for_file(&path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("LSP not available: {}", e))),
        };

        // Open document if needed
        if let Ok(content) = std::fs::read_to_string(&path) {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let lang_id = language_id_for_extension(ext).unwrap_or("text");
            let _ = client.did_open(&uri, lang_id, &content).await;
        }

        // Get hover
        match client.hover(&uri, params.line, params.column).await {
            Ok(Some(hover)) => {
                let content = format_hover_contents(&hover.contents);
                Ok(ToolOutput::success(content))
            }
            Ok(None) => Ok(ToolOutput::success("No hover information available")),
            Err(e) => Ok(ToolOutput::error(format!("Hover failed: {}", e))),
        }
    }
}

/// LSP definition tool
pub struct LspDefinitionTool {
    manager: Arc<LspManager>,
}

impl LspDefinitionTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for LspDefinitionTool {
    fn name(&self) -> &str {
        "lsp_definition"
    }

    fn description(&self) -> &str {
        "Jump to the definition of a symbol at a specific position"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (0-indexed)"
                },
                "column": {
                    "type": "integer",
                    "description": "Column number (0-indexed)"
                }
            },
            "required": ["file_path", "line", "column"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: HoverParams = serde_json::from_value(args.clone())?;

        let path = ctx.resolve_path(&params.file_path)?;
        let uri = format!("file://{}", path.display());

        let client = match self.manager.ensure_server_for_file(&path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("LSP not available: {}", e))),
        };

        // Open document
        if let Ok(content) = std::fs::read_to_string(&path) {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let lang_id = language_id_for_extension(ext).unwrap_or("text");
            let _ = client.did_open(&uri, lang_id, &content).await;
        }

        match client
            .goto_definition(&uri, params.line, params.column)
            .await
        {
            Ok(Some(response)) => {
                let output = format_definition_response(&response);
                Ok(ToolOutput::success(output))
            }
            Ok(None) => Ok(ToolOutput::success("No definition found")),
            Err(e) => Ok(ToolOutput::error(format!("Definition failed: {}", e))),
        }
    }
}

/// LSP references tool
pub struct LspReferencesTool {
    manager: Arc<LspManager>,
}

impl LspReferencesTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
struct ReferencesParams {
    file_path: String,
    line: u32,
    column: u32,
    #[serde(default)]
    include_declaration: bool,
}

#[async_trait]
impl Tool for LspReferencesTool {
    fn name(&self) -> &str {
        "lsp_references"
    }

    fn description(&self) -> &str {
        "Find all references to a symbol at a specific position"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (0-indexed)"
                },
                "column": {
                    "type": "integer",
                    "description": "Column number (0-indexed)"
                },
                "include_declaration": {
                    "type": "boolean",
                    "description": "Include the declaration in results",
                    "default": false
                }
            },
            "required": ["file_path", "line", "column"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: ReferencesParams = serde_json::from_value(args.clone())?;

        let path = ctx.resolve_path(&params.file_path)?;
        let uri = format!("file://{}", path.display());

        let client = match self.manager.ensure_server_for_file(&path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("LSP not available: {}", e))),
        };

        // Open document
        if let Ok(content) = std::fs::read_to_string(&path) {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let lang_id = language_id_for_extension(ext).unwrap_or("text");
            let _ = client.did_open(&uri, lang_id, &content).await;
        }

        match client
            .find_references(&uri, params.line, params.column, params.include_declaration)
            .await
        {
            Ok(locations) => {
                if locations.is_empty() {
                    Ok(ToolOutput::success("No references found"))
                } else {
                    let output = format_locations(&locations);
                    Ok(ToolOutput::success(output))
                }
            }
            Err(e) => Ok(ToolOutput::error(format!("References failed: {}", e))),
        }
    }
}

/// LSP workspace symbols tool
pub struct LspSymbolsTool {
    manager: Arc<LspManager>,
}

impl LspSymbolsTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
struct SymbolsParams {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

#[async_trait]
impl Tool for LspSymbolsTool {
    fn name(&self) -> &str {
        "lsp_symbols"
    }

    fn description(&self) -> &str {
        "Search for symbols in the workspace"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for symbol names"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results",
                    "default": 20
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let params: SymbolsParams = serde_json::from_value(args.clone())?;

        // Get all running servers
        let running = self.manager.list_running().await;

        if running.is_empty() {
            return Ok(ToolOutput::error(
                "No LSP servers running. Start a server first by opening a file.",
            ));
        }

        let mut all_symbols = Vec::new();

        for language in running {
            if let Some(client) = self.manager.get_client(&language).await {
                if let Ok(symbols) = client.workspace_symbols(&params.query).await {
                    all_symbols.extend(symbols);
                }
            }
        }

        if all_symbols.is_empty() {
            Ok(ToolOutput::success(format!(
                "No symbols found matching '{}'",
                params.query
            )))
        } else {
            // Limit results
            all_symbols.truncate(params.limit);
            let output = format_symbol_information(&all_symbols);
            Ok(ToolOutput::success(output))
        }
    }
}

/// LSP diagnostics tool
pub struct LspDiagnosticsTool {
    manager: Arc<LspManager>,
}

impl LspDiagnosticsTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
struct DiagnosticsParams {
    #[serde(default)]
    file_path: Option<String>,
}

#[async_trait]
impl Tool for LspDiagnosticsTool {
    fn name(&self) -> &str {
        "lsp_diagnostics"
    }

    fn description(&self) -> &str {
        "Get diagnostics (errors, warnings) for a file"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file (optional, returns all if not specified)"
                }
            }
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: DiagnosticsParams = serde_json::from_value(args.clone())?;

        if let Some(file_path) = params.file_path {
            let path = ctx.resolve_path(&file_path)?;
            let uri = format!("file://{}", path.display());

            let client = match self.manager.ensure_server_for_file(&path).await {
                Ok(c) => c,
                Err(e) => return Ok(ToolOutput::error(format!("LSP not available: {}", e))),
            };

            let diagnostics = client.get_diagnostics(&uri).await;

            if diagnostics.is_empty() {
                Ok(ToolOutput::success("No diagnostics"))
            } else {
                let output = format_diagnostics(&diagnostics);
                Ok(ToolOutput::success(output))
            }
        } else {
            Ok(ToolOutput::success(
                "Specify a file_path to get diagnostics for a specific file",
            ))
        }
    }
}

// ============================================================================
// Formatting Helpers
// ============================================================================

fn format_hover_contents(contents: &HoverContents) -> String {
    match contents {
        HoverContents::Scalar(marked) => format_marked_string(marked),
        HoverContents::Array(items) => items
            .iter()
            .map(format_marked_string)
            .collect::<Vec<_>>()
            .join("\n\n"),
        HoverContents::Markup(markup) => markup.value.clone(),
    }
}

fn format_marked_string(marked: &MarkedString) -> String {
    match marked {
        MarkedString::String(s) => s.clone(),
        MarkedString::LanguageString { language, value } => {
            format!("```{}\n{}\n```", language, value)
        }
    }
}

fn format_definition_response(response: &GotoDefinitionResponse) -> String {
    match response {
        GotoDefinitionResponse::Scalar(loc) => format_location(loc),
        GotoDefinitionResponse::Array(locs) => {
            if locs.is_empty() {
                "No definition found".to_string()
            } else {
                locs.iter()
                    .map(format_location)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        GotoDefinitionResponse::Link(links) => links
            .iter()
            .map(|link| {
                format!(
                    "{}:{}:{}",
                    uri_to_path(&link.target_uri),
                    link.target_selection_range.start.line + 1,
                    link.target_selection_range.start.character + 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn format_location(loc: &Location) -> String {
    format!(
        "{}:{}:{}",
        uri_to_path(&loc.uri),
        loc.range.start.line + 1,
        loc.range.start.character + 1
    )
}

fn format_locations(locations: &[Location]) -> String {
    let mut output = format!("Found {} references:\n", locations.len());
    for loc in locations {
        output.push_str(&format!("  {}\n", format_location(loc)));
    }
    output
}

fn format_symbol_information(symbols: &[SymbolInformation]) -> String {
    let mut output = format!("Found {} symbols:\n", symbols.len());
    for sym in symbols {
        let kind = format!("{:?}", sym.kind);
        output.push_str(&format!(
            "  {} ({}) - {}\n",
            sym.name,
            kind,
            format_location(&sym.location)
        ));
    }
    output
}

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    let mut output = format!("Found {} diagnostics:\n", diagnostics.len());
    for diag in diagnostics {
        let severity = match diag.severity {
            Some(DiagnosticSeverity::Error) => "ERROR",
            Some(DiagnosticSeverity::Warning) => "WARNING",
            Some(DiagnosticSeverity::Information) => "INFO",
            Some(DiagnosticSeverity::Hint) => "HINT",
            None => "UNKNOWN",
        };
        output.push_str(&format!(
            "  [{}] Line {}: {}\n",
            severity,
            diag.range.start.line + 1,
            diag.message
        ));
    }
    output
}

fn uri_to_path(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

/// Create all LSP tools
pub fn create_lsp_tools(manager: Arc<LspManager>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(LspHoverTool::new(manager.clone())) as Arc<dyn Tool>,
        Arc::new(LspDefinitionTool::new(manager.clone())),
        Arc::new(LspReferencesTool::new(manager.clone())),
        Arc::new(LspSymbolsTool::new(manager.clone())),
        Arc::new(LspDiagnosticsTool::new(manager)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_location() {
        let loc = Location {
            uri: "file:///workspace/src/main.rs".to_string(),
            range: Range::new(Position::new(9, 4), Position::new(9, 10)),
        };
        let formatted = format_location(&loc);
        assert_eq!(formatted, "/workspace/src/main.rs:10:5");
    }

    #[test]
    fn test_format_hover_markup() {
        let contents = HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: "```rust\nfn main() {}\n```".to_string(),
        });
        let formatted = format_hover_contents(&contents);
        assert!(formatted.contains("fn main()"));
    }

    #[test]
    fn test_uri_to_path() {
        assert_eq!(
            uri_to_path("file:///workspace/src/main.rs"),
            "/workspace/src/main.rs"
        );
        assert_eq!(
            uri_to_path("/workspace/src/main.rs"),
            "/workspace/src/main.rs"
        );
    }

    #[test]
    fn test_lsp_hover_tool_name() {
        let manager = Arc::new(LspManager::new());
        let tool = LspHoverTool::new(manager);
        assert_eq!(tool.name(), "lsp_hover");
    }

    #[test]
    fn test_lsp_hover_tool_description() {
        let manager = Arc::new(LspManager::new());
        let tool = LspHoverTool::new(manager);
        assert_eq!(
            tool.description(),
            "Get type information and documentation for a symbol at a specific position"
        );
    }

    #[test]
    fn test_lsp_hover_tool_parameters() {
        let manager = Arc::new(LspManager::new());
        let tool = LspHoverTool::new(manager);
        let params = tool.parameters();
        
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["file_path"].is_object());
        assert!(params["properties"]["line"].is_object());
        assert!(params["properties"]["column"].is_object());
        assert_eq!(params["required"][0], "file_path");
        assert_eq!(params["required"][1], "line");
        assert_eq!(params["required"][2], "column");
    }

    #[test]
    fn test_hover_params_deserialization() {
        let json = serde_json::json!({
            "file_path": "/path/to/file.rs",
            "line": 10,
            "column": 5
        });
        let params: HoverParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.file_path, "/path/to/file.rs");
        assert_eq!(params.line, 10);
        assert_eq!(params.column, 5);
    }

    #[test]
    fn test_lsp_definition_tool_name() {
        let manager = Arc::new(LspManager::new());
        let tool = LspDefinitionTool::new(manager);
        assert_eq!(tool.name(), "lsp_definition");
    }

    #[test]
    fn test_lsp_definition_tool_description() {
        let manager = Arc::new(LspManager::new());
        let tool = LspDefinitionTool::new(manager);
        assert_eq!(
            tool.description(),
            "Jump to the definition of a symbol at a specific position"
        );
    }

    #[test]
    fn test_lsp_definition_tool_parameters() {
        let manager = Arc::new(LspManager::new());
        let tool = LspDefinitionTool::new(manager);
        let params = tool.parameters();
        
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["file_path"].is_object());
        assert!(params["properties"]["line"].is_object());
        assert!(params["properties"]["column"].is_object());
        assert_eq!(params["required"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_definition_params_deserialization() {
        let json = serde_json::json!({
            "file_path": "/src/main.rs",
            "line": 20,
            "column": 15
        });
        let params: HoverParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.file_path, "/src/main.rs");
        assert_eq!(params.line, 20);
        assert_eq!(params.column, 15);
    }

    #[test]
    fn test_lsp_references_tool_name() {
        let manager = Arc::new(LspManager::new());
        let tool = LspReferencesTool::new(manager);
        assert_eq!(tool.name(), "lsp_references");
    }

    #[test]
    fn test_lsp_references_tool_description() {
        let manager = Arc::new(LspManager::new());
        let tool = LspReferencesTool::new(manager);
        assert_eq!(
            tool.description(),
            "Find all references to a symbol at a specific position"
        );
    }

    #[test]
    fn test_lsp_references_tool_parameters() {
        let manager = Arc::new(LspManager::new());
        let tool = LspReferencesTool::new(manager);
        let params = tool.parameters();
        
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["file_path"].is_object());
        assert!(params["properties"]["line"].is_object());
        assert!(params["properties"]["column"].is_object());
        assert!(params["properties"]["include_declaration"].is_object());
        assert_eq!(params["properties"]["include_declaration"]["default"], false);
    }

    #[test]
    fn test_references_params_deserialization() {
        let json = serde_json::json!({
            "file_path": "/lib/utils.rs",
            "line": 5,
            "column": 8,
            "include_declaration": true
        });
        let params: ReferencesParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.file_path, "/lib/utils.rs");
        assert_eq!(params.line, 5);
        assert_eq!(params.column, 8);
        assert!(params.include_declaration);
    }

    #[test]
    fn test_references_params_deserialization_default() {
        let json = serde_json::json!({
            "file_path": "/lib/utils.rs",
            "line": 5,
            "column": 8
        });
        let params: ReferencesParams = serde_json::from_value(json).unwrap();
        assert!(!params.include_declaration);
    }

    #[test]
    fn test_lsp_symbols_tool_name() {
        let manager = Arc::new(LspManager::new());
        let tool = LspSymbolsTool::new(manager);
        assert_eq!(tool.name(), "lsp_symbols");
    }

    #[test]
    fn test_lsp_symbols_tool_description() {
        let manager = Arc::new(LspManager::new());
        let tool = LspSymbolsTool::new(manager);
        assert_eq!(tool.description(), "Search for symbols in the workspace");
    }

    #[test]
    fn test_lsp_symbols_tool_parameters() {
        let manager = Arc::new(LspManager::new());
        let tool = LspSymbolsTool::new(manager);
        let params = tool.parameters();
        
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["query"].is_object());
        assert!(params["properties"]["limit"].is_object());
        assert_eq!(params["properties"]["limit"]["default"], 20);
        assert_eq!(params["required"][0], "query");
    }

    #[test]
    fn test_symbols_params_deserialization() {
        let json = serde_json::json!({
            "query": "MyFunction",
            "limit": 50
        });
        let params: SymbolsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.query, "MyFunction");
        assert_eq!(params.limit, 50);
    }

    #[test]
    fn test_symbols_params_deserialization_default_limit() {
        let json = serde_json::json!({
            "query": "MyFunction"
        });
        let params: SymbolsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.query, "MyFunction");
        assert_eq!(params.limit, 20);
    }

    #[test]
    fn test_lsp_diagnostics_tool_name() {
        let manager = Arc::new(LspManager::new());
        let tool = LspDiagnosticsTool::new(manager);
        assert_eq!(tool.name(), "lsp_diagnostics");
    }

    #[test]
    fn test_lsp_diagnostics_tool_description() {
        let manager = Arc::new(LspManager::new());
        let tool = LspDiagnosticsTool::new(manager);
        assert_eq!(
            tool.description(),
            "Get diagnostics (errors, warnings) for a file"
        );
    }

    #[test]
    fn test_lsp_diagnostics_tool_parameters() {
        let manager = Arc::new(LspManager::new());
        let tool = LspDiagnosticsTool::new(manager);
        let params = tool.parameters();
        
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["file_path"].is_object());
        assert!(params["required"].is_null() || params["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_diagnostics_params_deserialization() {
        let json = serde_json::json!({
            "file_path": "/src/lib.rs"
        });
        let params: DiagnosticsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.file_path, Some("/src/lib.rs".to_string()));
    }

    #[test]
    fn test_diagnostics_params_deserialization_no_file() {
        let json = serde_json::json!({});
        let params: DiagnosticsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.file_path, None);
    }

    #[test]
    fn test_create_lsp_tools() {
        let manager = Arc::new(LspManager::new());
        let tools = create_lsp_tools(manager);

        assert_eq!(tools.len(), 5);
        assert_eq!(tools[0].name(), "lsp_hover");
        assert_eq!(tools[1].name(), "lsp_definition");
        assert_eq!(tools[2].name(), "lsp_references");
        assert_eq!(tools[3].name(), "lsp_symbols");
        assert_eq!(tools[4].name(), "lsp_diagnostics");
    }

    #[test]
    fn test_format_marked_string_plain() {
        let marked = MarkedString::String("hello world".to_string());
        assert_eq!(format_marked_string(&marked), "hello world");
    }

    #[test]
    fn test_format_marked_string_language() {
        let marked = MarkedString::LanguageString {
            language: "rust".to_string(),
            value: "fn main() {}".to_string(),
        };
        let result = format_marked_string(&marked);
        assert!(result.contains("```rust"));
        assert!(result.contains("fn main() {}"));
    }

    #[test]
    fn test_format_definition_response_scalar() {
        let loc = Location {
            uri: "file:///src/main.rs".to_string(),
            range: Range::new(Position::new(4, 0), Position::new(4, 10)),
        };
        let resp = GotoDefinitionResponse::Scalar(loc);
        let result = format_definition_response(&resp);
        assert!(result.contains("/src/main.rs:5:1"));
    }

    #[test]
    fn test_format_definition_response_array_empty() {
        let resp = GotoDefinitionResponse::Array(vec![]);
        assert_eq!(format_definition_response(&resp), "No definition found");
    }

    #[test]
    fn test_format_definition_response_array_multiple() {
        let locs = vec![
            Location {
                uri: "file:///a.rs".to_string(),
                range: Range::new(Position::new(0, 0), Position::new(0, 5)),
            },
            Location {
                uri: "file:///b.rs".to_string(),
                range: Range::new(Position::new(9, 2), Position::new(9, 8)),
            },
        ];
        let resp = GotoDefinitionResponse::Array(locs);
        let result = format_definition_response(&resp);
        assert!(result.contains("/a.rs:1:1"));
        assert!(result.contains("/b.rs:10:3"));
    }

    #[test]
    fn test_format_definition_response_link() {
        let links = vec![LocationLink {
            origin_selection_range: None,
            target_uri: "file:///target.rs".to_string(),
            target_range: Range::new(Position::new(0, 0), Position::new(0, 10)),
            target_selection_range: Range::new(Position::new(5, 3), Position::new(5, 10)),
        }];
        let resp = GotoDefinitionResponse::Link(links);
        let result = format_definition_response(&resp);
        assert!(result.contains("/target.rs:6:4"));
    }

    #[test]
    fn test_format_locations() {
        let locs = vec![
            Location {
                uri: "file:///a.rs".to_string(),
                range: Range::new(Position::new(0, 0), Position::new(0, 5)),
            },
            Location {
                uri: "file:///b.rs".to_string(),
                range: Range::new(Position::new(4, 2), Position::new(4, 8)),
            },
        ];
        let result = format_locations(&locs);
        assert!(result.contains("Found 2 references"));
        assert!(result.contains("/a.rs:1:1"));
        assert!(result.contains("/b.rs:5:3"));
    }

    #[test]
    fn test_format_symbol_information() {
        let symbols = vec![SymbolInformation {
            name: "MyStruct".to_string(),
            kind: SymbolKind::Class,
            location: Location {
                uri: "file:///lib.rs".to_string(),
                range: Range::new(Position::new(9, 0), Position::new(9, 15)),
            },
            container_name: None,
        }];
        let result = format_symbol_information(&symbols);
        assert!(result.contains("Found 1 symbols"));
        assert!(result.contains("MyStruct"));
        assert!(result.contains("/lib.rs:10:1"));
    }

    #[test]
    fn test_format_diagnostics_all_severities() {
        let diagnostics = vec![
            Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 5)),
                severity: Some(DiagnosticSeverity::Error),
                message: "error msg".to_string(),
                code: None,
                source: None,
            },
            Diagnostic {
                range: Range::new(Position::new(1, 0), Position::new(1, 5)),
                severity: Some(DiagnosticSeverity::Warning),
                message: "warn msg".to_string(),
                code: None,
                source: None,
            },
            Diagnostic {
                range: Range::new(Position::new(2, 0), Position::new(2, 5)),
                severity: Some(DiagnosticSeverity::Information),
                message: "info msg".to_string(),
                code: None,
                source: None,
            },
            Diagnostic {
                range: Range::new(Position::new(3, 0), Position::new(3, 5)),
                severity: Some(DiagnosticSeverity::Hint),
                message: "hint msg".to_string(),
                code: None,
                source: None,
            },
            Diagnostic {
                range: Range::new(Position::new(4, 0), Position::new(4, 5)),
                severity: None,
                message: "unknown msg".to_string(),
                code: None,
                source: None,
            },
        ];
        let result = format_diagnostics(&diagnostics);
        assert!(result.contains("Found 5 diagnostics"));
        assert!(result.contains("[ERROR] Line 1: error msg"));
        assert!(result.contains("[WARNING] Line 2: warn msg"));
        assert!(result.contains("[INFO] Line 3: info msg"));
        assert!(result.contains("[HINT] Line 4: hint msg"));
        assert!(result.contains("[UNKNOWN] Line 5: unknown msg"));
    }

    #[test]
    fn test_format_hover_contents_scalar_string() {
        let contents = HoverContents::Scalar(MarkedString::String("scalar text".to_string()));
        assert_eq!(format_hover_contents(&contents), "scalar text");
    }

    #[test]
    fn test_format_hover_contents_array() {
        let contents = HoverContents::Array(vec![
            MarkedString::String("first".to_string()),
            MarkedString::String("second".to_string()),
        ]);
        let result = format_hover_contents(&contents);
        assert!(result.contains("first"));
        assert!(result.contains("second"));
    }
}
