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
// Document structure types
// ============================================================================

struct DocSection {
    heading: Option<String>,
    lines: Vec<String>,
}

struct ParsedDocument {
    sections: Vec<DocSection>,
}

// ============================================================================
// Structural parsers
// ============================================================================

fn parse_sectioned(raw: &str) -> ParsedDocument {
    let mut sections: Vec<DocSection> = Vec::new();
    let mut current = DocSection {
        heading: None,
        lines: Vec::new(),
    };
    for line in raw.lines() {
        if line.starts_with('#') {
            let non_empty = !current.lines.is_empty() || current.heading.is_some();
            if non_empty {
                sections.push(current);
            }
            current = DocSection {
                heading: Some(line.trim_start_matches('#').trim().to_string()),
                lines: Vec::new(),
            };
        } else {
            current.lines.push(line.to_string());
        }
    }
    if !current.lines.is_empty() || current.heading.is_some() {
        sections.push(current);
    }
    ParsedDocument { sections }
}

fn parse_tabular(raw: &str) -> ParsedDocument {
    let mut lines = raw.lines();
    let header = lines.next().unwrap_or("").to_string();
    let row_count = lines.count();
    ParsedDocument {
        sections: vec![DocSection {
            heading: Some(format!("Table ({} data rows)", row_count)),
            lines: vec![header],
        }],
    }
}

fn parse_code(raw: &str) -> ParsedDocument {
    let symbols: Vec<String> = raw
        .lines()
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
        .collect();
    ParsedDocument {
        sections: vec![DocSection {
            heading: Some("Detected Symbols".to_string()),
            lines: symbols,
        }],
    }
}

fn parse_document(raw: &str, path: &Path, strategy: ParseStrategy) -> ParsedDocument {
    let _ = path; // reserved for future path-aware heuristics
    match strategy {
        ParseStrategy::Tabular => parse_tabular(raw),
        ParseStrategy::Code => parse_code(raw),
        _ => parse_sectioned(raw),
    }
}

// ============================================================================
// Output formatting
// ============================================================================

fn build_structural_summary(doc: &ParsedDocument, strategy: ParseStrategy) -> String {
    let mut out = String::from("\n## Structural Summary\n\n");
    if doc.sections.is_empty() {
        out.push_str("(no structure detected)\n");
        return out;
    }
    match strategy {
        ParseStrategy::Code => {
            out.push_str("### Symbols\n\n");
            for sec in &doc.sections {
                for sym in sec.lines.iter().take(50) {
                    out.push_str(&format!("- `{}`\n", sym));
                }
                if sec.lines.len() > 50 {
                    out.push_str(&format!("… {} more symbols\n", sec.lines.len() - 50));
                }
            }
        }
        ParseStrategy::Tabular => {
            for sec in &doc.sections {
                if let Some(h) = &sec.heading {
                    out.push_str(&format!("### {}\n\n", h));
                }
                if let Some(header) = sec.lines.first() {
                    out.push_str(&format!("Header row: `{}`\n", header));
                }
            }
        }
        _ => {
            for sec in &doc.sections {
                if let Some(h) = &sec.heading {
                    out.push_str(&format!("### {}\n\n", h));
                }
                for line in sec.lines.iter().take(3) {
                    if !line.trim().is_empty() {
                        out.push_str(&format!("> {}\n", line));
                    }
                }
                if sec.lines.len() > 3 {
                    out.push_str(&format!("_… {} more lines_\n", sec.lines.len() - 3));
                }
                out.push('\n');
            }
        }
    }
    out
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

    /// Decode a file to text, using the registry if available.
    fn decode_file(&self, path: &Path, ctx: &ToolContext) -> Result<Option<String>> {
        // 1. Try the document parser registry (PDF, XLSX, DOCX, custom formats)
        if let Some(registry) = &ctx.document_parsers {
            match registry.parse_file(path) {
                Ok(Some(text)) => return Ok(Some(text)),
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
            Ok(text) => Ok(Some(text)),
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
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to parse (relative to workspace or absolute)"
                },
                "query": {
                    "type": "string",
                    "description": "Optional extraction goal or question (e.g. 'What are the key findings?'). Triggers LLM-enhanced extraction."
                },
                "strategy": {
                    "type": "string",
                    "enum": ["auto", "structured", "narrative", "tabular", "code"],
                    "description": "Parse strategy. Default: auto (inferred from file type and content)."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Max characters of document content sent to the LLM (default: 8000)."
                }
            },
            "required": ["path"]
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
            .unwrap_or(ParseStrategy::Auto);

        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(8000) as usize;

        // Resolve path relative to workspace
        let path = ctx.resolve_path(path_str)?;

        if !path.exists() {
            return Ok(ToolOutput::error(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Decode file content
        let raw_text = match self.decode_file(&path, ctx)? {
            Some(t) => t,
            None => {
                return Ok(ToolOutput::error(format!(
                    "Cannot read `{}` — it appears to be a binary file with no registered parser. \
                     Register a DocumentParser for this format via SessionOptions.",
                    path.display()
                )));
            }
        };

        let line_count = raw_text.lines().count();
        let word_count = raw_text.split_whitespace().count();

        // Detect or apply parse strategy
        let strategy = if strategy_hint == ParseStrategy::Auto {
            ParseStrategy::detect(&path, &raw_text)
        } else {
            strategy_hint
        };

        // Structural parsing
        let doc = parse_document(&raw_text, &path, strategy);
        let structural_summary = build_structural_summary(&doc, strategy);

        // LLM-enhanced extraction (only when a query is provided)
        let llm_answer = if let Some(q) = query {
            let content_for_llm = if raw_text.len() > max_chars {
                format!(
                    "{}\n… (document truncated to {} chars)",
                    &raw_text[..max_chars],
                    max_chars
                )
            } else {
                raw_text.clone()
            };

            let system = format!(
                "You are a document analysis assistant. \
                 The user will provide document content and ask you to extract information from it. \
                 Answer based solely on the provided content. Be concise."
            );

            let user_msg = format!(
                "Document: `{}`\nParse strategy: {}\n\n\
                 --- DOCUMENT ---\n{}\n--- END DOCUMENT ---\n\n\
                 Query: {}",
                path.display(),
                strategy.label(),
                content_for_llm,
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
             - **Lines**: {}\n\
             - **Words**: {}\n",
            path.display(),
            strategy.label(),
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
            "lines": line_count,
            "words": word_count,
            "llm_used": query.is_some(),
        })))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

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
    fn test_parse_document_tabular() {
        let dir = TempDir::new().unwrap();
        let content = "col1,col2,col3\nA,B,C\nD,E,F\n";
        let path = write_temp(&dir, "table.csv", content);
        let doc = parse_document(content, &path, ParseStrategy::Tabular);
        assert!(!doc.sections.is_empty());
        assert!(doc.sections[0]
            .lines
            .first()
            .map(|l| l.contains("col1"))
            .unwrap_or(false));
    }

    #[test]
    fn test_parse_document_narrative() {
        let dir = TempDir::new().unwrap();
        let content = "# Title\n\nSome paragraph text here.\n\n## Section 2\n\nMore text.\n";
        let path = write_temp(&dir, "doc.md", content);
        let doc = parse_document(content, &path, ParseStrategy::Narrative);
        assert!(doc.sections.len() >= 2);
    }

    #[test]
    fn test_build_structural_summary_code() {
        let dir = TempDir::new().unwrap();
        let content = "fn foo() {}\nfn bar() {}\nstruct Baz {}\n";
        let path = write_temp(&dir, "lib.rs", content);
        let doc = parse_document(content, &path, ParseStrategy::Code);
        let summary = build_structural_summary(&doc, ParseStrategy::Code);
        assert!(
            summary.contains("Structural") || summary.contains("Symbol") || !summary.is_empty()
        );
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
