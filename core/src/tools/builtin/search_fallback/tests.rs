use super::super::search::SearchTool;
use super::{find_fuzzy, fuzzy_scan, is_literal_query};
use crate::tools::{Tool, ToolContext};
use std::fs;

#[test]
fn a_regex_query_is_not_literal() {
    assert!(is_literal_query("schema"));
    assert!(!is_literal_query("a.*b"));
    assert!(!is_literal_query("file.rs"));
}

#[test]
fn a_short_query_rejects_a_transposition() {
    assert!(find_fuzzy("ab", "ab", false).is_some());
    assert!(find_fuzzy("ab", "ba", false).is_none());
}

#[test]
fn a_transposition_of_a_longer_name_stays_above_the_floor() {
    let hit = find_fuzzy("shcema", "fn schema()", false).expect("schema");
    assert!(hit.score >= 48, "{}", hit.score);
}

#[test]
fn a_sparse_window_under_half_the_contiguous_score_is_discarded() {
    let line = format!("ab{}cd", "x".repeat(12));
    assert!(find_fuzzy("abcdef", &line, false).is_none());
    assert!(find_fuzzy("abcdef", "abcdef", false).is_some());
}

#[tokio::test]
async fn an_exact_hit_does_not_retry() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), "schema stays\n").unwrap();
    let output = SearchTool::new(true)
        .execute(
            &serde_json::json!({"mode": "grep", "query": "schema"}),
            &ToolContext::new(root.path().to_path_buf()),
        )
        .await
        .unwrap();
    assert!(output.success, "{}", output.content);
    assert!(
        output.content.contains("schema stays"),
        "{}",
        output.content
    );
    let metadata = output.metadata.unwrap();
    assert!(metadata.get("match").is_none(), "{metadata}");
}

#[tokio::test]
async fn a_lowercase_query_retries_without_case() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), "Schema stays\n").unwrap();
    let output = SearchTool::new(true)
        .execute(
            &serde_json::json!({"mode": "grep", "query": "schema"}),
            &ToolContext::new(root.path().to_path_buf()),
        )
        .await
        .unwrap();
    assert!(
        output.content.contains("Schema stays"),
        "{}",
        output.content
    );
    assert_eq!(output.metadata.unwrap()["match"], "case_insensitive");
}

#[tokio::test]
async fn an_explicit_case_sensitive_query_does_not_widen_case() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), "Schema stays\n").unwrap();
    let output = SearchTool::new(true)
        .execute(
            &serde_json::json!({
                "mode": "grep",
                "query": "schema",
                "case_sensitive": true
            }),
            &ToolContext::new(root.path().to_path_buf()),
        )
        .await
        .unwrap();
    assert!(
        output.content.contains("No matches found"),
        "{}",
        output.content
    );
    let metadata = output.metadata.unwrap();
    assert!(metadata.get("match").is_none(), "{metadata}");
}

#[test]
fn fuzzy_scan_returns_the_transposed_line() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "fn schema() {}\n").unwrap();
    let text = fuzzy_scan(
        root.path(),
        root.path(),
        Some("*.rs"),
        "shcema",
        false,
        super::FUZZY_TIME_BUDGET,
    )
    .expect("hit");
    assert!(text.contains("src/lib.rs"), "{text}");
    assert!(text.contains("fn schema()"), "{text}");
    assert!(!text.contains(super::FUZZY_SCAN_STOPPED), "{text}");
}

#[test]
fn fuzzy_scan_clips_a_long_line() {
    let root = tempfile::tempdir().unwrap();
    let prefix = "fn schema() {} ";
    let line = format!("{prefix}{}", "x".repeat(super::MAX_LINE_CHARS));
    fs::write(root.path().join("long.rs"), &line).unwrap();
    let text = fuzzy_scan(
        root.path(),
        root.path(),
        Some("*.rs"),
        "shcema",
        false,
        super::FUZZY_TIME_BUDGET,
    )
    .expect("hit");
    let kept = super::MAX_LINE_CHARS - prefix.chars().count();
    assert!(
        text.contains(&format!("{prefix}{}", "x".repeat(kept))),
        "{text}"
    );
    assert!(!text.contains(&"x".repeat(kept + 1)), "{text}");
}

#[test]
fn find_fuzzy_matches_a_typo_late_in_the_line() {
    let line = format!("{} fn schema()", "x".repeat(80));
    assert!(find_fuzzy("shcema", &line, false).is_some());
}

#[test]
fn fuzzy_scan_stops_when_the_budget_is_already_spent() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..20 {
        fs::write(root.path().join(format!("f{index}.rs")), "fn schema() {}\n").unwrap();
    }
    let text = super::fuzzy_scan_within(
        root.path(),
        root.path(),
        Some("*.rs"),
        "shcema",
        false,
        std::time::Duration::ZERO,
    )
    .expect("hit");
    assert_eq!(text.matches("fn schema()").count(), 1, "{text}");
    assert!(text.contains(super::FUZZY_SCAN_STOPPED), "{text}");
}

#[test]
fn fuzzy_scan_says_when_a_stopped_scan_found_nothing() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..20 {
        fs::write(root.path().join(format!("f{index}.rs")), "fn other() {}\n").unwrap();
    }
    let text = super::fuzzy_scan_within(
        root.path(),
        root.path(),
        Some("*.rs"),
        "shcema",
        false,
        std::time::Duration::ZERO,
    )
    .expect("stopped scan");
    assert!(text.contains(super::FUZZY_SCAN_STOPPED), "{text}");
    assert!(!text.contains("fn other()"), "{text}");
}

async fn execute_grep(
    root: &std::path::Path,
    query: &str,
    budget: Option<std::time::Duration>,
) -> crate::tools::types::ToolOutput {
    let mut tool = SearchTool::new(true);
    if let Some(budget) = budget {
        tool = tool.with_fuzzy_budget(budget);
    }
    tool.execute(
        &serde_json::json!({"mode": "grep", "query": query}),
        &ToolContext::new(root.to_path_buf()),
    )
    .await
    .expect("grep")
}

#[tokio::test]
async fn a_finished_grep_widen_returns_the_fuzzy_line() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "fn schema() {}\n").unwrap();
    let output = execute_grep(root.path(), "shcema", None).await;
    assert!(output.success, "{}", output.content);
    assert!(output.content.contains("fn schema()"), "{}", output.content);
    assert!(
        !output.content.contains(super::FUZZY_SCAN_STOPPED),
        "{}",
        output.content
    );
    assert_eq!(output.metadata.unwrap()["match"], "fuzzy");
}

#[tokio::test]
async fn an_already_spent_grep_widen_reports_the_hit_and_the_stop() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..20 {
        fs::write(root.path().join(format!("f{index}.rs")), "fn schema() {}\n").unwrap();
    }
    let output = execute_grep(root.path(), "shcema", Some(std::time::Duration::ZERO)).await;
    assert!(output.success, "{}", output.content);
    assert_eq!(
        output.content.matches("fn schema()").count(),
        1,
        "{}",
        output.content
    );
    assert!(
        output.content.contains(super::FUZZY_SCAN_STOPPED),
        "{}",
        output.content
    );
    assert!(
        !output.content.contains("No matches found"),
        "{}",
        output.content
    );
    assert_eq!(output.metadata.unwrap()["match"], "fuzzy");
}

#[tokio::test]
async fn an_already_spent_grep_widen_with_no_hit_is_not_no_matches() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..20 {
        fs::write(root.path().join(format!("f{index}.rs")), "fn other() {}\n").unwrap();
    }
    let output = execute_grep(root.path(), "shcema", Some(std::time::Duration::ZERO)).await;
    assert!(output.success, "{}", output.content);
    assert!(
        output.content.contains(super::FUZZY_SCAN_STOPPED),
        "{}",
        output.content
    );
    assert!(
        !output.content.contains("No matches found"),
        "{}",
        output.content
    );
    assert!(
        output
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("match"))
            .is_none(),
        "{output:?}"
    );
}
