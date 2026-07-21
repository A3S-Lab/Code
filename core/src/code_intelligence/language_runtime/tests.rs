use std::{path::Path, str::FromStr};

use lsp_types::Uri;
use serde_json::json;

use super::super::{
    document_store::DocumentObservationKind, lsp::initialize::ServerTextSyncMode, CodePosition,
    CodeRange, CodeSymbolKind, DocumentSymbol,
};
#[cfg(unix)]
use super::paths::existing_workspace_file_uri;
use super::{
    bound_document_symbols,
    paths::{valid_utf16_position, validate_canonical_root, validate_workspace_path},
    protocol::{bound_items, did_save_params, document_sync_plan, DocumentSyncStep},
    MAX_DOCUMENT_SYMBOLS,
};
use crate::workspace::WorkspacePath;

fn document_symbol(name: impl Into<String>, children: Vec<DocumentSymbol>) -> DocumentSymbol {
    DocumentSymbol {
        name: name.into(),
        detail: None,
        kind: CodeSymbolKind::Function,
        range: CodeRange::new(CodePosition::new(0, 0), CodePosition::new(0, 1)),
        selection_range: CodeRange::new(CodePosition::new(0, 0), CodePosition::new(0, 1)),
        children,
    }
}

#[test]
fn sync_matrix_respects_full_incremental_disk_only_and_save() {
    assert_eq!(
        document_sync_plan(
            ServerTextSyncMode::Full,
            true,
            false,
            DocumentObservationKind::Opened,
            false,
        )
        .steps,
        [DocumentSyncStep::Open]
    );
    assert_eq!(
        document_sync_plan(
            ServerTextSyncMode::Full,
            true,
            false,
            DocumentObservationKind::Changed,
            true,
        )
        .steps,
        [DocumentSyncStep::Change]
    );
    assert_eq!(
        document_sync_plan(
            ServerTextSyncMode::Incremental,
            true,
            false,
            DocumentObservationKind::Changed,
            true,
        )
        .steps,
        [DocumentSyncStep::Close, DocumentSyncStep::Open]
    );
    assert_eq!(
        document_sync_plan(
            ServerTextSyncMode::Incremental,
            true,
            false,
            DocumentObservationKind::Reopened,
            true,
        )
        .steps,
        [DocumentSyncStep::Close, DocumentSyncStep::Open]
    );
    assert!(document_sync_plan(
        ServerTextSyncMode::None,
        true,
        false,
        DocumentObservationKind::Opened,
        false,
    )
    .steps
    .is_empty());
    let disk_only = document_sync_plan(
        ServerTextSyncMode::Full,
        false,
        true,
        DocumentObservationKind::Changed,
        true,
    );
    assert!(disk_only.steps.is_empty());
    assert!(disk_only.send_did_save);

    let uri = Uri::from_str("file:///workspace/src/lib.rs").unwrap();
    assert_eq!(
        serde_json::to_value(did_save_params(&uri)).unwrap(),
        json!({"textDocument": {"uri": "file:///workspace/src/lib.rs"}})
    );
}

#[test]
fn validates_utf16_boundaries_and_all_lsp_line_endings() {
    let text = "a🦀中\r\nsecond\rthird\nfourth";
    assert!(valid_utf16_position(text, CodePosition::new(0, 4)));
    assert!(!valid_utf16_position(text, CodePosition::new(0, 2)));
    assert!(valid_utf16_position(text, CodePosition::new(1, 6)));
    assert!(valid_utf16_position(text, CodePosition::new(2, 5)));
    assert!(valid_utf16_position(text, CodePosition::new(3, 6)));
    assert!(!valid_utf16_position(text, CodePosition::new(4, 0)));
    assert!(valid_utf16_position("trailing\n", CodePosition::new(1, 0)));
}

#[test]
fn rejects_non_normalized_paths_and_bounds_results() {
    for value in ["../outside.rs", "src/../lib.rs", "."] {
        assert!(validate_workspace_path(&WorkspacePath::from_normalized(value)).is_err());
    }
    // WorkspacePath intentionally treats a leading slash as workspace-relative.
    assert!(validate_workspace_path(&WorkspacePath::from_normalized("/src/lib.rs")).is_ok());
    assert!(validate_workspace_path(&WorkspacePath::from_normalized("src/lib.rs")).is_ok());
    assert_eq!(bound_items(vec![1, 2, 3], 2), (vec![1, 2], true));
    assert_eq!(bound_items(vec![1, 2], 2), (vec![1, 2], false));
    assert_eq!(bound_items(vec![1], 0), (Vec::<i32>::new(), true));
}

#[test]
fn document_symbol_bound_truncates_wide_trees_in_server_order() {
    let symbols = (0..=MAX_DOCUMENT_SYMBOLS)
        .map(|index| document_symbol(format!("symbol-{index}"), Vec::new()))
        .collect();

    let (bounded, truncated) = bound_document_symbols(symbols, MAX_DOCUMENT_SYMBOLS);

    assert!(truncated);
    assert_eq!(bounded.len(), MAX_DOCUMENT_SYMBOLS);
    assert_eq!(bounded[0].name, "symbol-0");
    assert_eq!(
        bounded[MAX_DOCUMENT_SYMBOLS - 1].name,
        format!("symbol-{}", MAX_DOCUMENT_SYMBOLS - 1)
    );
}

#[test]
fn document_symbol_bound_preserves_ancestors_and_child_nesting() {
    let symbols = vec![document_symbol(
        "root",
        vec![document_symbol(
            "child",
            vec![document_symbol(
                "grandchild",
                vec![document_symbol("discarded", Vec::new())],
            )],
        )],
    )];

    let (bounded, truncated) = bound_document_symbols(symbols, 3);

    assert!(truncated);
    assert_eq!(bounded.len(), 1);
    assert_eq!(bounded[0].name, "root");
    assert_eq!(bounded[0].children.len(), 1);
    assert_eq!(bounded[0].children[0].name, "child");
    assert_eq!(bounded[0].children[0].children.len(), 1);
    assert_eq!(bounded[0].children[0].children[0].name, "grandchild");
    assert!(bounded[0].children[0].children[0].children.is_empty());
}

#[test]
fn document_symbol_bound_is_not_truncated_at_the_exact_limit() {
    let symbols = (0..MAX_DOCUMENT_SYMBOLS)
        .map(|index| document_symbol(format!("symbol-{index}"), Vec::new()))
        .collect();

    let (bounded, truncated) = bound_document_symbols(symbols, MAX_DOCUMENT_SYMBOLS);

    assert!(!truncated);
    assert_eq!(bounded.len(), MAX_DOCUMENT_SYMBOLS);
}

#[tokio::test]
async fn canonical_root_must_be_resolved_and_a_directory() {
    let workspace = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    validate_canonical_root(&root).await.unwrap();
    assert!(validate_canonical_root(Path::new("relative"))
        .await
        .is_err());

    let file = root.join("file");
    std::fs::write(&file, "saved").unwrap();
    assert!(validate_canonical_root(&file).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn existing_source_uri_rejects_symlink_escape_and_missing_path() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let outside_file = outside.path().join("secret.rs");
    std::fs::write(&outside_file, "secret").unwrap();
    symlink(&outside_file, workspace.path().join("linked.rs")).unwrap();

    assert!(
        existing_workspace_file_uri(&root, &WorkspacePath::from_normalized("linked.rs"))
            .await
            .is_err()
    );
    assert!(
        existing_workspace_file_uri(&root, &WorkspacePath::from_normalized("missing.rs"))
            .await
            .is_err()
    );
}
