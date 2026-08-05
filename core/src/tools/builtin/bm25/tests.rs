use super::*;

fn context_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, ToolContext) {
    let temp = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = temp.path().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    let context = ToolContext::new(temp.path().to_path_buf());
    (temp, context)
}

#[test]
fn schema_exposes_bounded_plain_text_search() {
    let tool = Bm25Tool;
    let schema = tool.parameters();

    assert_eq!(tool.name(), "bm25");
    assert_eq!(schema["required"], serde_json::json!(["query"]));
    assert_eq!(schema["properties"]["limit"]["maximum"], MAX_LIMIT);
    assert_eq!(
        schema["properties"]["context"]["maximum"],
        MAX_CONTEXT_LINES
    );
    assert_eq!(tool.capabilities(&serde_json::json!({})).max_parallelism, 2);
}

#[tokio::test]
async fn ranks_multi_term_chunks_and_returns_source_metadata() {
    let (_temp, context) = context_with_files(&[
        (
            "src/session.rs",
            "pub fn invalidate_session_cache() {\n    // session cache invalidation policy\n    clear_session_cache();\n}\n",
        ),
        (
            "src/log.rs",
            "pub fn log_session() {\n    println!(\"session\");\n}\n",
        ),
    ]);

    let result = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "session cache invalidation",
                "path": "src",
                "glob": "*.rs",
                "limit": 5,
                "context": 1
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("src/session.rs:1-3"));
    assert!(result.content.contains("session cache invalidation policy"));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["algorithm"], "bm25");
    assert_eq!(metadata["results"][0]["path"], "src/session.rs");
    assert!(metadata["results"][0]["score"].as_f64().unwrap() > 0.0);
    assert_eq!(metadata["source_anchors"][0], "src/session.rs");
}

#[tokio::test]
async fn honors_glob_filter() {
    let (_temp, context) = context_with_files(&[
        ("src/auth.rs", "authentication policy token\n"),
        ("README.md", "authentication policy token token token\n"),
    ]);

    let result = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "authentication policy token",
                "glob": "*.rs"
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("src/auth.rs"));
    assert!(!result.content.contains("README.md"));
}

#[tokio::test]
async fn reports_no_matches_without_failing() {
    let (_temp, context) = context_with_files(&[("src/lib.rs", "pub fn existing() {}\n")]);

    let result = Bm25Tool
        .execute(&serde_json::json!({"query": "missing term"}), &context)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.content.contains("No BM25 matches found"));
}

#[tokio::test]
async fn rejects_empty_punctuation_and_escaping_queries() {
    let (_temp, context) = context_with_files(&[("src/lib.rs", "content\n")]);

    for args in [
        serde_json::json!({"query": ""}),
        serde_json::json!({"query": "::"}),
        serde_json::json!({"query": "content", "path": "../outside"}),
    ] {
        let result = Bm25Tool.execute(&args, &context).await.unwrap();
        assert!(!result.success, "args={args} output={}", result.content);
    }
}

#[tokio::test]
async fn validates_numeric_bounds_for_direct_calls() {
    let (_temp, context) = context_with_files(&[("src/lib.rs", "content\n")]);

    for args in [
        serde_json::json!({"query": "content", "limit": 0}),
        serde_json::json!({"query": "content", "limit": MAX_LIMIT + 1}),
        serde_json::json!({"query": "content", "context": MAX_CONTEXT_LINES + 1}),
    ] {
        let result = Bm25Tool.execute(&args, &context).await.unwrap();
        assert!(!result.success, "args={args} output={}", result.content);
    }
}
