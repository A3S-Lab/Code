use crate::config::{AgenticParseConfig, AgenticSearchConfig};
use crate::document_consume::{
    self, build_deep_search_response, build_fast_search_response, build_filename_search_response,
    build_search_document_substrate, build_search_no_results_output,
    build_search_tool_output as build_fast_search_tool_output, sample_deep_search_regions,
    DeepSearchDocumentRegion, DeepSearchSampledLine, DeepSearchSamplingDocument,
    SearchDocumentMatch, SearchDocumentMatchLine,
};
use crate::document_parse_engine::execute_parse_request;
use crate::document_search_engine::{execute_fast_search, execute_filename_search};
use crate::document_service_types::{
    resolve_parse_request, resolve_search_request, ParseExecutionStrategy, ResolvedParseRequest,
    SearchExecutionMode,
};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("MockLlmClient should not be used in this test")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("MockLlmClient should not be used in this test")
    }
}

#[test]
fn build_fast_search_tool_output_preserves_mode() {
    let output = build_fast_search_tool_output(crate::document_consume::SearchResponseInput {
        keywords: vec!["parser".to_string()],
        result_count: 1,
        report: "Found 1 file(s) matching \"parser\"".to_string(),
        results_metadata: vec![json!({"path": "docs/report.pdf"})],
    });
    assert!(output.success);
    assert_eq!(output.metadata.as_ref().unwrap()["mode"], json!("fast"));
}

#[test]
fn build_fast_search_response_renders_and_serializes_matches() {
    let output = build_fast_search_response(
        "parser",
        vec!["parser".to_string()],
        &[SearchDocumentMatch {
            path_display: "docs/report.pdf".to_string(),
            file_type: "docs".to_string(),
            relevance: 1.5,
            score: document_consume::SearchScoreMetadataInput {
                base_score: 1.0,
                path_signal: 0.2,
                idf_boost: 1.0,
                file_type_boost: 0.9,
                unique_keywords_matched: 1,
            },
            matches: vec![SearchDocumentMatchLine {
                line_number: 12,
                content: "The parser now emits structured search labels.".to_string(),
                locator: Some("page 2 | page 2: 1. Overview".to_string()),
                context_before: vec!["[section] page 2: 1. Overview".to_string()],
                context_after: vec![],
            }],
            document_metadata: Some(document_consume::SearchDocumentMetadata {
                title: Some("report.pdf".to_string()),
                stage: Some("normalized".to_string()),
                line_count: 3,
                chunk_count: 1,
                has_runtime_metadata: false,
            }),
            document_runtime: None,
        }],
    );
    assert!(output
        .content
        .contains("Found 1 file(s) matching \"parser\""));
    assert_eq!(
        output.metadata.as_ref().unwrap()["results"][0]["match_count"],
        json!(1)
    );
    assert_eq!(
        output.metadata.as_ref().unwrap()["results"][0]["document_metadata"]["stage"],
        json!("normalized")
    );
}

#[test]
fn build_parse_tool_output_preserves_strategy_metadata() {
    let output = crate::document_consume::build_parse_tool_output(
        &crate::document_consume::ParseResultInput {
            path_display: "report.pdf",
            strategy_label: "narrative",
            document_parser_config: None,
            document_runtime: None,
            document_quality: None,
            block_count: 1,
            non_empty_block_count: 1,
            line_count: 2,
            word_count: 4,
            document_language: None,
            document_keywords: &[],
            document_provenance: None,
            document_confidence: None,
            query_used: false,
            llm_input_truncated: false,
            llm_blocks_included: 0,
            llm_block_details: &[],
            chunk_highlights: &[],
            structured_payloads: &[],
            max_chars: 8000,
            structural_summary: "\n## Structural Summary\n\nOverview\n",
            llm_answer: None,
            tables: &[],
            pages: &[],
        },
    );
    assert!(output.success);
    assert_eq!(
        output.metadata.as_ref().unwrap()["strategy"],
        json!("narrative")
    );
}

#[test]
fn prepare_document_uses_plaintext_fallback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello service").unwrap();
    let parsed = document_consume::prepare_document_from_path(&path, None, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(parsed.to_text(), "hello service");
}

#[test]
fn load_extracted_document_uses_plaintext_fallback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello service").unwrap();
    let extracted = document_consume::load_extracted_document_from_path(&path, None, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(extracted.into_parsed_document().to_text(), "hello service");
}

#[test]
fn prepare_parse_document_builds_parse_views() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Title\n\nBody text").unwrap();
    let prepared = document_consume::prepare_parse_document_from_path(
        &path,
        None,
        None,
        "auto",
        8000,
        Some("title"),
        crate::document_service_types::document_block_kind_label,
    )
    .unwrap()
    .unwrap();
    assert_eq!(prepared.strategy_label, "narrative");
    assert!(prepared.structural_summary.contains("Structural Summary"));
    assert!(prepared.llm_rendered_content.contains("## Block 1:"));
}

#[test]
fn build_parse_tool_output_surfaces_structured_payloads() {
    let output = crate::document_consume::build_parse_tool_output(
        &crate::document_consume::ParseResultInput {
            path_display: "sheet.xlsx",
            strategy_label: "tabular",
            document_parser_config: None,
            document_runtime: None,
            document_quality: None,
            block_count: 1,
            non_empty_block_count: 1,
            line_count: 4,
            word_count: 6,
            document_language: Some("en"),
            document_keywords: &["revenue".to_string()],
            document_provenance: None,
            document_confidence: None,
            query_used: false,
            llm_input_truncated: false,
            llm_blocks_included: 0,
            llm_block_details: &[],
            chunk_highlights: &[],
            structured_payloads: &[json!({
                "index": 1,
                "kind": "table",
                "label": "sheet1",
                "payload_summary": "object keys=headers, rows",
                "payload_preview": "{\n  \"headers\": [\"name\", \"value\"]\n}",
                "structured_payload": {
                    "headers": ["name", "value"],
                    "rows": [["foo", "1"]]
                }
            })],
            max_chars: 8000,
            structural_summary: "\n## Structural Summary\n\nTable\n",
            llm_answer: None,
            tables: &[],
            pages: &[],
        },
    );

    assert!(output.success);
    assert!(output.content.contains("Structured Payloads"));
    assert_eq!(
        output.metadata.as_ref().unwrap()["structured_payloads"][0]["structured_payload"]
            ["headers"][0],
        json!("name")
    );
}

#[test]
fn build_search_document_substrate_preserves_runtime_metadata() {
    let doc = crate::document_parser::ParsedDocument {
        title: Some("scan.pdf".to_string()),
        blocks: vec![crate::document_parser::DocumentBlock::new(
            crate::document_parser::DocumentBlockKind::Metadata,
            Some("ocr"),
            "mode=ocr\nformat=pdf\nprovider=test-ocr\nmodel=unit-test-vlm\nprompt=set\nmax_images=2\ndpi=144",
        )],
        metadata: Some(crate::document_parser::DocumentMetadata {
            attributes: std::collections::BTreeMap::from([(
                "document.stage".to_string(),
                "normalized".to_string(),
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let substrate = build_search_document_substrate(std::path::Path::new("scan.pdf"), &doc, None);

    assert!(!substrate.search_lines.is_empty());
    assert!(substrate.document_runtime.is_some());
    assert_eq!(substrate.metadata.title.as_deref(), Some("scan.pdf"));
    assert_eq!(substrate.metadata.stage.as_deref(), Some("normalized"));
    assert!(substrate.metadata.has_runtime_metadata);
    assert!(substrate.metadata.line_count >= 1);
    assert_eq!(substrate.metadata.chunk_count, 1);
}

#[test]
fn prepare_search_document_uses_plaintext_fallback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello search").unwrap();

    let substrate = document_consume::prepare_search_document_substrate(&path, None, None, None)
        .unwrap()
        .unwrap();

    assert!(!substrate.search_lines.is_empty());
    assert_eq!(substrate.metadata.title, None);
    assert_eq!(substrate.metadata.stage, None);
    assert!(!substrate.metadata.has_runtime_metadata);
}

#[test]
fn build_deep_search_response_renders_evidence_and_metadata() {
    let output = build_deep_search_response(
        "token",
        vec!["token".to_string()],
        4,
        &[DeepSearchDocumentRegion {
            path_display: "src/auth.rs".to_string(),
            file_type: "code".to_string(),
            evidence_score: 1.234,
            score: document_consume::SearchScoreMetadataInput {
                base_score: 1.0,
                path_signal: 0.3,
                idf_boost: 1.1,
                file_type_boost: 1.2,
                unique_keywords_matched: 2,
            },
            matches: vec![],
            sampled_lines: vec![DeepSearchSampledLine {
                line_number: 10,
                content: "verify_token(token)".to_string(),
                locator: Some("page 1".to_string()),
                distance: 0,
                weight: 1.0,
            }],
            document_metadata: document_consume::SearchDocumentMetadata {
                title: Some("auth.rs".to_string()),
                stage: Some("prepared".to_string()),
                line_count: 8,
                chunk_count: 2,
                has_runtime_metadata: false,
            },
            document_runtime: None,
        }],
    );
    assert!(output.content.contains("evidence: 1.234"));
    assert_eq!(
        output.metadata.as_ref().unwrap()["initial_pool_size"],
        json!(4)
    );
    assert_eq!(
        output.metadata.as_ref().unwrap()["results"][0]["document_metadata"]["title"],
        json!("auth.rs")
    );
}

#[test]
fn build_fast_search_response_preserves_locator_text() {
    let output = build_fast_search_response(
        "parser labels",
        vec!["parser".to_string()],
        &[SearchDocumentMatch {
            path_display: "docs/report.pdf".to_string(),
            file_type: "docs".to_string(),
            relevance: 1.5,
            score: document_consume::SearchScoreMetadataInput {
                base_score: 1.0,
                path_signal: 0.2,
                idf_boost: 1.0,
                file_type_boost: 0.9,
                unique_keywords_matched: 1,
            },
            matches: vec![SearchDocumentMatchLine {
                line_number: 12,
                content: "The parser now emits structured search labels.".to_string(),
                locator: Some("page 2 | page 2: 1. Overview".to_string()),
                context_before: vec!["[section] page 2: 1. Overview".to_string()],
                context_after: vec!["Additional supporting text.".to_string()],
            }],
            document_metadata: Some(document_consume::SearchDocumentMetadata {
                title: Some("report.pdf".to_string()),
                stage: Some("normalized".to_string()),
                line_count: 4,
                chunk_count: 1,
                has_runtime_metadata: false,
            }),
            document_runtime: None,
        }],
    );
    assert!(output.content.contains(
        "▶ L12 [page 2 | page 2: 1. Overview]: The parser now emits structured search labels."
    ));
    assert_eq!(
        output.metadata.as_ref().unwrap()["results"][0]["matches"][0]["locator"],
        json!("page 2 | page 2: 1. Overview")
    );
}

#[test]
fn build_search_no_results_output_is_stable() {
    let output = build_search_no_results_output("missing symbol");
    assert!(output.success);
    assert!(output
        .content
        .contains("No results found for: missing symbol"));
}

#[test]
fn build_filename_search_response_preserves_mode_metadata() {
    let output = build_filename_search_response(&[
        std::path::PathBuf::from("src/auth.rs"),
        std::path::PathBuf::from("src/token.rs"),
    ]);
    assert!(output.success);
    assert!(output.content.contains("src/auth.rs"));
    assert_eq!(
        output.metadata.as_ref().unwrap()["mode"],
        json!("filename_only")
    );
    assert_eq!(output.metadata.as_ref().unwrap()["result_count"], json!(2));
}

#[test]
fn sample_deep_search_regions_builds_evidence_ranked_regions() {
    let regions = sample_deep_search_regions(
        &[DeepSearchSamplingDocument {
            path_display: "src/auth.rs".to_string(),
            file_type: "code".to_string(),
            relevance: 2.0,
            score: document_consume::SearchScoreMetadataInput {
                base_score: 1.0,
                path_signal: 0.2,
                idf_boost: 1.0,
                file_type_boost: 1.2,
                unique_keywords_matched: 1,
            },
            matches: vec![SearchDocumentMatchLine {
                line_number: 3,
                content: "verify_token(token)".to_string(),
                locator: Some("page 1".to_string()),
                context_before: vec![],
                context_after: vec![],
            }],
            search_lines: vec![
                "# auth.rs".to_string(),
                "fn authenticate() {".to_string(),
                "verify_token(token)".to_string(),
                "}".to_string(),
            ],
            search_line_locators: vec![None, None, Some("page 1".to_string()), None],
            document_metadata: document_consume::SearchDocumentMetadata {
                title: Some("auth.rs".to_string()),
                stage: Some("prepared".to_string()),
                line_count: 4,
                chunk_count: 1,
                has_runtime_metadata: false,
            },
            document_runtime: None,
        }],
        2,
    );
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].path_display, "src/auth.rs");
    assert!(!regions[0].sampled_lines.is_empty());
    assert!(regions[0].evidence_score > 0.0);
    assert_eq!(
        regions[0].document_metadata.stage.as_deref(),
        Some("prepared")
    );
}

#[test]
fn extract_search_keywords_removes_stop_words_and_splits_identifiers() {
    let kws = crate::document_consume::extract_search_keywords(
        "how does AuthTokenManager verify_user-token work",
    );
    assert!(!kws.iter().any(|k| k == "how"));
    assert!(kws.iter().any(|k| k == "auth"));
    assert!(kws.iter().any(|k| k == "token"));
    assert!(kws.iter().any(|k| k == "manager"));
    assert!(kws.len() <= 8);
}

#[test]
fn search_path_signal_score_rewards_filename_hits() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path();
    let patterns = vec![
        regex::Regex::new("(?i)auth").unwrap(),
        regex::Regex::new("(?i)rs").unwrap(),
    ];
    let auth_score = crate::document_service_types::search_path_signal_score(
        workspace,
        &workspace.join("auth.rs"),
        &patterns,
    );
    let readme_score = crate::document_service_types::search_path_signal_score(
        workspace,
        &workspace.join("README.md"),
        &patterns,
    );
    assert!(auth_score > readme_score);
    assert!(auth_score > 0.0);
}

#[test]
fn resolve_search_request_applies_defaults_and_normalization() {
    let request = resolve_search_request(
        &json!({"mode": "weird", "max_results": 0, "context_lines": 999, "include": " *.rs "}),
        Some(&AgenticSearchConfig {
            enabled: true,
            default_mode: "deep".to_string(),
            max_results: 7,
            context_lines: 4,
        }),
    );
    assert_eq!(request.mode, SearchExecutionMode::Fast);
    assert_eq!(request.max_results, 1);
    assert_eq!(request.context_lines, 20);
    assert_eq!(request.include_glob.as_deref(), Some("*.rs"));
}

#[test]
fn resolve_parse_request_applies_defaults_and_normalization() {
    let request = resolve_parse_request(
        &json!({"strategy": "unknown", "max_chars": 1}),
        Some(&AgenticParseConfig {
            enabled: true,
            default_strategy: "structured".to_string(),
            max_chars: 12_000,
        }),
    );
    assert_eq!(request.strategy, ParseExecutionStrategy::Auto);
    assert_eq!(request.max_chars, 500);
}

#[tokio::test]
async fn execute_fast_search_returns_ranked_matches() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("auth.rs"),
        "fn verify_token(token: &str) {}",
    )
    .unwrap();
    let output = execute_fast_search(
        "verify token",
        dir.path().to_path_buf(),
        None,
        None,
        5,
        None,
        2,
    )
    .await
    .unwrap();
    assert!(output.success);
    assert!(output.content.contains("auth.rs"));
    assert_eq!(output.metadata.as_ref().unwrap()["mode"], json!("fast"));
}

#[tokio::test]
async fn execute_filename_search_returns_no_results_message() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
    let output = execute_filename_search("missing-auth", dir.path().to_path_buf(), 5, None)
        .await
        .unwrap();
    assert!(output.success);
    assert!(output.content.contains("No files found matching"));
}

#[tokio::test]
async fn execute_parse_request_returns_structural_summary_without_query() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Title\n\nBody text").unwrap();
    let ctx = crate::tools::ToolContext::new(dir.path().to_path_buf());
    let output = execute_parse_request(
        Arc::new(MockLlmClient),
        &ctx,
        &path,
        None,
        &ResolvedParseRequest {
            strategy: ParseExecutionStrategy::Auto,
            max_chars: 8_000,
        },
    )
    .await
    .unwrap();
    assert!(output.success);
    assert_eq!(
        output.metadata.as_ref().unwrap()["strategy"],
        json!("narrative")
    );
    assert!(output.content.contains("Structural Summary"));
}

#[tokio::test]
async fn execute_search_request_dispatches_filename_mode() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("auth.rs"),
        "fn verify_token(token: &str) {}",
    )
    .unwrap();
    let output = execute_filename_search("auth", dir.path().to_path_buf(), 5, None)
        .await
        .unwrap();
    assert!(output.success);
    assert_eq!(
        output.metadata.as_ref().unwrap()["mode"],
        json!("filename_only")
    );
    assert!(output.content.contains("auth.rs"));
}
