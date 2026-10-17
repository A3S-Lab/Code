use std::sync::Arc;

use serde_json::{json, Value};

use crate::code_intelligence::{
    CodeDiagnostic, CodeDiagnosticSeverity, CodeLocation, CodePosition, CodeQueryResult, CodeRange,
    CodeSymbolKind, DocumentSnapshot, DocumentSymbol, SymbolInformation,
};
use crate::tools::ToolContext;
use crate::workspace::{WorkspacePath, WorkspaceTextReader};

const DECLARATION_LINE_BYTES: usize = 256;

pub(super) fn document_symbols(result: CodeQueryResult<DocumentSymbol>) -> Value {
    query_result(result, document_symbol)
}

pub(super) fn workspace_symbols(
    result: CodeQueryResult<SymbolInformation>,
    declaration_lines: Vec<Option<String>>,
) -> Value {
    let mut declaration_lines = declaration_lines.into_iter();
    query_result(result, |symbol| {
        symbol_information(symbol, declaration_lines.next().flatten())
    })
}

pub(super) fn locations(
    result: CodeQueryResult<CodeLocation>,
    declaration_lines: Vec<Option<String>>,
) -> Value {
    let mut declaration_lines = declaration_lines.into_iter();
    query_result(result, |item| {
        located(item, declaration_lines.next().flatten())
    })
}

pub(super) async fn saved_declaration_lines(
    ctx: &ToolContext,
    locations: Vec<(WorkspacePath, u32)>,
) -> Vec<Option<String>> {
    let Some(reader) = ctx.workspace_services.text_reader() else {
        return locations.into_iter().map(|_| None).collect();
    };
    let mut lines = Vec::new();
    for (path, line) in locations {
        lines.push(one_saved_line(&reader, &path, line).await);
    }
    lines
}

async fn one_saved_line(
    reader: &Arc<dyn WorkspaceTextReader>,
    path: &WorkspacePath,
    line: u32,
) -> Option<String> {
    let range = reader
        .read_text_range(path, usize::try_from(line).ok()?, 1)
        .await
        .ok()?;
    range
        .lines
        .into_iter()
        .next()
        .map(|text| truncate_to_bytes(&text, DECLARATION_LINE_BYTES))
}

fn truncate_to_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

pub(super) fn diagnostics(result: CodeQueryResult<CodeDiagnostic>) -> Value {
    query_result(result, diagnostic)
}

fn query_result<T>(result: CodeQueryResult<T>, map: impl FnMut(T) -> Value) -> Value {
    json!({
        "items": result.items.into_iter().map(map).collect::<Vec<_>>(),
        "truncated": result.truncated,
        "workspace_revision": result.workspace_revision,
        "document": result.document.map(snapshot),
    })
}

fn snapshot(snapshot: DocumentSnapshot) -> Value {
    json!({
        "revision": snapshot.revision.value(),
        "content_hash": snapshot.content_hash,
        "stale": snapshot.stale,
    })
}

fn document_symbol(symbol: DocumentSymbol) -> Value {
    json!({
        "name": symbol.name,
        "detail": symbol.detail,
        "kind": symbol_kind(symbol.kind),
        "range": range(symbol.range),
        "selection_range": range(symbol.selection_range),
        "children": symbol.children.into_iter().map(document_symbol).collect::<Vec<_>>(),
    })
}

fn symbol_information(symbol: SymbolInformation, declaration_line: Option<String>) -> Value {
    let mut value = json!({
        "name": symbol.name,
        "kind": symbol_kind(symbol.kind),
        "location": location(symbol.location),
        "container_name": symbol.container_name,
    });
    if let Some(line) = declaration_line {
        value["declaration_line"] = Value::String(line);
    }
    value
}

fn located(item: CodeLocation, declaration_line: Option<String>) -> Value {
    let mut value = location(item);
    if let Some(line) = declaration_line {
        value["declaration_line"] = Value::String(line);
    }
    value
}

fn diagnostic(diagnostic: CodeDiagnostic) -> Value {
    json!({
        "location": location(diagnostic.location),
        "severity": diagnostic.severity.map(diagnostic_severity),
        "code": diagnostic.code,
        "source": diagnostic.source,
        "message": diagnostic.message,
    })
}

fn location(location: CodeLocation) -> Value {
    json!({
        "path": location.path.as_str(),
        "range": range(location.range),
    })
}

fn range(range: CodeRange) -> Value {
    json!({
        "start": position(range.start),
        "end": position(range.end),
    })
}

fn position(position: CodePosition) -> Value {
    json!({
        "line": position.line,
        "character": position.character,
    })
}

fn diagnostic_severity(severity: CodeDiagnosticSeverity) -> &'static str {
    match severity {
        CodeDiagnosticSeverity::Error => "error",
        CodeDiagnosticSeverity::Warning => "warning",
        CodeDiagnosticSeverity::Information => "information",
        CodeDiagnosticSeverity::Hint => "hint",
    }
}

fn symbol_kind(kind: CodeSymbolKind) -> &'static str {
    match kind {
        CodeSymbolKind::File => "file",
        CodeSymbolKind::Module => "module",
        CodeSymbolKind::Namespace => "namespace",
        CodeSymbolKind::Package => "package",
        CodeSymbolKind::Class => "class",
        CodeSymbolKind::Method => "method",
        CodeSymbolKind::Property => "property",
        CodeSymbolKind::Field => "field",
        CodeSymbolKind::Constructor => "constructor",
        CodeSymbolKind::Enum => "enum",
        CodeSymbolKind::Interface => "interface",
        CodeSymbolKind::Function => "function",
        CodeSymbolKind::Variable => "variable",
        CodeSymbolKind::Constant => "constant",
        CodeSymbolKind::String => "string",
        CodeSymbolKind::Number => "number",
        CodeSymbolKind::Boolean => "boolean",
        CodeSymbolKind::Array => "array",
        CodeSymbolKind::Object => "object",
        CodeSymbolKind::Key => "key",
        CodeSymbolKind::Null => "null",
        CodeSymbolKind::EnumMember => "enum_member",
        CodeSymbolKind::Struct => "struct",
        CodeSymbolKind::Event => "event",
        CodeSymbolKind::Operator => "operator",
        CodeSymbolKind::TypeParameter => "type_parameter",
        CodeSymbolKind::Unknown => "unknown",
    }
}
