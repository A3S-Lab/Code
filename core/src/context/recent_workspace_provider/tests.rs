#![cfg(test)]

use super::*;

#[tokio::test]
async fn returns_recent_files_as_context() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("src.rs"), "fn main() {}\n").unwrap();
    let manifest = LocalWorkspaceManifest::start(temp.path());
    let mut rx = manifest.subscribe();
    tokio::time::timeout(
        crate::test_support::external_resource_start_timeout(std::time::Duration::from_secs(5)),
        rx.recv(),
    )
    .await
    .unwrap()
    .unwrap();

    manifest.touch_file("src.rs");
    let provider = RecentWorkspaceFilesContextProvider::new(manifest);
    let result = provider
        .query(&ContextQuery::new("work on this"))
        .await
        .unwrap();

    assert_eq!(result.provider, "recent_workspace_files");
    assert_eq!(result.items.len(), 1);
    assert!(result.items[0].content.contains("src.rs"));
    assert_eq!(
        result.items[0].source.as_deref(),
        Some("a3s://workspace/recent-files")
    );
}

fn snapshot(paths: impl IntoIterator<Item = String>) -> LocalWorkspaceManifestSnapshot {
    LocalWorkspaceManifestSnapshot {
        version: 7,
        root: "/authorized-workspace".into(),
        scanned_at_ms: 123,
        files: paths
            .into_iter()
            .map(|path| crate::workspace::LocalWorkspaceFile {
                path,
                size: 0,
                modified_ms: None,
                language: None,
                status: crate::workspace::LocalWorkspaceFileStatus::Unknown,
                binary: false,
                generated: false,
            })
            .collect(),
    }
}

#[test]
fn navigation_caps_rendered_cjk_escaped_bytes_and_smaller_query_budget() {
    let path = format!("资料/{}/文件<&>.rs", "变量<&>".repeat(140));
    let snapshot = snapshot([path.clone(), "src/main.rs".into()]);
    let entries: Vec<_> = (0..33)
        .map(|i| RecentWorkspaceFile {
            path: format!("{path}-{i}"),
            score: 0.8,
            touched_at_ms: 321,
            touch_count: 3,
        })
        .collect();
    for budget in [0, 1, 40, 100, 250, 1000, 4000] {
        let result = project_navigation(&snapshot, &entries, 32, budget);
        assert!(result.truncated);
        assert!(result.total_tokens <= budget.min(MAX_NAVIGATION_TOKENS));
        if let Some(item) = result.items.first() {
            let rendered = item.to_xml();
            assert!(rendered.len() <= MAX_NAVIGATION_BYTES);
            assert_eq!(
                item.token_count,
                crate::llm::estimate_prompt_tokens(&[], Some(&rendered), &[])
            );
            assert!(item.token_count > item.content.split_whitespace().count());
            assert_eq!(item.metadata["manifest_version"], 7);
            assert_eq!(item.metadata["recent_touched_at_ms"], 321);
            assert_eq!(item.metadata["truncated"], true);
            assert!(item.content.ends_with("truncated: true\n"));
        }
    }
}

#[test]
fn navigation_large_manifest_is_a_bounded_directory_sample_without_recent_files() {
    let mut snapshot = snapshot((0..100_000).map(|n| format!("dir{n:06}/main.rs")));
    let first = project_navigation(&snapshot, &[], 12, 4000);
    assert_eq!(first.items.len(), 1);
    assert!(first.truncated);
    assert_eq!(first.items[0].content.matches("Directory:").count(), 12);
    // Entries outside the documented sample do not change projection work or bytes.
    for file in snapshot.files.iter_mut().skip(MAX_DIRECTORY_CANDIDATES) {
        file.path = "outside-sample/ignored.rs".into();
    }
    assert_eq!(
        first.content_digest(),
        project_navigation(&snapshot, &[], 12, 4000).content_digest()
    );
}

#[test]
fn navigation_uses_original_global_source_budgets_and_latest_source_dedupe() {
    use crate::context::{ContextAssembler, ContextBudget, ContextSourcePolicy};
    let first = project_navigation(&snapshot(["old/main.rs".into()]), &[], 12, 4000);
    let mut current = snapshot(["new/main.rs".into()]);
    current.version = 8;
    let next = project_navigation(&current, &[], 12, 4000);
    let tokens = next.total_tokens;
    let assembler = ContextAssembler::new(ContextBudget {
        max_items: 4,
        max_tokens: tokens,
    });
    let selected = assembler.assemble(&[first.clone(), next.clone()]);
    assert_eq!(selected.items.len(), 1);
    assert!(selected.items[0].content.contains("new/"));
    assert!(!selected.items[0].content.contains("old/"));
    let mut second_source = next.clone();
    second_source.provider = "other-host-view".into();
    assert_eq!(
        assembler
            .assemble(&[next.clone(), second_source])
            .items
            .len(),
        1
    );
    assert!(ContextAssembler::new(ContextBudget {
        max_items: 4,
        max_tokens: tokens - 1
    })
    .assemble(std::slice::from_ref(&next))
    .is_empty());
    assert!(assembler
        .clone()
        .with_source_policy(ContextSourcePolicy {
            max_items_per_source: None,
            max_tokens_per_source: Some(tokens - 1)
        })
        .assemble(std::slice::from_ref(&next))
        .is_empty());
    let deleted = project_navigation(&snapshot([]), &[], 12, 4000);
    assert!(assembler.assemble(&[first, next, deleted]).is_empty());
}

#[tokio::test]
async fn navigation_never_activates_deferred_discovery_and_honors_empty_queries() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("secret.txt"),
        "not yet authorized for discovery",
    )
    .unwrap();
    let manifest = LocalWorkspaceManifest::start_deferred(root.path());
    let provider = RecentWorkspaceFilesContextProvider::new(manifest.clone());
    for query in [
        ContextQuery::new("hint"),
        ContextQuery::new("hint").with_max_results(0),
        ContextQuery::new("hint").with_max_tokens(0),
        ContextQuery::new("hint").with_types([ContextType::Memory]),
    ] {
        assert!(provider.query(&query).await.unwrap().is_empty());
        assert!(!manifest.is_active());
        assert_eq!(manifest.snapshot().version, 0);
    }
}
