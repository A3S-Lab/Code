use serde_json::{json, Value};

use crate::code_intelligence::{
    CodeDiagnostic, CodeDiagnosticSeverity, CodeLocation, CodePosition, CodeQueryResult, CodeRange,
    CodeSymbolKind, DocumentSnapshot, DocumentSymbol, SymbolInformation,
};

pub(super) fn document_symbols(result: CodeQueryResult<DocumentSymbol>) -> Value {
    query_result(result, document_symbol)
}

pub(super) fn workspace_symbols(result: CodeQueryResult<SymbolInformation>) -> Value {
    query_result(result, symbol_information)
}

pub(super) fn locations(result: CodeQueryResult<CodeLocation>) -> Value {
    query_result(result, location)
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

fn symbol_information(symbol: SymbolInformation) -> Value {
    json!({
        "name": symbol.name,
        "kind": symbol_kind(symbol.kind),
        "location": location(symbol.location),
        "container_name": symbol.container_name,
    })
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
