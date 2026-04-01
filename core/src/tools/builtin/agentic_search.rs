//! Agentic Search Tool - Sirchmunk-inspired intelligent code search
//!
//! Implements multi-phase search pipeline:
//! - Phase 0: Semantic cache (future)
//! - Phase 1: Parallel probing (keywords + structure)
//! - Phase 2: Dual retrieval (content + structure)
//! - Phase 3: Result synthesis with relevance scoring

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum FileType {
    Code,
    Config,
    Documentation,
    Other,
}

#[cfg(test)]
impl FileType {
    fn from_path(path: &std::path::Path) -> Self {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext {
                "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "c" | "cpp" | "h"
                | "hpp" => Self::Code,
                "toml" | "yaml" | "yml" | "json" | "ini" | "conf" => Self::Config,
                "md" | "txt" | "rst" | "adoc" => Self::Documentation,
                _ => Self::Other,
            }
        } else {
            Self::Other
        }
    }
}

pub struct AgenticSearchTool;

impl AgenticSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AgenticSearchTool {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AgenticSearchTool {
    fn name(&self) -> &str {
        "agentic_search"
    }

    fn description(&self) -> &str {
        "Intelligent multi-phase code search. Extracts keywords from natural language queries, \
         searches file contents and structure in parallel, then ranks results by relevance. \
         Supports fast mode (default) and filename_only mode for quick file discovery."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Required. Natural language search query, for example 'JWT token validation' or 'how does authentication work'. Always provide this exact field name: 'query'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["fast", "deep", "filename_only"],
                    "description": "Optional. Search mode. Default: fast."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Optional. Maximum number of files to return. Default: 10."
                },
                "include": {
                    "type": "string",
                    "description": "Optional. Glob pattern to filter files, for example '*.rs' or '*.{ts,tsx}'."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Optional. Lines of context around each match. Default: 2."
                }
            },
            "required": ["query"],
            "examples": [
                {
                    "query": "JWT token validation"
                },
                {
                    "query": "authentication flow",
                    "mode": "deep",
                    "max_results": 8,
                    "include": "*.rs",
                    "context_lines": 3
                }
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("query parameter is required"))?;

        let request = crate::document_service_types::resolve_search_request(
            args,
            ctx.agentic_search_config.as_ref(),
        );
        crate::document_search_engine::execute_search_request(
            query,
            ctx.workspace.clone(),
            ctx.document_parsers.clone(),
            ctx.document_pipeline.clone(),
            request,
        )
        .await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_parser::DocumentParser;
    use regex::Regex;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn setup_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        let mut f = File::create(root.join("auth.rs")).unwrap();
        writeln!(f, "use jwt::Token;\n\npub fn verify_token(token: &str) -> Result<Claims> {{\n    // JWT verification\n    todo!()\n}}").unwrap();

        let mut f = File::create(root.join("session.rs")).unwrap();
        writeln!(f, "pub struct Session {{\n    pub user_id: String,\n    pub token: String,\n}}\n\nimpl Session {{\n    pub fn new(user_id: &str, token: &str) -> Self {{\n        Self {{ user_id: user_id.to_string(), token: token.to_string() }}\n    }}\n}}").unwrap();

        let mut f = File::create(root.join("README.md")).unwrap();
        writeln!(
            f,
            "# Auth Service\n\nHandles JWT authentication and session management."
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_extract_keywords_basic() {
        let kws = crate::document_consume::extract_search_keywords("JWT token validation");
        assert!(kws.iter().any(|k| k.contains("jwt") || k.contains("JWT")));
        assert!(kws.iter().any(|k| k.contains("token")));
        assert!(kws.iter().any(|k| k.contains("validation")));
    }

    #[test]
    fn test_extract_keywords_stop_words_removed() {
        let kws =
            crate::document_consume::extract_search_keywords("how does the authentication work");
        assert!(!kws.iter().any(|k| k == "how"));
        assert!(!kws.iter().any(|k| k == "the"));
        assert!(kws.iter().any(|k| k.contains("authentication")));
    }

    #[test]
    fn test_extract_keywords_deduplication() {
        let kws = crate::document_consume::extract_search_keywords("auth auth authentication");
        let unique: std::collections::HashSet<_> = kws.iter().collect();
        assert_eq!(kws.len(), unique.len());
    }

    #[test]
    fn test_extract_keywords_splits_identifier_variants() {
        let kws =
            crate::document_consume::extract_search_keywords("AuthTokenManager verify_user-token");
        assert!(kws.iter().any(|k| k == "auth"));
        assert!(kws.iter().any(|k| k == "token"));
        assert!(kws.iter().any(|k| k == "manager"));
        assert!(kws.iter().any(|k| k == "verify"));
        assert!(kws.iter().any(|k| k == "user"));
    }

    #[test]
    fn test_file_type_detection() {
        assert_eq!(
            FileType::from_path(std::path::Path::new("main.rs")),
            FileType::Code
        );
        assert_eq!(
            FileType::from_path(std::path::Path::new("config.toml")),
            FileType::Config
        );
        assert_eq!(
            FileType::from_path(std::path::Path::new("README.md")),
            FileType::Documentation
        );
    }

    #[test]
    fn test_agentic_search_schema_is_canonical() {
        let tool = AgenticSearchTool::new();
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["query"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["query"], "JWT token validation");
        assert!(examples[0].get("q").is_none());
    }

    #[tokio::test]
    async fn test_fast_search_finds_results() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "JWT token"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("auth.rs") || output.content.contains("session.rs"));
        assert!(output.content.contains("score: base="));
        let metadata = output.metadata.unwrap();
        assert!(metadata["results"].is_array());
    }

    #[tokio::test]
    async fn test_filename_only_mode() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "auth", "mode": "filename_only"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("auth.rs"));
    }

    #[test]
    fn test_path_signal_score_rewards_filename_matches() {
        let dir = setup_workspace();
        let workspace = dir.path();
        let patterns = vec![
            Regex::new("(?i)auth").unwrap(),
            Regex::new("(?i)rs").unwrap(),
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

    #[tokio::test]
    async fn test_no_results() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "xyznonexistentterm12345"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("No results"));
    }

    #[tokio::test]
    async fn test_tool_name() {
        let tool = AgenticSearchTool::new();
        assert_eq!(tool.name(), "agentic_search");
    }

    #[tokio::test]
    async fn test_max_results_respected() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "pub", "max_results": 1});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        if let Some(meta) = &output.metadata {
            let count = meta
                .get("result_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            assert!(count <= 1);
        }
    }

    #[tokio::test]
    async fn test_deep_mode() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "JWT token", "mode": "deep"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("evidence"));
        assert!(output.content.contains("score: base="));
        if let Some(meta) = &output.metadata {
            assert_eq!(meta.get("mode").and_then(|v| v.as_str()), Some("deep"));
            assert!(meta.get("initial_pool_size").is_some());
            assert!(meta.get("results").and_then(|v| v.as_array()).is_some());
        }
    }

    #[tokio::test]
    async fn test_deep_mode_monte_carlo_sampling() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "token", "mode": "deep", "max_results": 2});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        // Deep mode should show evidence scores
        assert!(output.content.contains("evidence:"));
    }

    #[test]
    fn test_build_search_lines_includes_block_labels() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![crate::document_parser::DocumentBlock::new(
                crate::document_parser::DocumentBlockKind::Table,
                Some("sheet1"),
                "name\tvalue\nfoo\t1",
            )
            .with_source("xl/worksheets/sheet1.xml")
            .with_ordinal(1)],
            metadata: None,
        };

        let lines = crate::document_consume::build_search_lines(&doc);
        assert!(lines.iter().any(|line| line.contains("# report.pdf")));
        assert!(lines
            .iter()
            .any(|line| line.contains("source=xl/worksheets/sheet1.xml")));
        assert!(lines.iter().any(|line| line.contains("[table] sheet1")));
        assert!(lines.iter().any(|line| line.contains("foo\t1")));
    }

    #[test]
    fn test_derive_match_locator_prefers_page_and_label_markers() {
        let lines = vec![
            "# scan.pdf".to_string(),
            "[loc] source=scan.pdf, page=2, ordinal=4".to_string(),
            "[section] page 2: 1. Overview".to_string(),
            "The parser now emits structured search labels.".to_string(),
        ];

        assert_eq!(
            crate::document_render::derive_match_locator(&lines, 3).as_deref(),
            Some("page 2 | page 2: 1. Overview")
        );
    }

    #[test]
    fn test_format_block_location_includes_page_continuation_flags() {
        let location = crate::document_parser::DocumentBlockLocation {
            source: Some("report.pdf".to_string()),
            page: Some(2),
            ordinal: Some(4),
            continued_from_previous_page: true,
            continued_to_next_page: true,
        };

        assert_eq!(
            crate::document_render::format_block_location(&location),
            "source=report.pdf, page=2, ordinal=4, continued_from_previous_page=true, continued_to_next_page=true"
        );
    }

    struct OcrDocParser;

    impl DocumentParser for OcrDocParser {
        fn name(&self) -> &str {
            "ocr-doc"
        }

        fn supported_extensions(&self) -> &[&str] {
            &["ocrdoc"]
        }

        fn parse(&self, _path: &std::path::Path) -> Result<String> {
            Ok("Document parser OCR body".to_string())
        }

        fn parse_extracted(
            &self,
            _path: &std::path::Path,
        ) -> Result<crate::document_pipeline::ExtractedDocument> {
            Ok(crate::document_pipeline::ExtractedDocument::new(
                crate::document_parser::ParsedDocument {
                title: Some("scan.ocrdoc".to_string()),
                blocks: vec![
                    crate::document_parser::DocumentBlock::new(
                        crate::document_parser::DocumentBlockKind::Metadata,
                        Some("ocr"),
                        "mode=ocr\nformat=pdf\nprovider=mock\nmodel=openai/gpt-4.1-mini\nprompt=set\nmax_images=2\ndpi=144",
                    )
                    .with_source("document_parser")
                    .with_ordinal(0),
                    crate::document_parser::DocumentBlock::new(
                        crate::document_parser::DocumentBlockKind::Paragraph,
                        Some("body"),
                        "Document parser OCR body",
                    )
                    .with_ordinal(1),
                ],
                metadata: None,
            },
            ))
        }
    }

    #[tokio::test]
    async fn test_fast_search_metadata_surfaces_document_runtime() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scan.ocrdoc");
        std::fs::write(&path, "placeholder").unwrap();

        let mut registry = crate::document_parser::DocumentParserRegistry::empty();
        registry.register(Arc::new(OcrDocParser));

        let ctx =
            ToolContext::new(dir.path().to_path_buf()).with_document_parsers(Arc::new(registry));
        let tool = AgenticSearchTool::new();

        let output = tool
            .execute(&json!({"query": "OCR body"}), &ctx)
            .await
            .unwrap();

        assert!(output.success);
        let metadata = output.metadata.expect("search metadata should be present");
        let runtime = &metadata["results"][0]["document_runtime"]["ocr"];
        assert_eq!(runtime["used"], json!(true));
        assert_eq!(runtime["provider"], json!("mock"));
        assert_eq!(runtime["format"], json!("pdf"));
        assert!(metadata["results"][0]["matches"].is_array());
    }
}
