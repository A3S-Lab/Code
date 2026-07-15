use super::super::{
    CodeDiagnostic, CodeDiagnosticSeverity, CodeLocation, CodePosition, CodeRange, CodeSymbolKind,
    DocumentSymbol, SymbolInformation,
};
use crate::workspace::WorkspacePath;
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DocumentSymbolResponse, GotoDefinitionResponse, Location,
    LocationLink, NumberOrString, OneOf, SymbolKind, Uri, WorkspaceSymbol, WorkspaceSymbolResponse,
};
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use url::Url;

/// Failure while normalizing a language-server response into core types.
#[derive(Debug, Error)]
pub(crate) enum MappingError {
    #[error("workspace root must be an absolute canonical path: {root:?}")]
    InvalidWorkspaceRoot { root: PathBuf },

    #[error("unsupported resource URI scheme: {scheme}")]
    UnsupportedUriScheme { scheme: String },

    #[error("resource URI is not a valid local file URI: {uri}")]
    InvalidFileUri { uri: String },

    #[error("resource path does not exist: {path:?}")]
    PathDoesNotExist { path: PathBuf },

    #[error("resource path could not be resolved: {path:?}: {source}")]
    PathResolution {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("resource path is outside the workspace: {path:?}")]
    OutsideWorkspace { path: PathBuf },

    #[error("workspace-relative resource path is not valid UTF-8: {path:?}")]
    NonUtf8WorkspacePath { path: PathBuf },

    #[error("protocol range ends before it starts: {start:?}..{end:?}")]
    InvalidRange {
        start: CodePosition,
        end: CodePosition,
    },

    #[error("protocol selection range {selection:?} is outside enclosing range {enclosing:?}")]
    SelectionOutsideRange {
        selection: CodeRange,
        enclosing: CodeRange,
    },
}

pub(crate) fn map_position(position: lsp_types::Position) -> CodePosition {
    CodePosition::new(position.line, position.character)
}

pub(crate) fn map_range(range: lsp_types::Range) -> Result<CodeRange, MappingError> {
    let start = map_position(range.start);
    let end = map_position(range.end);
    if end < start {
        return Err(MappingError::InvalidRange { start, end });
    }

    Ok(CodeRange::new(start, end))
}

pub(crate) fn map_document_symbol(
    symbol: lsp_types::DocumentSymbol,
) -> Result<DocumentSymbol, MappingError> {
    let range = map_range(symbol.range)?;
    let selection_range = map_range(symbol.selection_range)?;
    ensure_range_contains(range, selection_range)?;
    let children = symbol
        .children
        .unwrap_or_default()
        .into_iter()
        .map(map_document_symbol)
        .collect::<Result<_, _>>()?;

    Ok(DocumentSymbol {
        name: symbol.name,
        detail: symbol.detail,
        kind: map_symbol_kind(symbol.kind),
        range,
        selection_range,
        children,
    })
}

pub(crate) async fn map_document_symbol_response(
    canonical_workspace_root: &Path,
    response: DocumentSymbolResponse,
) -> Result<Vec<DocumentSymbol>, MappingError> {
    match response {
        DocumentSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .map(map_document_symbol)
            .collect::<Result<_, _>>(),
        DocumentSymbolResponse::Flat(symbols) => {
            let mut mapped_symbols = Vec::with_capacity(symbols.len());
            for symbol in symbols {
                let mapped = map_symbol_information(canonical_workspace_root, symbol).await?;
                mapped_symbols.push(DocumentSymbol {
                    name: mapped.name,
                    detail: None,
                    kind: mapped.kind,
                    range: mapped.location.range,
                    selection_range: mapped.location.range,
                    children: Vec::new(),
                });
            }
            Ok(mapped_symbols)
        }
    }
}

pub(crate) async fn map_symbol_information(
    canonical_workspace_root: &Path,
    symbol: lsp_types::SymbolInformation,
) -> Result<SymbolInformation, MappingError> {
    Ok(SymbolInformation {
        name: symbol.name,
        kind: map_symbol_kind(symbol.kind),
        location: map_location(canonical_workspace_root, symbol.location).await?,
        container_name: symbol.container_name,
    })
}

pub(crate) async fn map_workspace_symbol(
    canonical_workspace_root: &Path,
    symbol: WorkspaceSymbol,
) -> Result<SymbolInformation, MappingError> {
    let location = match symbol.location {
        OneOf::Left(location) => map_location(canonical_workspace_root, location).await?,
        OneOf::Right(location) => CodeLocation {
            path: file_uri_to_workspace_path(canonical_workspace_root, &location.uri).await?,
            range: CodeRange::default(),
        },
    };

    Ok(SymbolInformation {
        name: symbol.name,
        kind: map_symbol_kind(symbol.kind),
        location,
        container_name: symbol.container_name,
    })
}

pub(crate) async fn map_workspace_symbol_response(
    canonical_workspace_root: &Path,
    response: WorkspaceSymbolResponse,
) -> Result<Vec<SymbolInformation>, MappingError> {
    let mut mapped_symbols = Vec::new();
    match response {
        WorkspaceSymbolResponse::Flat(symbols) => {
            mapped_symbols.reserve(symbols.len());
            for symbol in symbols {
                mapped_symbols
                    .push(map_symbol_information(canonical_workspace_root, symbol).await?);
            }
        }
        WorkspaceSymbolResponse::Nested(symbols) => {
            mapped_symbols.reserve(symbols.len());
            for symbol in symbols {
                mapped_symbols.push(map_workspace_symbol(canonical_workspace_root, symbol).await?);
            }
        }
    }
    Ok(mapped_symbols)
}

pub(crate) async fn map_location(
    canonical_workspace_root: &Path,
    location: Location,
) -> Result<CodeLocation, MappingError> {
    Ok(CodeLocation {
        path: file_uri_to_workspace_path(canonical_workspace_root, &location.uri).await?,
        range: map_range(location.range)?,
    })
}

pub(crate) async fn map_location_link(
    canonical_workspace_root: &Path,
    link: LocationLink,
) -> Result<CodeLocation, MappingError> {
    let target_range = map_range(link.target_range)?;
    let target_selection_range = map_range(link.target_selection_range)?;
    ensure_range_contains(target_range, target_selection_range)?;

    Ok(CodeLocation {
        path: file_uri_to_workspace_path(canonical_workspace_root, &link.target_uri).await?,
        range: target_selection_range,
    })
}

pub(crate) async fn map_definition_response(
    canonical_workspace_root: &Path,
    response: Option<GotoDefinitionResponse>,
) -> Result<Vec<CodeLocation>, MappingError> {
    match response {
        None => Ok(Vec::new()),
        Some(GotoDefinitionResponse::Scalar(location)) => Ok(vec![
            map_location(canonical_workspace_root, location).await?,
        ]),
        Some(GotoDefinitionResponse::Array(locations)) => {
            let mut mapped = Vec::with_capacity(locations.len());
            for location in locations {
                mapped.push(map_location(canonical_workspace_root, location).await?);
            }
            Ok(mapped)
        }
        Some(GotoDefinitionResponse::Link(links)) => {
            let mut mapped = Vec::with_capacity(links.len());
            for link in links {
                mapped.push(map_location_link(canonical_workspace_root, link).await?);
            }
            Ok(mapped)
        }
    }
}

pub(crate) async fn map_diagnostics(
    canonical_workspace_root: &Path,
    uri: &Uri,
    diagnostics: Vec<Diagnostic>,
) -> Result<Vec<CodeDiagnostic>, MappingError> {
    let path = file_uri_to_workspace_path(canonical_workspace_root, uri).await?;
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            Ok(CodeDiagnostic {
                location: CodeLocation {
                    path: path.clone(),
                    range: map_range(diagnostic.range)?,
                },
                severity: diagnostic.severity.and_then(map_diagnostic_severity),
                code: diagnostic.code.map(|code| match code {
                    NumberOrString::Number(number) => number.to_string(),
                    NumberOrString::String(text) => text,
                }),
                source: diagnostic.source,
                message: diagnostic.message,
            })
        })
        .collect()
}

pub(crate) fn map_symbol_kind(kind: SymbolKind) -> CodeSymbolKind {
    match kind {
        SymbolKind::FILE => CodeSymbolKind::File,
        SymbolKind::MODULE => CodeSymbolKind::Module,
        SymbolKind::NAMESPACE => CodeSymbolKind::Namespace,
        SymbolKind::PACKAGE => CodeSymbolKind::Package,
        SymbolKind::CLASS => CodeSymbolKind::Class,
        SymbolKind::METHOD => CodeSymbolKind::Method,
        SymbolKind::PROPERTY => CodeSymbolKind::Property,
        SymbolKind::FIELD => CodeSymbolKind::Field,
        SymbolKind::CONSTRUCTOR => CodeSymbolKind::Constructor,
        SymbolKind::ENUM => CodeSymbolKind::Enum,
        SymbolKind::INTERFACE => CodeSymbolKind::Interface,
        SymbolKind::FUNCTION => CodeSymbolKind::Function,
        SymbolKind::VARIABLE => CodeSymbolKind::Variable,
        SymbolKind::CONSTANT => CodeSymbolKind::Constant,
        SymbolKind::STRING => CodeSymbolKind::String,
        SymbolKind::NUMBER => CodeSymbolKind::Number,
        SymbolKind::BOOLEAN => CodeSymbolKind::Boolean,
        SymbolKind::ARRAY => CodeSymbolKind::Array,
        SymbolKind::OBJECT => CodeSymbolKind::Object,
        SymbolKind::KEY => CodeSymbolKind::Key,
        SymbolKind::NULL => CodeSymbolKind::Null,
        SymbolKind::ENUM_MEMBER => CodeSymbolKind::EnumMember,
        SymbolKind::STRUCT => CodeSymbolKind::Struct,
        SymbolKind::EVENT => CodeSymbolKind::Event,
        SymbolKind::OPERATOR => CodeSymbolKind::Operator,
        SymbolKind::TYPE_PARAMETER => CodeSymbolKind::TypeParameter,
        _ => CodeSymbolKind::Unknown,
    }
}

pub(crate) fn map_diagnostic_severity(
    severity: DiagnosticSeverity,
) -> Option<CodeDiagnosticSeverity> {
    match severity {
        DiagnosticSeverity::ERROR => Some(CodeDiagnosticSeverity::Error),
        DiagnosticSeverity::WARNING => Some(CodeDiagnosticSeverity::Warning),
        DiagnosticSeverity::INFORMATION => Some(CodeDiagnosticSeverity::Information),
        DiagnosticSeverity::HINT => Some(CodeDiagnosticSeverity::Hint),
        _ => None,
    }
}

/// Convert a local file URI into a canonical, workspace-relative path.
///
/// Both the workspace root and the URI target are expected to exist. The root
/// must already be canonical; the target is canonicalized before containment
/// is checked so a symlink cannot escape the workspace. A missing target is an
/// error rather than an invitation to fall back to lexical normalization.
pub(crate) async fn file_uri_to_workspace_path(
    canonical_workspace_root: &Path,
    uri: &Uri,
) -> Result<WorkspacePath, MappingError> {
    if !canonical_workspace_root.is_absolute() {
        return Err(MappingError::InvalidWorkspaceRoot {
            root: canonical_workspace_root.to_path_buf(),
        });
    }

    let uri_text = uri.as_str();
    let parsed = Url::parse(uri_text).map_err(|_| MappingError::InvalidFileUri {
        uri: uri_text.to_owned(),
    })?;
    if parsed.scheme() != "file" {
        return Err(MappingError::UnsupportedUriScheme {
            scheme: parsed.scheme().to_owned(),
        });
    }
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || !has_valid_percent_encoding(parsed.path())
    {
        return Err(MappingError::InvalidFileUri {
            uri: uri_text.to_owned(),
        });
    }

    let decoded_path = parsed
        .to_file_path()
        .map_err(|()| MappingError::InvalidFileUri {
            uri: uri_text.to_owned(),
        })?;
    let canonical_path = tokio::fs::canonicalize(&decoded_path)
        .await
        .map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                MappingError::PathDoesNotExist {
                    path: decoded_path.clone(),
                }
            } else {
                MappingError::PathResolution {
                    path: decoded_path.clone(),
                    source,
                }
            }
        })?;
    let relative = canonical_path
        .strip_prefix(canonical_workspace_root)
        .map_err(|_| MappingError::OutsideWorkspace {
            path: canonical_path.clone(),
        })?;
    let relative = relative
        .to_str()
        .ok_or_else(|| MappingError::NonUtf8WorkspacePath {
            path: relative.to_path_buf(),
        })?;

    Ok(WorkspacePath::from_normalized(relative))
}

fn ensure_range_contains(enclosing: CodeRange, selection: CodeRange) -> Result<(), MappingError> {
    if selection.start < enclosing.start || selection.end > enclosing.end {
        return Err(MappingError::SelectionOutsideRange {
            selection,
            enclosing,
        });
    }
    Ok(())
}

fn has_valid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }

        if index + 2 >= bytes.len()
            || !bytes[index + 1].is_ascii_hexdigit()
            || !bytes[index + 2].is_ascii_hexdigit()
        {
            return false;
        }
        index += 3;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        DocumentSymbol as LspDocumentSymbol, Position, Range,
        SymbolInformation as LspSymbolInformation, WorkspaceLocation,
    };
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn canonical_root(directory: &TempDir) -> PathBuf {
        fs::canonicalize(directory.path()).unwrap()
    }

    fn create_file(directory: &TempDir, relative: &str) -> PathBuf {
        let path = directory.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, b"saved content").unwrap();
        path
    }

    fn file_uri(path: &Path) -> Uri {
        Url::from_file_path(path).unwrap().as_str().parse().unwrap()
    }

    fn protocol_range(start: (u32, u32), end: (u32, u32)) -> Range {
        Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))
    }

    #[allow(deprecated)]
    fn document_symbol(
        name: &str,
        kind: SymbolKind,
        range: Range,
        selection_range: Range,
        children: Vec<LspDocumentSymbol>,
    ) -> LspDocumentSymbol {
        LspDocumentSymbol {
            name: name.to_owned(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range,
            selection_range,
            children: Some(children),
        }
    }

    #[allow(deprecated)]
    fn symbol_information(
        name: &str,
        kind: SymbolKind,
        location: Location,
    ) -> LspSymbolInformation {
        LspSymbolInformation {
            name: name.to_owned(),
            kind,
            tags: None,
            deprecated: None,
            location,
            container_name: Some("container".to_owned()),
        }
    }

    #[test]
    fn position_preserves_utf16_code_unit_offset() {
        let prefix = "a\u{1f980}\u{4e2d}";
        let utf16_offset = prefix.encode_utf16().count() as u32;
        assert_eq!(utf16_offset, 4);

        assert_eq!(
            map_position(Position::new(7, utf16_offset)),
            CodePosition::new(7, 4)
        );
    }

    #[tokio::test]
    async fn document_symbols_keep_nested_structure() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let child = document_symbol(
            "method",
            SymbolKind::METHOD,
            protocol_range((2, 2), (4, 3)),
            protocol_range((2, 5), (2, 11)),
            Vec::new(),
        );
        let parent = document_symbol(
            "Type",
            SymbolKind::CLASS,
            protocol_range((1, 0), (5, 1)),
            protocol_range((1, 6), (1, 10)),
            vec![child],
        );

        let mapped =
            map_document_symbol_response(&root, DocumentSymbolResponse::Nested(vec![parent]))
                .await
                .unwrap()
                .remove(0);
        assert_eq!(mapped.kind, CodeSymbolKind::Class);
        assert_eq!(mapped.children.len(), 1);
        assert_eq!(mapped.children[0].name, "method");
        assert_eq!(mapped.children[0].kind, CodeSymbolKind::Method);
        assert!(mapped.children[0].children.is_empty());
    }

    #[tokio::test]
    async fn definition_response_maps_location_link_array_and_null_shapes() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let target = create_file(&workspace, "src/target.rs");
        let uri = file_uri(&target);

        let scalar = map_definition_response(
            &root,
            Some(GotoDefinitionResponse::Scalar(Location::new(
                uri.clone(),
                protocol_range((1, 2), (1, 8)),
            ))),
        )
        .await
        .unwrap();
        assert_eq!(scalar.len(), 1);
        assert_eq!(scalar[0].path.as_str(), "src/target.rs");

        let array = map_definition_response(
            &root,
            Some(GotoDefinitionResponse::Array(vec![Location::new(
                uri.clone(),
                protocol_range((2, 0), (2, 4)),
            )])),
        )
        .await
        .unwrap();
        assert_eq!(array[0].range.start, CodePosition::new(2, 0));

        let links = map_definition_response(
            &root,
            Some(GotoDefinitionResponse::Link(vec![LocationLink {
                origin_selection_range: None,
                target_uri: uri,
                target_range: protocol_range((3, 0), (6, 1)),
                target_selection_range: protocol_range((3, 4), (3, 10)),
            }])),
        )
        .await
        .unwrap();
        assert_eq!(
            links[0].range,
            CodeRange::new(CodePosition::new(3, 4), CodePosition::new(3, 10))
        );

        assert!(map_definition_response(&root, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn location_link_rejects_selection_outside_target_range() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let target = create_file(&workspace, "target.ts");
        let error = map_location_link(
            &root,
            LocationLink {
                origin_selection_range: None,
                target_uri: file_uri(&target),
                target_range: protocol_range((5, 0), (7, 0)),
                target_selection_range: protocol_range((4, 0), (4, 3)),
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, MappingError::SelectionOutsideRange { .. }));
    }

    #[test]
    fn rejects_reversed_protocol_range() {
        let error = map_range(protocol_range((4, 1), (3, 9))).unwrap_err();
        assert!(matches!(
            error,
            MappingError::InvalidRange {
                start: CodePosition {
                    line: 4,
                    character: 1
                },
                end: CodePosition {
                    line: 3,
                    character: 9
                }
            }
        ));
    }

    #[tokio::test]
    async fn diagnostics_normalize_numeric_and_string_codes() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let source = create_file(&workspace, "src/lib.rs");
        let diagnostics = vec![
            Diagnostic::new(
                protocol_range((0, 0), (0, 1)),
                Some(DiagnosticSeverity::ERROR),
                Some(NumberOrString::Number(42)),
                Some("compiler".to_owned()),
                "numeric".to_owned(),
                None,
                None,
            ),
            Diagnostic::new(
                protocol_range((1, 0), (1, 1)),
                Some(DiagnosticSeverity::WARNING),
                Some(NumberOrString::String("E_NAME".to_owned())),
                None,
                "string".to_owned(),
                None,
                None,
            ),
        ];

        let mapped = map_diagnostics(&root, &file_uri(&source), diagnostics)
            .await
            .unwrap();
        assert_eq!(mapped[0].code.as_deref(), Some("42"));
        assert_eq!(mapped[0].severity, Some(CodeDiagnosticSeverity::Error));
        assert_eq!(mapped[1].code.as_deref(), Some("E_NAME"));
        assert_eq!(mapped[1].severity, Some(CodeDiagnosticSeverity::Warning));
    }

    #[test]
    fn symbol_and_severity_mappings_handle_unknown_values() {
        let unknown_kind: SymbolKind = serde_json::from_value(json!(99)).unwrap();
        let unknown_severity: DiagnosticSeverity = serde_json::from_value(json!(99)).unwrap();

        assert_eq!(map_symbol_kind(SymbolKind::STRUCT), CodeSymbolKind::Struct);
        assert_eq!(map_symbol_kind(unknown_kind), CodeSymbolKind::Unknown);
        assert_eq!(
            map_diagnostic_severity(DiagnosticSeverity::HINT),
            Some(CodeDiagnosticSeverity::Hint)
        );
        assert_eq!(map_diagnostic_severity(unknown_severity), None);
    }

    #[tokio::test]
    async fn maps_classic_and_range_less_workspace_symbols() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let source = create_file(&workspace, "src/main.ts");
        let uri = file_uri(&source);

        let classic = map_workspace_symbol_response(
            &root,
            WorkspaceSymbolResponse::Flat(vec![symbol_information(
                "run",
                SymbolKind::FUNCTION,
                Location::new(uri.clone(), protocol_range((1, 1), (1, 4))),
            )]),
        )
        .await
        .unwrap()
        .remove(0);
        assert_eq!(classic.location.path.as_str(), "src/main.ts");
        assert_eq!(classic.kind, CodeSymbolKind::Function);

        let modern = map_workspace_symbol_response(
            &root,
            WorkspaceSymbolResponse::Nested(vec![
                WorkspaceSymbol {
                    name: "Runner".to_owned(),
                    kind: SymbolKind::CLASS,
                    tags: None,
                    container_name: None,
                    location: OneOf::Left(Location::new(
                        uri.clone(),
                        protocol_range((3, 1), (3, 7)),
                    )),
                    data: None,
                },
                WorkspaceSymbol {
                    name: "State".to_owned(),
                    kind: SymbolKind::INTERFACE,
                    tags: None,
                    container_name: Some("model".to_owned()),
                    location: OneOf::Right(WorkspaceLocation { uri }),
                    data: None,
                },
            ]),
        )
        .await
        .unwrap();
        assert_eq!(modern[0].location.range.start, CodePosition::new(3, 1));
        assert_eq!(modern[1].location.path.as_str(), "src/main.ts");
        assert_eq!(modern[1].location.range, CodeRange::default());
    }

    #[tokio::test]
    async fn percent_decodes_file_name() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let source = create_file(&workspace, "src/with space.rs");
        let uri = file_uri(&source);
        assert!(uri.as_str().contains("with%20space.rs"));

        let mapped = file_uri_to_workspace_path(&root, &uri).await.unwrap();
        assert_eq!(mapped.as_str(), "src/with space.rs");
    }

    #[tokio::test]
    async fn rejects_paths_outside_workspace() {
        let workspace = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let source = create_file(&outside, "outside.rs");

        let error = file_uri_to_workspace_path(&root, &file_uri(&source))
            .await
            .unwrap_err();
        assert!(matches!(error, MappingError::OutsideWorkspace { .. }));
    }

    #[tokio::test]
    async fn rejects_non_file_uri() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let uri: Uri = "https://example.test/source.rs".parse().unwrap();

        let error = file_uri_to_workspace_path(&root, &uri).await.unwrap_err();
        assert!(matches!(
            error,
            MappingError::UnsupportedUriScheme { ref scheme } if scheme == "https"
        ));
    }

    #[tokio::test]
    async fn missing_paths_do_not_use_lexical_fallback() {
        let workspace = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let missing = workspace.path().join("missing.rs");

        let error = file_uri_to_workspace_path(&root, &file_uri(&missing))
            .await
            .unwrap_err();
        assert!(matches!(error, MappingError::PathDoesNotExist { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let workspace = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let root = canonical_root(&workspace);
        let outside_source = create_file(&outside, "secret.rs");
        let linked_source = workspace.path().join("linked.rs");
        symlink(outside_source, &linked_source).unwrap();

        let error = file_uri_to_workspace_path(&root, &file_uri(&linked_source))
            .await
            .unwrap_err();
        assert!(matches!(error, MappingError::OutsideWorkspace { .. }));
    }
}
