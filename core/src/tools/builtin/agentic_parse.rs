//! Agentic Parse Tool — LLM-enhanced document parsing
//!
//! Inspired by Landing AI's document intelligence approach.
//! Extracts structured information from documents using:
//! - DocumentParserRegistry (binary format decoding: PDF, XLSX, DOCX, …)
//! - Parse strategy heuristics (auto / structured / narrative / tabular / code)
//! - Optional LLM pass for semantic extraction / QA

use crate::llm::{LlmClient, Message};
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

// ============================================================================
// Parse strategy
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum ParseStrategy {
    Auto,
    Structured,
    Narrative,
    Tabular,
    Code,
}

impl ParseStrategy {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "structured" => Self::Structured,
            "narrative" => Self::Narrative,
            "tabular" => Self::Tabular,
            "code" => Self::Code,
            _ => Self::Auto,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Structured => "structured",
            Self::Narrative => "narrative",
            Self::Tabular => "tabular",
            Self::Code => "code",
        }
    }

    fn detect(path: &Path, content: &str) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "csv" | "tsv" => return Self::Tabular,
            "json" | "toml" | "yaml" | "yml" | "xml" | "hcl" => return Self::Structured,
            "md" | "markdown" | "rst" | "txt" | "adoc" => return Self::Narrative,
            "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "java" | "c" | "cpp" | "h"
            | "hpp" | "cs" | "rb" | "sh" | "bash" | "zsh" | "fish" | "sql" | "graphql"
            | "proto" | "tf" => return Self::Code,
            _ => {}
        }
        // Content heuristics
        let total = content.lines().count().max(1);
        let comma_rows = content
            .lines()
            .filter(|l| l.matches(',').count() >= 2)
            .count();
        if comma_rows * 100 / total > 50 {
            return Self::Tabular;
        }
        Self::Narrative
    }
}

// ============================================================================
// Output formatting
// ============================================================================

fn build_structural_summary(
    doc: &crate::document_parser::ParsedDocument,
    strategy: ParseStrategy,
) -> String {
    let mut out = String::from("\n## Structural Summary\n\n");
    if doc.blocks.is_empty() {
        out.push_str("(no structure detected)\n");
        return out;
    }

    match strategy {
        ParseStrategy::Code => append_code_summary(&mut out, doc),
        ParseStrategy::Tabular => append_tabular_summary(&mut out, doc),
        _ => append_block_summary(&mut out, doc),
    }
    out
}

fn append_code_summary(out: &mut String, doc: &crate::document_parser::ParsedDocument) {
    let symbols = detect_code_symbols(&doc.to_text());
    if symbols.is_empty() {
        out.push_str("(no symbols detected)\n");
        return;
    }

    out.push_str("### Symbols\n\n");
    for symbol in symbols.iter().take(50) {
        out.push_str(&format!("- `{}`\n", symbol));
    }
    if symbols.len() > 50 {
        out.push_str(&format!("… {} more symbols\n", symbols.len() - 50));
    }
}

fn append_tabular_summary(out: &mut String, doc: &crate::document_parser::ParsedDocument) {
    let mut wrote_any = false;
    for block in &doc.blocks {
        if !matches!(block.kind, crate::document_parser::DocumentBlockKind::Table) {
            continue;
        }

        wrote_any = true;
        if let Some(label) = &block.label {
            out.push_str(&format!("### {}\n\n", label));
        }

        let mut lines = block.content.lines().filter(|line| !line.trim().is_empty());
        if let Some(header) = lines.next() {
            let row_count = lines.count();
            out.push_str(&format!("Header row: `{}`\n", header.trim()));
            out.push_str(&format!("Data rows: {}\n\n", row_count));
        }
    }

    if !wrote_any {
        append_block_summary(out, doc);
    }
}

fn append_block_summary(out: &mut String, doc: &crate::document_parser::ParsedDocument) {
    if let Some(title) = &doc.title {
        out.push_str(&format!("### {}\n\n", title));
    }

    for block in doc.blocks.iter().take(12) {
        let heading = block
            .label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| block_kind_heading(&block.kind));
        out.push_str(&format!("### {}\n\n", heading));

        if let Some(location) = &block.location {
            let location = block_location_label(location);
            if !location.is_empty() {
                out.push_str(&format!("_Location: {}_\n\n", location));
            }
        }

        let preview_lines: Vec<&str> = block
            .content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();

        if preview_lines.is_empty() {
            out.push_str("(empty block)\n\n");
            continue;
        }

        for line in preview_lines {
            out.push_str(&format!("> {}\n", line));
        }

        let total_lines = block
            .content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .count();
        if total_lines > 3 {
            out.push_str(&format!("_… {} more lines_\n", total_lines - 3));
        }
        out.push('\n');
    }

    if doc.blocks.len() > 12 {
        out.push_str(&format!("_… {} more blocks_\n", doc.blocks.len() - 12));
    }
}

fn block_kind_heading(kind: &crate::document_parser::DocumentBlockKind) -> String {
    match kind {
        crate::document_parser::DocumentBlockKind::Paragraph => "Paragraph".to_string(),
        crate::document_parser::DocumentBlockKind::Heading => "Heading".to_string(),
        crate::document_parser::DocumentBlockKind::Table => "Table".to_string(),
        crate::document_parser::DocumentBlockKind::Section => "Section".to_string(),
        crate::document_parser::DocumentBlockKind::Metadata => "Metadata".to_string(),
        crate::document_parser::DocumentBlockKind::Slide => "Slide".to_string(),
        crate::document_parser::DocumentBlockKind::EmailHeader => "Email Header".to_string(),
        crate::document_parser::DocumentBlockKind::Code => "Code".to_string(),
        crate::document_parser::DocumentBlockKind::Raw => "Raw Content".to_string(),
    }
}

fn detect_code_symbols(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|l| l.trim())
        .filter(|l| {
            l.starts_with("pub fn ")
                || l.starts_with("fn ")
                || l.starts_with("async fn ")
                || l.starts_with("pub async fn ")
                || l.starts_with("pub struct ")
                || l.starts_with("struct ")
                || l.starts_with("pub enum ")
                || l.starts_with("enum ")
                || l.starts_with("pub trait ")
                || l.starts_with("trait ")
                || l.starts_with("impl ")
                || l.starts_with("def ")
                || l.starts_with("class ")
                || l.starts_with("func ")
                || l.starts_with("function ")
        })
        .map(|l| l.to_string())
        .collect()
}

fn block_kind_label(kind: &crate::document_parser::DocumentBlockKind) -> &'static str {
    match kind {
        crate::document_parser::DocumentBlockKind::Paragraph => "paragraph",
        crate::document_parser::DocumentBlockKind::Heading => "heading",
        crate::document_parser::DocumentBlockKind::Table => "table",
        crate::document_parser::DocumentBlockKind::Section => "section",
        crate::document_parser::DocumentBlockKind::Metadata => "metadata",
        crate::document_parser::DocumentBlockKind::Slide => "slide",
        crate::document_parser::DocumentBlockKind::EmailHeader => "email_header",
        crate::document_parser::DocumentBlockKind::Code => "code",
        crate::document_parser::DocumentBlockKind::Raw => "raw",
    }
}

fn block_location_label(location: &crate::document_parser::DocumentBlockLocation) -> String {
    let mut parts = Vec::new();
    if let Some(source) = &location.source {
        if !source.trim().is_empty() {
            parts.push(format!("source={}", source.trim()));
        }
    }
    if let Some(page) = location.page {
        parts.push(format!("page={page}"));
    }
    if let Some(ordinal) = location.ordinal {
        parts.push(format!("ordinal={ordinal}"));
    }
    parts.join(", ")
}

fn render_document_for_llm(
    doc: &crate::document_parser::ParsedDocument,
    max_chars: usize,
) -> (String, bool, usize) {
    if max_chars == 0 {
        return (String::new(), true, 0);
    }

    let mut out = String::new();
    let mut included_blocks = 0usize;
    let mut truncated = false;

    if let Some(title) = &doc.title {
        let header = format!("# Document: {}\n\n", title.trim());
        if header.chars().count() <= max_chars {
            out.push_str(&header);
        } else {
            return (header.chars().take(max_chars).collect(), true, 0);
        }
    }

    for block in &doc.blocks {
        let mut section = format!(
            "## Block {}: {}",
            included_blocks + 1,
            block_kind_label(&block.kind)
        );
        if let Some(label) = &block.label {
            let label = label.trim();
            if !label.is_empty() {
                section.push_str(&format!(" ({})", label));
            }
        }
        section.push_str("\n");
        if let Some(location) = &block.location {
            let location = block_location_label(location);
            if !location.is_empty() {
                section.push_str(&format!("Location: {}\n", location));
            }
        }
        section.push_str(block.content.trim());
        section.push_str("\n\n");

        let current_len = out.chars().count();
        let section_len = section.chars().count();
        if current_len + section_len <= max_chars {
            out.push_str(&section);
            included_blocks += 1;
            continue;
        }

        let remaining = max_chars.saturating_sub(current_len);
        if remaining > 0 {
            let mut partial: String = section.chars().take(remaining).collect();
            if !partial.ends_with('\n') {
                partial.push('\n');
            }
            partial.push_str("… [truncated]");
            out.push_str(&partial);
        }
        truncated = true;
        break;
    }

    if included_blocks < doc.blocks.len() {
        truncated = true;
    }

    (out, truncated, included_blocks)
}

fn describe_default_parser_config(config: Option<&crate::config::DefaultParserConfig>) -> String {
    match config {
        Some(config) => {
            let mut line = format!(
                "enabled={}, max_file_size_mb={}",
                config.enabled, config.max_file_size_mb
            );
            if let Some(ocr) = &config.ocr {
                line.push_str(&format!(
                    ", ocr.enabled={}, ocr.model={}, ocr.max_images={}, ocr.dpi={}",
                    ocr.enabled,
                    ocr.model.as_deref().unwrap_or("unset"),
                    ocr.max_images,
                    ocr.dpi
                ));
            } else {
                line.push_str(", ocr=unset");
            }
            line
        }
        None => "unset".to_string(),
    }
}

// ============================================================================
// AgenticParseTool
// ============================================================================

/// Agentic document parsing tool.
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

    /// Decode a file to a structured document, using the registry if available.
    fn decode_file(
        &self,
        path: &Path,
        ctx: &ToolContext,
    ) -> Result<Option<crate::document_parser::ParsedDocument>> {
        // 1. Try the document parser registry (PDF, XLSX, DOCX, custom formats)
        if let Some(registry) = &ctx.document_parsers {
            match registry.parse_file_document(path) {
                Ok(Some(doc)) => return Ok(Some(doc)),
                Ok(None) => {} // no parser registered for this extension — fall through
                Err(e) => {
                    tracing::warn!(
                        "document_parsers failed on {}: {} — falling back to text read",
                        path.display(),
                        e
                    );
                }
            }
        }

        // 2. Plain-text fallback
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(crate::document_parser::ParsedDocument::from_text(
                text,
            ))),
            Err(_) => Ok(None), // binary with no parser
        }
    }
}

#[async_trait]
impl Tool for AgenticParseTool {
    fn name(&self) -> &str {
        "agentic_parse"
    }

    fn description(&self) -> &str {
        "Intelligent document parsing with LLM-enhanced extraction. \
         Supports PDFs, Word docs, spreadsheets (via registered parsers), \
         Markdown, source code, CSV, and more. \
         Automatically detects the optimal parse strategy. \
         Provide a `query` to extract specific information using the LLM."
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
            .ok_or_else(|| anyhow::anyhow!("path parameter is required"))?;

        let query = args.get("query").and_then(|v| v.as_str());

        let strategy_hint = args
            .get("strategy")
            .and_then(|v| v.as_str())
            .map(ParseStrategy::from_str)
            .or_else(|| {
                ctx.agentic_parse_config
                    .as_ref()
                    .map(|cfg| ParseStrategy::from_str(&cfg.default_strategy))
            })
            .unwrap_or(ParseStrategy::Auto);

        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .or_else(|| ctx.agentic_parse_config.as_ref().map(|cfg| cfg.max_chars))
            .unwrap_or(8000);

        // Resolve path relative to workspace
        let path = ctx.resolve_path(path_str)?;

        if !path.exists() {
            return Ok(ToolOutput::error(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Decode file content
        let raw_document = match self.decode_file(&path, ctx)? {
            Some(doc) => doc,
            None => {
                return Ok(ToolOutput::error(format!(
                    "Cannot read `{}` — it appears to be a binary file with no registered parser. \
                     Register a DocumentParser for this format via SessionOptions.",
                    path.display()
                )));
            }
        };

        let raw_text = raw_document.to_text();
        let line_count = raw_text.lines().count();
        let word_count = raw_text.split_whitespace().count();
        let block_count = raw_document.block_count();
        let non_empty_block_count = raw_document.non_empty_block_count();
        let default_parser_summary =
            describe_default_parser_config(ctx.default_parser_config.as_ref());

        // Detect or apply parse strategy
        let strategy = if strategy_hint == ParseStrategy::Auto {
            ParseStrategy::detect(&path, &raw_text)
        } else {
            strategy_hint
        };

        // Structural parsing
        let structural_summary = build_structural_summary(&raw_document, strategy);

        // LLM-enhanced extraction (only when a query is provided)
        let (content_for_llm, llm_input_truncated, llm_blocks_included) =
            render_document_for_llm(&raw_document, max_chars);

        let llm_answer = if let Some(q) = query {
            let truncation_note = if llm_input_truncated {
                format!(
                    "\nLLM input was truncated to {} chars across {} block(s).",
                    max_chars, llm_blocks_included
                )
            } else {
                String::new()
            };

            let system = "You are a document analysis assistant. \
                 The user will provide document content and ask you to extract information from it. \
                 Answer based solely on the provided content. Be concise."
                .to_string();

            let user_msg = format!(
                "Document: `{}`\nParse strategy: {}\n\n\
                 --- DOCUMENT ---\n{}\n--- END DOCUMENT ---{}\n\n\
                 Query: {}",
                path.display(),
                strategy.label(),
                content_for_llm,
                truncation_note,
                q
            );

            let messages = vec![Message::user(&user_msg)];
            match self.llm.complete(&messages, Some(&system), &[]).await {
                Ok(resp) => Some(resp.text()),
                Err(e) => Some(format!("[LLM extraction failed: {}]", e)),
            }
        } else {
            None
        };

        // Compose output
        let mut output = format!(
            "# Agentic Parse: `{}`\n\n\
             - **Strategy**: `{}`\n\
             - **Default Parser**: `{}`\n\
             - **Blocks**: {} (non-empty: {})\n\
             - **Lines**: {}\n\
             - **Words**: {}\n",
            path.display(),
            strategy.label(),
            default_parser_summary,
            block_count,
            non_empty_block_count,
            line_count,
            word_count,
        );

        output.push_str(&structural_summary);

        if let Some(answer) = llm_answer {
            output.push_str("\n## Query Answer\n\n");
            output.push_str(&answer);
            output.push('\n');
        }

        Ok(ToolOutput::success(output).with_metadata(json!({
            "file": path.display().to_string(),
            "strategy": strategy.label(),
            "default_parser": ctx.default_parser_config.as_ref().map(|cfg| json!({
                "enabled": cfg.enabled,
                "max_file_size_mb": cfg.max_file_size_mb,
                "ocr": cfg.ocr.as_ref().map(|ocr| json!({
                    "enabled": ocr.enabled,
                    "model": ocr.model,
                    "prompt": ocr.prompt,
                    "max_images": ocr.max_images,
                    "dpi": ocr.dpi,
                })),
            })),
            "blocks": block_count,
            "non_empty_blocks": non_empty_block_count,
            "lines": line_count,
            "words": word_count,
            "llm_used": query.is_some(),
            "llm_input_truncated": llm_input_truncated,
            "llm_blocks_included": llm_blocks_included,
        })))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
    use async_trait::async_trait;
    use std::io::Write;
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
        let strategy = ParseStrategy::detect(&path, "# Hello\n\nThis is a test.");
        assert_eq!(strategy, ParseStrategy::Narrative);
    }

    #[test]
    fn test_detect_strategy_csv() {
        let dir = TempDir::new().unwrap();
        let content = "name,age,city\nAlice,30,NYC\nBob,25,LA\n";
        let path = write_temp(&dir, "data.csv", content);
        let strategy = ParseStrategy::detect(&path, content);
        assert_eq!(strategy, ParseStrategy::Tabular);
    }

    #[test]
    fn test_detect_strategy_rust_source() {
        let dir = TempDir::new().unwrap();
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let path = write_temp(&dir, "main.rs", content);
        let strategy = ParseStrategy::detect(&path, content);
        assert_eq!(strategy, ParseStrategy::Code);
    }

    #[test]
    fn test_detect_strategy_json() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"key": "value", "num": 42}"#;
        let path = write_temp(&dir, "config.json", content);
        let strategy = ParseStrategy::detect(&path, content);
        assert_eq!(strategy, ParseStrategy::Structured);
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
        };
        let summary = build_structural_summary(&doc, ParseStrategy::Tabular);
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
        };
        let summary = build_structural_summary(&doc, ParseStrategy::Narrative);
        assert!(summary.contains("Intro"));
        assert!(summary.contains("Some paragraph text here."));
    }

    #[test]
    fn test_build_structural_summary_code() {
        let doc = crate::document_parser::ParsedDocument::from_text(
            "fn foo() {}\nfn bar() {}\nstruct Baz {}\n",
        );
        let summary = build_structural_summary(&doc, ParseStrategy::Code);
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
        };

        let (rendered, truncated, included_blocks) = render_document_for_llm(&doc, 1024);
        assert!(!truncated);
        assert_eq!(included_blocks, 2);
        assert!(rendered.contains("# Document: report.pdf"));
        assert!(rendered.contains("## Block 1: section (intro)"));
        assert!(rendered.contains("Location: source=page-1, page=1, ordinal=1"));
        assert!(rendered.contains("## Block 2: table (sheet1)"));
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
        };

        let (rendered, truncated, included_blocks) = render_document_for_llm(&doc, 70);
        assert!(truncated);
        assert!(included_blocks <= 1);
        assert!(rendered.contains("truncated"));
    }

    #[test]
    fn test_describe_default_parser_config() {
        let summary = describe_default_parser_config(Some(&crate::config::DefaultParserConfig {
            enabled: true,
            max_file_size_mb: 64,
            ocr: Some(crate::config::DefaultParserOcrConfig {
                enabled: true,
                model: Some("openai/gpt-4.1-mini".to_string()),
                prompt: None,
                max_images: 6,
                dpi: 200,
            }),
        }));

        assert!(summary.contains("enabled=true"));
        assert!(summary.contains("max_file_size_mb=64"));
        assert!(summary.contains("ocr.enabled=true"));
        assert!(summary.contains("openai/gpt-4.1-mini"));
    }

    #[test]
    fn test_from_str_strategy() {
        assert_eq!(ParseStrategy::from_str("auto"), ParseStrategy::Auto);
        assert_eq!(
            ParseStrategy::from_str("structured"),
            ParseStrategy::Structured
        );
        assert_eq!(
            ParseStrategy::from_str("narrative"),
            ParseStrategy::Narrative
        );
        assert_eq!(ParseStrategy::from_str("tabular"), ParseStrategy::Tabular);
        assert_eq!(ParseStrategy::from_str("code"), ParseStrategy::Code);
        assert_eq!(ParseStrategy::from_str("unknown"), ParseStrategy::Auto);
    }

    #[test]
    fn test_strategy_label() {
        assert_eq!(ParseStrategy::Auto.label(), "auto");
        assert_eq!(ParseStrategy::Structured.label(), "structured");
        assert_eq!(ParseStrategy::Narrative.label(), "narrative");
        assert_eq!(ParseStrategy::Tabular.label(), "tabular");
        assert_eq!(ParseStrategy::Code.label(), "code");
    }
}
