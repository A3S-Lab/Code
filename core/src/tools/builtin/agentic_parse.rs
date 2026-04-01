//! Agentic Parse Tool — LLM-enhanced document parsing
//!
//! Extracts document context for A3S Code using:
//! - DocumentParserRegistry (binary format decoding: PDF, XLSX, DOCX, …)
//! - Parse strategy heuristics (auto / structured / narrative / tabular / code)
//! - Optional LLM pass for semantic extraction / QA

use crate::llm::LlmClient;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

/// Agentic document parsing tool for context recovery.
///
/// Combines `DocumentParserRegistry` (binary format decoding) with structural
/// extraction and optional LLM-enhanced semantic extraction / QA.
pub struct AgenticParseTool {
    llm: Arc<dyn LlmClient>,
}

impl AgenticParseTool {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl Tool for AgenticParseTool {
    fn name(&self) -> &str {
        "agentic_parse"
    }

    fn description(&self) -> &str {
        "Document context extraction with optional LLM-assisted answering. \
         Supports PDFs, Word docs, spreadsheets (via registered parsers), \
         Markdown, source code, CSV, and more. \
         Automatically selects a parse strategy for context recovery. \
         Provide a `query` to extract specific information from the prepared content."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Required. Path to the file to parse, relative to the workspace or absolute. Always provide this exact field name: 'path'."
                },
                "query": {
                    "type": "string",
                    "description": "Optional. Extraction goal or question, for example 'What are the key findings?'. Triggers LLM-enhanced extraction."
                },
                "strategy": {
                    "type": "string",
                    "enum": ["auto", "structured", "narrative", "tabular", "code"],
                    "description": "Optional. Parse strategy. Default: auto."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Optional. Maximum characters of document content sent to the LLM. Default: 8000."
                }
            },
            "required": ["path"],
            "examples": [
                {
                    "path": "README.md"
                },
                {
                    "path": "report.pdf",
                    "query": "What are the key findings?",
                    "strategy": "narrative",
                    "max_chars": 6000
                }
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("path parameter is required"))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let resolved = crate::document_service_types::resolve_parse_request(
            args,
            ctx.agentic_parse_config.as_ref(),
        );

        // Resolve path relative to workspace
        let path = ctx.resolve_path(path_str)?;

        crate::document_parse_engine::execute_parse_request(
            Arc::clone(&self.llm),
            ctx,
            &path,
            query,
            &resolved,
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
    use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
    use async_trait::async_trait;
    use serde_json::json;
    use std::io::Write;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    struct MockLlmClient;

    struct MockOcrDocumentParser;

    impl DocumentParser for MockOcrDocumentParser {
        fn name(&self) -> &str {
            "mock-ocr-doc"
        }

        fn supported_extensions(&self) -> &[&str] {
            &["pdf"]
        }

        fn parse(&self, _path: &Path) -> anyhow::Result<String> {
            Ok("Recovered body text".to_string())
        }

        fn parse_extracted(
            &self,
            _path: &Path,
        ) -> anyhow::Result<crate::document_pipeline::ExtractedDocument> {
            Ok(crate::document_pipeline::ExtractedDocument::new(
                crate::document_parser::ParsedDocument {
                title: Some("scan.pdf".to_string()),
                blocks: vec![
                    crate::document_parser::DocumentBlock::new(
                        crate::document_parser::DocumentBlockKind::Metadata,
                        Some("ocr"),
                        "mode=ocr\nformat=pdf\nprovider=mock-ocr\nmodel=moonshot/kimi-vl\nprompt=set\nmax_images=4\ndpi=180",
                    ),
                    crate::document_parser::DocumentBlock::new(
                        crate::document_parser::DocumentBlockKind::Paragraph,
                        Some("body"),
                        "Recovered body text",
                    ),
                ],
                metadata: None,
            },
            ))
        }
    }

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

    fn write_temp(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{}", content).unwrap();
        path
    }

    #[test]
    fn test_detect_strategy_markdown() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "readme.md", "# Hello\n\nThis is a test.");
        let strategy = crate::document_service_types::detect_parse_strategy(
            &path,
            "# Hello\n\nThis is a test.",
        );
        assert_eq!(
            strategy,
            crate::document_service_types::ParseExecutionStrategy::Narrative
        );
    }

    #[test]
    fn test_detect_strategy_csv() {
        let dir = TempDir::new().unwrap();
        let content = "name,age,city\nAlice,30,NYC\nBob,25,LA\n";
        let path = write_temp(&dir, "data.csv", content);
        let strategy = crate::document_service_types::detect_parse_strategy(&path, content);
        assert_eq!(
            strategy,
            crate::document_service_types::ParseExecutionStrategy::Tabular
        );
    }

    #[test]
    fn test_detect_strategy_rust_source() {
        let dir = TempDir::new().unwrap();
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let path = write_temp(&dir, "main.rs", content);
        let strategy = crate::document_service_types::detect_parse_strategy(&path, content);
        assert_eq!(
            strategy,
            crate::document_service_types::ParseExecutionStrategy::Code
        );
    }

    #[test]
    fn test_detect_strategy_json() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"key": "value", "num": 42}"#;
        let path = write_temp(&dir, "config.json", content);
        let strategy = crate::document_service_types::detect_parse_strategy(&path, content);
        assert_eq!(
            strategy,
            crate::document_service_types::ParseExecutionStrategy::Structured
        );
    }

    #[test]
    fn test_agentic_parse_schema_is_canonical() {
        let tool = AgenticParseTool::new(Arc::new(MockLlmClient));
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["path"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["path"], "README.md");
        assert!(examples[0].get("file_path").is_none());
    }

    #[test]
    fn test_build_structural_summary_tabular_blocks() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("table.csv".to_string()),
            blocks: vec![crate::document_parser::DocumentBlock::new(
                crate::document_parser::DocumentBlockKind::Table,
                Some("sheet1"),
                "col1,col2,col3\nA,B,C\nD,E,F\n",
            )],
            metadata: None,
        };
        let summary = crate::document_consume::build_structural_summary(
            &doc,
            crate::document_consume::StructuralSummaryStyle::Tabular,
        );
        assert!(summary.contains("Header row"));
        assert!(summary.contains("Data rows: 2"));
    }

    #[test]
    fn test_build_structural_summary_narrative_blocks() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("doc.md".to_string()),
            blocks: vec![
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Heading,
                    Some("Title"),
                    "Title",
                ),
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Paragraph,
                    Some("Intro"),
                    "Some paragraph text here.\nAnother line.\nThird line.\nFourth line.",
                ),
            ],
            metadata: None,
        };
        let summary = crate::document_consume::build_structural_summary(
            &doc,
            crate::document_consume::StructuralSummaryStyle::Narrative,
        );
        assert!(summary.contains("Intro"));
        assert!(summary.contains("Some paragraph text here."));
    }

    #[test]
    fn test_build_structural_summary_code() {
        let doc = crate::document_parser::ParsedDocument::from_text(
            "fn foo() {}\nfn bar() {}\nstruct Baz {}\n",
        );
        let summary = crate::document_consume::build_structural_summary(
            &doc,
            crate::document_consume::StructuralSummaryStyle::Code,
        );
        assert!(
            summary.contains("Structural") || summary.contains("Symbol") || !summary.is_empty()
        );
    }

    #[test]
    fn test_render_document_for_llm_preserves_block_boundaries() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Section,
                    Some("intro"),
                    "hello world",
                )
                .with_source("page-1")
                .with_page(1)
                .with_ordinal(1),
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Table,
                    Some("sheet1"),
                    "a,b\n1,2",
                ),
            ],
            metadata: None,
        };

        let rendered = crate::document_consume::render_document_for_llm(
            &doc,
            1024,
            None,
            None,
            crate::document_service_types::document_block_kind_label,
        );
        assert!(!rendered.truncated);
        assert_eq!(rendered.included_blocks, 2);
        assert_eq!(rendered.included_indices, vec![0, 1]);
        assert!(rendered.content.contains("# Document: report.pdf"));
        assert!(rendered.content.contains("## Block 1: section (intro)"));
        assert!(rendered
            .content
            .contains("Location: source=page-1, page=1, ordinal=1"));
        assert!(rendered.content.contains("## Block 2: table (sheet1)"));
    }

    #[test]
    fn test_render_document_for_llm_truncates_by_block_budget() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Section,
                    Some("intro"),
                    "hello world",
                ),
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Section,
                    Some("details"),
                    "this is a much longer block that should force truncation",
                ),
            ],
            metadata: None,
        };

        let rendered = crate::document_consume::render_document_for_llm(
            &doc,
            70,
            None,
            None,
            crate::document_service_types::document_block_kind_label,
        );
        assert!(rendered.truncated);
        assert!(rendered.included_blocks <= 1);
        assert!(rendered.content.contains("truncated"));
        assert!(rendered.content.contains("## Block 1: section (intro)"));
    }

    #[test]
    fn test_render_document_for_llm_preserves_block_preview_when_truncated() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![crate::document_parser::DocumentBlock::new(
                crate::document_parser::DocumentBlockKind::Section,
                Some("details"),
                "line one\nline two\nline three\nline four",
            )],
            metadata: None,
        };

        let rendered = crate::document_consume::render_document_for_llm(
            &doc,
            90,
            None,
            None,
            crate::document_service_types::document_block_kind_label,
        );
        assert!(rendered.truncated);
        assert_eq!(rendered.included_blocks, 0);
        assert!(rendered.content.contains("## Block 1: section (details)"));
        assert!(rendered.content.contains("line one"));
        assert!(rendered.content.contains("[truncated]"));
    }

    #[test]
    fn test_render_document_for_llm_prioritizes_query_relevant_blocks() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Section,
                    Some("introduction"),
                    "general overview and background",
                ),
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Table,
                    Some("security findings"),
                    "critical vulnerability in auth token validation",
                ),
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Section,
                    Some("appendix"),
                    "extra notes",
                ),
            ],
            metadata: None,
        };

        let rendered = crate::document_consume::render_document_for_llm(
            &doc,
            160,
            Some("security vulnerability"),
            None,
            crate::document_service_types::document_block_kind_label,
        );
        assert!(rendered.included_blocks >= 1);
        assert!(rendered
            .content
            .contains("## Block 2: table (security findings)"));
        assert_eq!(rendered.included_indices.first().copied(), Some(1));
    }

    #[test]
    fn test_llm_block_metadata_includes_location_display() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![crate::document_parser::DocumentBlock::new(
                crate::document_parser::DocumentBlockKind::Section,
                Some("page 2: 1. Overview"),
                "hello world",
            )
            .with_source("report.pdf")
            .with_page(2)
            .with_ordinal(4)],
            metadata: None,
        };

        let metadata = crate::document_consume::llm_block_metadata(
            &doc,
            &[0],
            crate::document_service_types::document_block_kind_label,
        );
        assert_eq!(metadata[0]["index"], json!(1));
        assert_eq!(metadata[0]["kind"], json!("section"));
        assert_eq!(metadata[0]["label"], json!("page 2: 1. Overview"));
        assert_eq!(
            metadata[0]["location"]["display"],
            json!("source=report.pdf, page=2, ordinal=4")
        );
    }

    #[test]
    fn test_llm_block_metadata_includes_page_continuation_flags() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![crate::document_parser::DocumentBlock::new(
                crate::document_parser::DocumentBlockKind::Section,
                Some("page 2: 1. Overview"),
                "hello world",
            )
            .with_source("report.pdf")
            .with_page(2)
            .with_ordinal(4)
            .with_continued_from_previous_page(true)
            .with_continued_to_next_page(true)],
            metadata: None,
        };

        let metadata = crate::document_consume::llm_block_metadata(
            &doc,
            &[0],
            crate::document_service_types::document_block_kind_label,
        );
        assert_eq!(
            metadata[0]["location"]["continued_from_previous_page"],
            json!(true)
        );
        assert_eq!(
            metadata[0]["location"]["continued_to_next_page"],
            json!(true)
        );
        assert_eq!(
            metadata[0]["location"]["display"],
            json!(
                "source=report.pdf, page=2, ordinal=4, continued_from_previous_page=true, continued_to_next_page=true"
            )
        );
    }

    #[test]
    fn test_describe_document_parser_config() {
        let summary = crate::document_consume::describe_document_parser_config(Some(
            &crate::config::DocumentParserConfig {
                enabled: true,
                max_file_size_mb: 64,
                ocr: Some(crate::config::DocumentOcrConfig {
                    enabled: true,
                    model: Some("openai/gpt-4.1-mini".to_string()),
                    prompt: Some("Extract tables faithfully".to_string()),
                    max_images: 6,
                    dpi: 200,
                }),
                ..Default::default()
            },
        ));

        assert!(summary.contains("enabled=true"));
        assert!(summary.contains("max_file_size_mb=64"));
        assert!(summary.contains("ocr.enabled=true"));
        assert!(summary.contains("openai/gpt-4.1-mini"));
        assert!(summary.contains("ocr.prompt=set"));
        assert!(summary.contains("ocr.max_images=6"));
        assert!(summary.contains("ocr.dpi=200"));
    }

    #[test]
    fn test_extract_document_runtime_metadata_from_ocr_block() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("scan.pdf".to_string()),
            blocks: vec![
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Metadata,
                    Some("ocr"),
                    "mode=ocr\nformat=pdf\nprovider=mock-ocr\nmodel=moonshot/kimi-vl\nprompt=set\nmax_images=4\ndpi=180",
                ),
                crate::document_parser::DocumentBlock::new(
                    crate::document_parser::DocumentBlockKind::Paragraph,
                    Some("body"),
                    "Recovered body text",
                ),
            ],
            metadata: None,
        };

        let metadata = crate::document_consume::extract_document_runtime_metadata(&doc).unwrap();
        assert_eq!(metadata["ocr"]["used"], true);
        assert_eq!(metadata["ocr"]["format"], "pdf");
        assert_eq!(metadata["ocr"]["provider"], "mock-ocr");
        assert_eq!(metadata["ocr"]["model"], "moonshot/kimi-vl");
        assert_eq!(metadata["ocr"]["prompt"], "set");
        assert_eq!(metadata["ocr"]["max_images"], 4);
        assert_eq!(metadata["ocr"]["dpi"], 180);
    }

    #[test]
    fn test_extract_document_runtime_metadata_returns_none_without_ocr_block() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("notes.txt".to_string()),
            blocks: vec![crate::document_parser::DocumentBlock::new(
                crate::document_parser::DocumentBlockKind::Paragraph,
                Some("body"),
                "Plain text only",
            )],
            metadata: None,
        };

        assert!(crate::document_consume::extract_document_runtime_metadata(&doc).is_none());
    }

    #[test]
    fn test_summarize_document_runtime() {
        let metadata = serde_json::json!({
            "ocr": {
                "used": true,
                "format": "image",
                "provider": "mock-ocr",
                "model": "moonshot/kimi-vl",
                "max_images": 4,
                "dpi": 180
            }
        });
        let summary = crate::document_consume::summarize_document_runtime(Some(&metadata)).unwrap();
        assert!(summary.contains("`image`"));
        assert!(summary.contains("mock-ocr"));
        assert!(summary.contains("moonshot/kimi-vl"));
        assert!(summary.contains("max_images=4"));
    }

    #[tokio::test]
    async fn test_execute_surfaces_document_runtime_metadata() {
        let dir = TempDir::new().unwrap();
        let file_path = write_temp(&dir, "scan.pdf", "fake pdf");

        let mut registry = crate::document_parser::DocumentParserRegistry::empty();
        registry.register(Arc::new(MockOcrDocumentParser));

        let ctx = ToolContext::new(dir.path().to_path_buf())
            .with_document_parsers(Arc::new(registry))
            .with_document_parser_config(crate::config::DocumentParserConfig {
                enabled: true,
                max_file_size_mb: 64,
                ocr: Some(crate::config::DocumentOcrConfig {
                    enabled: true,
                    model: Some("moonshot/kimi-vl".to_string()),
                    prompt: Some("Read scan".to_string()),
                    max_images: 4,
                    dpi: 180,
                }),
                ..Default::default()
            });

        let tool = AgenticParseTool::new(Arc::new(MockLlmClient));
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": file_path.file_name().unwrap().to_string_lossy().to_string()
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Runtime"));
        assert!(result.content.contains("OCR used for"));
        assert!(result.content.contains("mock-ocr"));
        assert!(result.content.contains("moonshot/kimi-vl"));
        let runtime = result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("document_runtime"))
            .unwrap();
        assert_eq!(runtime["ocr"]["format"], "pdf");
        assert_eq!(runtime["ocr"]["provider"], "mock-ocr");
        assert_eq!(runtime["ocr"]["model"], "moonshot/kimi-vl");
    }

    #[tokio::test]
    async fn test_execute_surfaces_llm_block_metadata() {
        let dir = TempDir::new().unwrap();
        let file_path = write_temp(&dir, "report.md", "# Overview\n\nBody text");

        let tool = AgenticParseTool::new(Arc::new(MockLlmClient));
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": file_path.file_name().unwrap().to_string_lossy().to_string(),
                    "query": "overview"
                }),
                &ToolContext::new(dir.path().to_path_buf()),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("LLM Blocks"));
        let llm_blocks = result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("llm_blocks"))
            .and_then(|value| value.as_array())
            .expect("llm_blocks should be present");
        assert!(!llm_blocks.is_empty());
        assert!(llm_blocks[0].get("kind").is_some());
    }

    #[test]
    fn test_llm_block_metadata_includes_structured_payload() {
        let doc = crate::document_parser::ParsedDocument {
            title: Some("sheet.xlsx".to_string()),
            blocks: vec![crate::document_parser::DocumentBlock::new(
                crate::document_parser::DocumentBlockKind::Table,
                Some("sheet1"),
                "name\tvalue\nfoo\t1",
            )
            .with_structured_payload(r#"{"headers":["name","value"],"rows":[["foo","1"]]}"#)],
            metadata: None,
        };

        let blocks = crate::document_consume::llm_block_metadata(
            &doc,
            &[0],
            crate::document_service_types::document_block_kind_label,
        );

        assert_eq!(
            blocks[0]["structured_payload"]["headers"][0],
            serde_json::json!("name")
        );
    }

    #[tokio::test]
    async fn test_execute_surfaces_document_quality_metadata() {
        let dir = TempDir::new().unwrap();
        let file_path = write_temp(&dir, "tiny.txt", "A");

        let tool = AgenticParseTool::new(Arc::new(MockLlmClient));
        let ctx = ToolContext::new(dir.path().to_path_buf()).with_document_pipeline(Arc::new(
            crate::document_pipeline_defaults::build_default_document_pipeline_registry_for_config(
                &crate::config::DocumentParserConfig {
                    cache: Some(crate::config::DocumentCacheConfig {
                        enabled: false,
                        directory: None,
                    }),
                    ..Default::default()
                },
            ),
        ));
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": file_path.file_name().unwrap().to_string_lossy().to_string()
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Quality"));
        assert!(result.content.contains("score="));
        let quality = result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("document_quality"))
            .unwrap();
        assert!(quality.get("score").is_some());
        assert!(quality.get("grade").is_some());
        assert_eq!(
            quality["issues"][0]["code"],
            serde_json::json!("content.short_blocks")
        );
    }

    #[tokio::test]
    async fn test_execute_surfaces_language_keywords_and_chunk_highlights() {
        let dir = TempDir::new().unwrap();
        let file_path = write_temp(
            &dir,
            "report.md",
            "# Revenue Overview\n\nRevenue growth accelerated across enterprise regions.\n\n## Security Findings\n\nToken validation failure affected login reliability.",
        );

        let tool = AgenticParseTool::new(Arc::new(MockLlmClient));
        let ctx = ToolContext::new(dir.path().to_path_buf()).with_document_pipeline(Arc::new(
            crate::document_pipeline_defaults::build_default_document_pipeline_registry_for_config(
                &crate::config::DocumentParserConfig {
                    cache: Some(crate::config::DocumentCacheConfig {
                        enabled: false,
                        directory: None,
                    }),
                    ..Default::default()
                },
            ),
        ));
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": file_path.file_name().unwrap().to_string_lossy().to_string(),
                    "query": "security token validation"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Language"));
        assert!(result.content.contains("Keywords"));
        assert!(result.content.contains("Key Chunks"));
        assert!(result.content.contains("Security Findings"));

        let metadata = result.metadata.as_ref().unwrap();
        assert_eq!(metadata["document_language"], json!("en"));
        assert!(metadata["document_keywords"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "security")));
        assert!(metadata["chunk_highlights"]
            .as_array()
            .is_some_and(|values| !values.is_empty()));
        assert_eq!(metadata["chunk_highlights"][0]["language"], json!("en"));
    }

    #[test]
    fn test_from_str_strategy() {
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::from_str("auto"),
            crate::document_service_types::ParseExecutionStrategy::Auto
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::from_str("structured"),
            crate::document_service_types::ParseExecutionStrategy::Structured
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::from_str("narrative"),
            crate::document_service_types::ParseExecutionStrategy::Narrative
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::from_str("tabular"),
            crate::document_service_types::ParseExecutionStrategy::Tabular
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::from_str("code"),
            crate::document_service_types::ParseExecutionStrategy::Code
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::from_str("unknown"),
            crate::document_service_types::ParseExecutionStrategy::Auto
        );
    }

    #[test]
    fn test_strategy_label() {
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::Auto.label(),
            "auto"
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::Structured.label(),
            "structured"
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::Narrative.label(),
            "narrative"
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::Tabular.label(),
            "tabular"
        );
        assert_eq!(
            crate::document_service_types::ParseExecutionStrategy::Code.label(),
            "code"
        );
    }
}
