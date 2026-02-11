//! LSP Protocol Type Definitions
//!
//! Defines the core types for the Language Server Protocol (LSP).
//! Based on LSP 3.17 specification.

use serde::{Deserialize, Serialize};

// ============================================================================
// JSON-RPC Types
// ============================================================================

/// JSON-RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        }
    }
}

// ============================================================================
// LSP Initialize
// ============================================================================

/// Client capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_document: Option<TextDocumentClientCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceClientCapabilities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hover: Option<HoverClientCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<DefinitionClientCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<ReferenceClientCapabilities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoverClientCapabilities {
    #[serde(default)]
    pub dynamic_registration: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionClientCapabilities {
    #[serde(default)]
    pub dynamic_registration: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceClientCapabilities {
    #[serde(default)]
    pub dynamic_registration: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_folders: Option<bool>,
}

/// Initialize params
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub process_id: Option<u32>,
    pub root_uri: Option<String>,
    pub capabilities: ClientCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<serde_json::Value>,
}

/// Server capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hover_provider: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_provider: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references_provider: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_symbol_provider: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_symbol_provider: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_document_sync: Option<TextDocumentSyncKind>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum TextDocumentSyncKind {
    None = 0,
    Full = 1,
    Incremental = 2,
}

/// Initialize result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

// ============================================================================
// LSP Position and Location
// ============================================================================

/// Position in a text document (0-indexed)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// Range in a text document
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// Location in a document
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

/// Text document identifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// Text document position params
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentPositionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

// ============================================================================
// LSP Hover
// ============================================================================

/// Hover result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hover {
    pub contents: HoverContents,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

/// Hover contents
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HoverContents {
    Scalar(MarkedString),
    Array(Vec<MarkedString>),
    Markup(MarkupContent),
}

/// Marked string (deprecated but still used)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MarkedString {
    String(String),
    LanguageString { language: String, value: String },
}

/// Markup content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkupContent {
    pub kind: MarkupKind,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkupKind {
    PlainText,
    Markdown,
}

// ============================================================================
// LSP Definition
// ============================================================================

/// Definition response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GotoDefinitionResponse {
    Scalar(Location),
    Array(Vec<Location>),
    Link(Vec<LocationLink>),
}

/// Location link
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_selection_range: Option<Range>,
    pub target_uri: String,
    pub target_range: Range,
    pub target_selection_range: Range,
}

// ============================================================================
// LSP References
// ============================================================================

/// Reference params
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    pub context: ReferenceContext,
}

/// Reference context
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceContext {
    pub include_declaration: bool,
}

// ============================================================================
// LSP Symbols
// ============================================================================

/// Symbol kind
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum SymbolKind {
    File = 1,
    Module = 2,
    Namespace = 3,
    Package = 4,
    Class = 5,
    Method = 6,
    Property = 7,
    Field = 8,
    Constructor = 9,
    Enum = 10,
    Interface = 11,
    Function = 12,
    Variable = 13,
    Constant = 14,
    String = 15,
    Number = 16,
    Boolean = 17,
    Array = 18,
    Object = 19,
    Key = 20,
    Null = 21,
    EnumMember = 22,
    Struct = 23,
    Event = 24,
    Operator = 25,
    TypeParameter = 26,
}

/// Document symbol
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub kind: SymbolKind,
    pub range: Range,
    pub selection_range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<DocumentSymbol>>,
}

/// Symbol information (workspace symbols)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
    pub name: String,
    pub kind: SymbolKind,
    pub location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
}

/// Workspace symbol params
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSymbolParams {
    pub query: String,
}

// ============================================================================
// LSP Diagnostics
// ============================================================================

/// Diagnostic severity
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

/// Diagnostic
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<DiagnosticSeverity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<DiagnosticCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiagnosticCode {
    Number(i32),
    String(String),
}

/// Publish diagnostics params (notification from server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

// ============================================================================
// LSP Document Sync
// ============================================================================

/// Text document item
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

/// Did open params
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

/// Did close params
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

/// Versioned text document identifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i32,
}

/// Text document content change event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentContentChangeEvent {
    pub text: String,
}

/// Did change params
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

// ============================================================================
// LSP Notification Types
// ============================================================================

/// LSP notification from server
#[derive(Debug, Clone)]
pub enum LspNotification {
    PublishDiagnostics(PublishDiagnosticsParams),
    LogMessage {
        r#type: i32,
        message: String,
    },
    ShowMessage {
        r#type: i32,
        message: String,
    },
    Unknown {
        method: String,
        params: Option<serde_json::Value>,
    },
}

impl LspNotification {
    pub fn from_json_rpc(notification: &JsonRpcNotification) -> Self {
        match notification.method.as_str() {
            "textDocument/publishDiagnostics" => {
                if let Some(params) = &notification.params {
                    if let Ok(diag_params) = serde_json::from_value(params.clone()) {
                        return LspNotification::PublishDiagnostics(diag_params);
                    }
                }
                LspNotification::Unknown {
                    method: notification.method.clone(),
                    params: notification.params.clone(),
                }
            }
            "window/logMessage" => {
                if let Some(params) = &notification.params {
                    let r#type = params.get("type").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
                    let message = params
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    return LspNotification::LogMessage { r#type, message };
                }
                LspNotification::Unknown {
                    method: notification.method.clone(),
                    params: notification.params.clone(),
                }
            }
            "window/showMessage" => {
                if let Some(params) = &notification.params {
                    let r#type = params.get("type").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
                    let message = params
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    return LspNotification::ShowMessage { r#type, message };
                }
                LspNotification::Unknown {
                    method: notification.method.clone(),
                    params: notification.params.clone(),
                }
            }
            _ => LspNotification::Unknown {
                method: notification.method.clone(),
                params: notification.params.clone(),
            },
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position() {
        let pos = Position::new(10, 5);
        assert_eq!(pos.line, 10);
        assert_eq!(pos.character, 5);
    }

    #[test]
    fn test_range() {
        let range = Range::new(Position::new(0, 0), Position::new(10, 20));
        assert_eq!(range.start.line, 0);
        assert_eq!(range.end.line, 10);
    }

    #[test]
    fn test_json_rpc_request() {
        let req = JsonRpcRequest::new(1, "textDocument/hover", Some(serde_json::json!({})));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"textDocument/hover\""));
    }

    #[test]
    fn test_initialize_params() {
        let params = InitializeParams {
            process_id: Some(12345),
            root_uri: Some("file:///workspace".to_string()),
            capabilities: ClientCapabilities::default(),
            initialization_options: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("processId"));
        assert!(json.contains("rootUri"));
    }

    #[test]
    fn test_hover_contents_markup() {
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "```rust\nfn main() {}\n```".to_string(),
            }),
            range: None,
        };
        let json = serde_json::to_string(&hover).unwrap();
        assert!(json.contains("markdown"));
    }

    #[test]
    fn test_diagnostic() {
        let diag = Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 10)),
            severity: Some(DiagnosticSeverity::Error),
            code: Some(DiagnosticCode::String("E0001".to_string())),
            source: Some("rust-analyzer".to_string()),
            message: "expected type".to_string(),
        };
        let json = serde_json::to_string(&diag).unwrap();
        assert!(json.contains("expected type"));
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;

    #[test]
    fn test_json_rpc_notification_new() {
        let notif = JsonRpcNotification::new("test/method", Some(serde_json::json!({"key": "value"})));
        assert_eq!(notif.jsonrpc, "2.0");
        assert_eq!(notif.method, "test/method");
        assert!(notif.params.is_some());
    }

    #[test]
    fn test_json_rpc_notification_no_params() {
        let notif = JsonRpcNotification::new("test/method", None);
        assert_eq!(notif.method, "test/method");
        assert!(notif.params.is_none());
    }

    #[test]
    fn test_json_rpc_request_serialize() {
        let req = JsonRpcRequest::new(42, "initialize", None);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"method\":\"initialize\""));
    }

    #[test]
    fn test_json_rpc_response_with_result() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(serde_json::json!({"status": "ok"})),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_json_rpc_response_with_error() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid Request".to_string(),
                data: None,
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"error\""));
        assert!(json.contains("Invalid Request"));
    }

    #[test]
    fn test_position_equality() {
        let pos1 = Position::new(5, 10);
        let pos2 = Position::new(5, 10);
        let pos3 = Position::new(5, 11);
        assert_eq!(pos1, pos2);
        assert_ne!(pos1, pos3);
    }

    #[test]
    fn test_range_equality() {
        let range1 = Range::new(Position::new(0, 0), Position::new(1, 1));
        let range2 = Range::new(Position::new(0, 0), Position::new(1, 1));
        let range3 = Range::new(Position::new(0, 0), Position::new(2, 2));
        assert_eq!(range1, range2);
        assert_ne!(range1, range3);
    }

    #[test]
    fn test_location_serialize() {
        let loc = Location {
            uri: "file:///test.rs".to_string(),
            range: Range::new(Position::new(10, 5), Position::new(10, 15)),
        };
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("file:///test.rs"));
    }

    #[test]
    fn test_text_document_identifier() {
        let doc = TextDocumentIdentifier {
            uri: "file:///src/main.rs".to_string(),
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("file:///src/main.rs"));
    }

    #[test]
    fn test_text_document_position_params() {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///test.rs".to_string(),
            },
            position: Position::new(5, 10),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("textDocument"));
        assert!(json.contains("position"));
    }

    #[test]
    fn test_hover_with_range() {
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String("test".to_string())),
            range: Some(Range::new(Position::new(0, 0), Position::new(0, 5))),
        };
        let json = serde_json::to_string(&hover).unwrap();
        assert!(json.contains("range"));
    }

    #[test]
    fn test_hover_contents_scalar() {
        let contents = HoverContents::Scalar(MarkedString::String("hover text".to_string()));
        let json = serde_json::to_value(&contents).unwrap();
        assert!(json.is_string() || json.is_object());
    }

    #[test]
    fn test_hover_contents_array() {
        let contents = HoverContents::Array(vec![
            MarkedString::String("line1".to_string()),
            MarkedString::String("line2".to_string()),
        ]);
        let json = serde_json::to_value(&contents).unwrap();
        assert!(json.is_array());
    }

    #[test]
    fn test_marked_string_language() {
        let marked = MarkedString::LanguageString {
            language: "rust".to_string(),
            value: "fn main() {}".to_string(),
        };
        let json = serde_json::to_string(&marked).unwrap();
        assert!(json.contains("rust"));
        assert!(json.contains("fn main"));
    }

    #[test]
    fn test_markup_content_plaintext() {
        let markup = MarkupContent {
            kind: MarkupKind::PlainText,
            value: "plain text".to_string(),
        };
        let json = serde_json::to_string(&markup).unwrap();
        assert!(json.contains("plaintext"));
    }

    #[test]
    fn test_markup_content_markdown() {
        let markup = MarkupContent {
            kind: MarkupKind::Markdown,
            value: "# Header".to_string(),
        };
        let json = serde_json::to_string(&markup).unwrap();
        assert!(json.contains("markdown"));
    }

    #[test]
    fn test_goto_definition_scalar() {
        let response = GotoDefinitionResponse::Scalar(Location {
            uri: "file:///def.rs".to_string(),
            range: Range::new(Position::new(0, 0), Position::new(0, 10)),
        });
        let json = serde_json::to_value(&response).unwrap();
        assert!(json.is_object());
    }

    #[test]
    fn test_goto_definition_array() {
        let response = GotoDefinitionResponse::Array(vec![
            Location {
                uri: "file:///def1.rs".to_string(),
                range: Range::new(Position::new(0, 0), Position::new(0, 10)),
            },
            Location {
                uri: "file:///def2.rs".to_string(),
                range: Range::new(Position::new(5, 0), Position::new(5, 10)),
            },
        ]);
        let json = serde_json::to_value(&response).unwrap();
        assert!(json.is_array());
    }

    #[test]
    fn test_location_link() {
        let link = LocationLink {
            origin_selection_range: Some(Range::new(Position::new(0, 0), Position::new(0, 5))),
            target_uri: "file:///target.rs".to_string(),
            target_range: Range::new(Position::new(10, 0), Position::new(10, 20)),
            target_selection_range: Range::new(Position::new(10, 5), Position::new(10, 15)),
        };
        let json = serde_json::to_string(&link).unwrap();
        assert!(json.contains("targetUri"));
        assert!(json.contains("targetRange"));
    }

    #[test]
    fn test_reference_params() {
        let params = ReferenceParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///test.rs".to_string(),
            },
            position: Position::new(5, 10),
            context: ReferenceContext {
                include_declaration: true,
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("includeDeclaration"));
    }

    #[test]
    fn test_reference_context_serialize() {
        let ctx = ReferenceContext {
            include_declaration: false,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("false"));
    }

    #[test]
    fn test_document_symbol() {
        let symbol = DocumentSymbol {
            name: "test_function".to_string(),
            detail: Some("fn test_function()".to_string()),
            kind: SymbolKind::Function,
            range: Range::new(Position::new(0, 0), Position::new(10, 0)),
            selection_range: Range::new(Position::new(0, 3), Position::new(0, 16)),
            children: None,
        };
        let json = serde_json::to_string(&symbol).unwrap();
        assert!(json.contains("test_function"));
    }

    #[test]
    fn test_document_symbol_with_children() {
        let symbol = DocumentSymbol {
            name: "MyStruct".to_string(),
            detail: None,
            kind: SymbolKind::Struct,
            range: Range::new(Position::new(0, 0), Position::new(20, 0)),
            selection_range: Range::new(Position::new(0, 7), Position::new(0, 15)),
            children: Some(vec![DocumentSymbol {
                name: "field".to_string(),
                detail: Some("i32".to_string()),
                kind: SymbolKind::Field,
                range: Range::new(Position::new(1, 4), Position::new(1, 15)),
                selection_range: Range::new(Position::new(1, 4), Position::new(1, 9)),
                children: None,
            }]),
        };
        let json = serde_json::to_string(&symbol).unwrap();
        assert!(json.contains("MyStruct"));
        assert!(json.contains("field"));
    }

    #[test]
    fn test_symbol_information() {
        let info = SymbolInformation {
            name: "my_function".to_string(),
            kind: SymbolKind::Function,
            location: Location {
                uri: "file:///src/lib.rs".to_string(),
                range: Range::new(Position::new(5, 0), Position::new(10, 0)),
            },
            container_name: Some("my_module".to_string()),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("my_function"));
        assert!(json.contains("my_module"));
    }

    #[test]
    fn test_workspace_symbol_params() {
        let params = WorkspaceSymbolParams {
            query: "test".to_string(),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("test"));
    }

    #[test]
    fn test_diagnostic_severity_values() {
        assert_eq!(DiagnosticSeverity::Error as u8, 1);
        assert_eq!(DiagnosticSeverity::Warning as u8, 2);
        assert_eq!(DiagnosticSeverity::Information as u8, 3);
        assert_eq!(DiagnosticSeverity::Hint as u8, 4);
    }

    #[test]
    fn test_diagnostic_code_number() {
        let code = DiagnosticCode::Number(404);
        let json = serde_json::to_value(&code).unwrap();
        assert_eq!(json, 404);
    }

    #[test]
    fn test_diagnostic_code_string() {
        let code = DiagnosticCode::String("E0001".to_string());
        let json = serde_json::to_value(&code).unwrap();
        assert_eq!(json, "E0001");
    }

    #[test]
    fn test_publish_diagnostics_params() {
        let params = PublishDiagnosticsParams {
            uri: "file:///test.rs".to_string(),
            diagnostics: vec![Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 5)),
                severity: Some(DiagnosticSeverity::Warning),
                code: None,
                source: None,
                message: "unused variable".to_string(),
            }],
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("unused variable"));
    }

    #[test]
    fn test_text_document_item() {
        let item = TextDocumentItem {
            uri: "file:///test.rs".to_string(),
            language_id: "rust".to_string(),
            version: 1,
            text: "fn main() {}".to_string(),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("languageId"));
        assert!(json.contains("rust"));
    }

    #[test]
    fn test_did_open_text_document_params() {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: "file:///test.rs".to_string(),
                language_id: "rust".to_string(),
                version: 1,
                text: "fn main() {}".to_string(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("textDocument"));
    }

    #[test]
    fn test_did_close_text_document_params() {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///test.rs".to_string(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("textDocument"));
    }

    #[test]
    fn test_versioned_text_document_identifier() {
        let doc = VersionedTextDocumentIdentifier {
            uri: "file:///test.rs".to_string(),
            version: 5,
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"version\":5"));
    }

    #[test]
    fn test_text_document_content_change_event() {
        let change = TextDocumentContentChangeEvent {
            text: "new content".to_string(),
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("new content"));
    }

    #[test]
    fn test_did_change_text_document_params() {
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: "file:///test.rs".to_string(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                text: "updated text".to_string(),
            }],
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("contentChanges"));
    }

    #[test]
    fn test_lsp_notification_from_publish_diagnostics() {
        let json_notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "textDocument/publishDiagnostics".to_string(),
            params: Some(serde_json::json!({
                "uri": "file:///test.rs",
                "diagnostics": []
            })),
        };
        let notif = LspNotification::from_json_rpc(&json_notif);
        match notif {
            LspNotification::PublishDiagnostics(_) => {}
            _ => panic!("Expected PublishDiagnostics"),
        }
    }

    #[test]
    fn test_lsp_notification_from_log_message() {
        let json_notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "window/logMessage".to_string(),
            params: Some(serde_json::json!({
                "type": 1,
                "message": "Log message"
            })),
        };
        let notif = LspNotification::from_json_rpc(&json_notif);
        match notif {
            LspNotification::LogMessage { r#type, message } => {
                assert_eq!(r#type, 1);
                assert_eq!(message, "Log message");
            }
            _ => panic!("Expected LogMessage"),
        }
    }

    #[test]
    fn test_lsp_notification_from_show_message() {
        let json_notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "window/showMessage".to_string(),
            params: Some(serde_json::json!({
                "type": 2,
                "message": "Show message"
            })),
        };
        let notif = LspNotification::from_json_rpc(&json_notif);
        match notif {
            LspNotification::ShowMessage { r#type, message } => {
                assert_eq!(r#type, 2);
                assert_eq!(message, "Show message");
            }
            _ => panic!("Expected ShowMessage"),
        }
    }

    #[test]
    fn test_lsp_notification_unknown() {
        let json_notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "custom/method".to_string(),
            params: Some(serde_json::json!({"key": "value"})),
        };
        let notif = LspNotification::from_json_rpc(&json_notif);
        match notif {
            LspNotification::Unknown { method, params } => {
                assert_eq!(method, "custom/method");
                assert!(params.is_some());
            }
            _ => panic!("Expected Unknown"),
        }
    }

    #[test]
    fn test_server_capabilities_default() {
        let caps = ServerCapabilities::default();
        assert!(caps.hover_provider.is_none());
        assert!(caps.definition_provider.is_none());
    }

    #[test]
    fn test_initialize_result() {
        let result = InitializeResult {
            capabilities: ServerCapabilities::default(),
            server_info: Some(ServerInfo {
                name: "test-server".to_string(),
                version: Some("1.0.0".to_string()),
            }),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test-server"));
    }

    #[test]
    fn test_text_document_sync_kind_values() {
        assert_eq!(TextDocumentSyncKind::None as u8, 0);
        assert_eq!(TextDocumentSyncKind::Full as u8, 1);
        assert_eq!(TextDocumentSyncKind::Incremental as u8, 2);
    }

    #[test]
    fn test_symbol_kind_values() {
        assert_eq!(SymbolKind::File as u8, 1);
        assert_eq!(SymbolKind::Function as u8, 12);
        assert_eq!(SymbolKind::TypeParameter as u8, 26);
    }

    #[test]
    fn test_client_capabilities_default() {
        let caps = ClientCapabilities::default();
        assert!(caps.text_document.is_none());
        assert!(caps.workspace.is_none());
    }
}
