//! Document Parser Extension Point
//!
//! `DocumentParser` is a core extension point that allows users to extend
//! agentic tools (agentic_search, agentic_parse, etc.) with custom file format
//! support for binary and structured formats such as PDF, Excel, Word, etc.
//!
//! # Architecture
//!
//! - **Core**: `DocumentParser` trait + `DocumentParserRegistry` live here
//! - **Default**: `PlainTextParser` covers all common text/code formats
//! - **Built-in tools**: agentic-search and agentic-parse use this registry via `ToolContext`
//! - **Custom**: Users register additional parsers via `SessionOptions`
//!
//! # Example
//!
//! ```rust,no_run
//! use a3s_code_core::document_parser::{DocumentParser, DocumentParserRegistry};
//! use std::path::Path;
//! use anyhow::Result;
//!
//! struct PdfParser;
//!
//! impl DocumentParser for PdfParser {
//!     fn name(&self) -> &str { "pdf" }
//!     fn supported_extensions(&self) -> &[&str] { &["pdf"] }
//!     fn parse(&self, path: &Path) -> Result<String> {
//!         // e.g. pdf_extract::extract_text(path)
//!         todo!()
//!     }
//! }
//!
//! let mut registry = DocumentParserRegistry::new();
//! registry.register(std::sync::Arc::new(PdfParser));
//! ```

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// ============================================================================
// Structured document model
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentBlockKind {
    Paragraph,
    Heading,
    Table,
    Section,
    Metadata,
    Slide,
    EmailHeader,
    Code,
    Raw,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentBlockLocation {
    pub source: Option<String>,
    pub page: Option<usize>,
    pub ordinal: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentBlock {
    pub kind: DocumentBlockKind,
    pub label: Option<String>,
    pub content: String,
    pub location: Option<DocumentBlockLocation>,
}

impl DocumentBlock {
    pub fn new(
        kind: DocumentBlockKind,
        label: Option<impl Into<String>>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            label: label.map(Into::into),
            content: content.into(),
            location: None,
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .source = Some(source.into());
        self
    }

    pub fn with_page(mut self, page: usize) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .page = Some(page);
        self
    }

    pub fn with_ordinal(mut self, ordinal: usize) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .ordinal = Some(ordinal);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDocument {
    pub title: Option<String>,
    pub blocks: Vec<DocumentBlock>,
}

impl ParsedDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            title: None,
            blocks: vec![DocumentBlock::new(
                DocumentBlockKind::Raw,
                None::<String>,
                text,
            )],
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn push(&mut self, block: DocumentBlock) {
        self.blocks.push(block);
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn non_empty_block_count(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| !block.content.trim().is_empty())
            .count()
    }

    pub fn char_count(&self) -> usize {
        self.to_text().chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|b| b.content.trim().is_empty())
    }

    pub fn to_text(&self) -> String {
        let mut parts = Vec::new();
        if let Some(title) = &self.title {
            if !title.trim().is_empty() {
                parts.push(title.trim().to_string());
            }
        }
        for block in &self.blocks {
            let mut chunk = String::new();
            if let Some(label) = &block.label {
                if !label.trim().is_empty() {
                    chunk.push_str(label.trim());
                    chunk.push('\n');
                }
            }
            chunk.push_str(block.content.trim());
            if !chunk.trim().is_empty() {
                parts.push(chunk.trim().to_string());
            }
        }
        parts.join("\n\n")
    }
}

// ============================================================================
// DocumentParser trait
// ============================================================================

/// Extension point for custom file format parsing.
///
/// Implement this trait to add support for formats that cannot be read as plain
/// text (PDF, Excel, Word, images with OCR, etc.) without modifying any core
/// tool logic.
pub trait DocumentParser: Send + Sync {
    /// Unique parser identifier (used for logging and debugging).
    fn name(&self) -> &str;

    /// File extensions this parser handles (case-insensitive, no leading dot).
    ///
    /// Example: `&["pdf", "PDF"]`
    fn supported_extensions(&self) -> &[&str];

    /// Extract plain-text content from `path`.
    ///
    /// Return `Err` if the file cannot be read or parsed; the registry will
    /// log a warning and skip the file rather than propagating the error.
    fn parse(&self, path: &Path) -> Result<String>;

    /// Extract a structured document from `path`.
    ///
    /// The default implementation wraps [`DocumentParser::parse`] into a
    /// single raw-text block so existing parsers remain source-compatible.
    fn parse_document(&self, path: &Path) -> Result<ParsedDocument> {
        Ok(ParsedDocument::from_text(self.parse(path)?))
    }

    /// Override to control whether this parser will attempt a file before the
    /// extension lookup.  The default checks extension against
    /// `supported_extensions()`.
    fn can_parse(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                self.supported_extensions()
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(ext))
            })
            .unwrap_or(false)
    }

    /// Maximum file size (bytes) this parser accepts.  Files larger than this
    /// limit are silently skipped.  Default: 10 MiB.
    fn max_file_size(&self) -> u64 {
        10 * 1024 * 1024
    }
}

// ============================================================================
// PlainTextParser — built-in default
// ============================================================================

/// Built-in parser for all common text, code, and config formats.
///
/// Handles UTF-8 files up to 1 MiB.  Binary or oversized files are skipped.
pub struct PlainTextParser;

impl DocumentParser for PlainTextParser {
    fn name(&self) -> &str {
        "plain-text"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[
            // Source code
            "rs",
            "py",
            "ts",
            "tsx",
            "js",
            "jsx",
            "go",
            "java",
            "c",
            "cpp",
            "h",
            "hpp",
            "cs",
            "rb",
            "php",
            "swift",
            "kt",
            "scala",
            "sh",
            "bash",
            "zsh",
            "fish",
            // Config / data
            "toml",
            "yaml",
            "yml",
            "json",
            "jsonc",
            "ini",
            "conf",
            "cfg",
            "env",
            "xml",
            // Documentation
            "md",
            "mdx",
            "txt",
            "rst",
            "adoc",
            "org",
            // Web
            "html",
            "htm",
            "css",
            "scss",
            "sass",
            "less",
            // Data
            "csv",
            "tsv",
            "log",
            // Build
            "makefile",
            "dockerfile",
            "gradlew",
        ]
    }

    fn parse(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!(
                "plain-text parser: failed to read {}: {}",
                path.display(),
                e
            )
        })
    }

    fn max_file_size(&self) -> u64 {
        1024 * 1024 // 1 MiB
    }
}

// ============================================================================
// DocumentParserRegistry
// ============================================================================

/// Registry that maps file extensions to `DocumentParser` implementations.
///
/// - Created with `new()`: includes `PlainTextParser` for all common formats.
/// - Created with `empty()`: no parsers at all (useful for overriding defaults).
/// - Later registrations override earlier ones for the same extension.
#[derive(Clone)]
pub struct DocumentParserRegistry {
    /// Registration order is preserved for fallback `can_parse` checks.
    parsers: Vec<Arc<dyn DocumentParser>>,
    /// Fast O(1) lookup by lowercase extension.
    extension_map: HashMap<String, Arc<dyn DocumentParser>>,
}

impl DocumentParserRegistry {
    /// Create a registry pre-loaded with `PlainTextParser`.
    pub fn new() -> Self {
        Self::new_with_default_parser(crate::config::DefaultParserConfig::default(), None)
    }

    /// Create a registry pre-loaded with `PlainTextParser` and a configured
    /// `DefaultParser`.
    pub fn new_with_default_parser_config(config: crate::config::DefaultParserConfig) -> Self {
        Self::new_with_default_parser(config, None)
    }

    /// Create a registry pre-loaded with `PlainTextParser` and a configured
    /// `DefaultParser`, optionally wiring an OCR provider into that parser.
    pub fn new_with_default_parser(
        config: crate::config::DefaultParserConfig,
        ocr_provider: Option<Arc<dyn crate::default_parser::DefaultParserOcrProvider>>,
    ) -> Self {
        let mut r = Self::empty();
        r.register(Arc::new(PlainTextParser));
        if config.enabled {
            let parser = match ocr_provider {
                Some(provider) => {
                    crate::default_parser::DefaultParser::with_config_and_ocr(config, provider)
                }
                None => crate::default_parser::DefaultParser::with_config(config),
            };
            r.register(Arc::new(parser));
        }
        r
    }

    /// Create an empty registry (no parsers registered).
    pub fn empty() -> Self {
        Self {
            parsers: Vec::new(),
            extension_map: HashMap::new(),
        }
    }

    /// Register a parser.  Later registrations win for the same extension.
    pub fn register(&mut self, parser: Arc<dyn DocumentParser>) {
        for ext in parser.supported_extensions() {
            self.extension_map
                .insert(ext.to_lowercase(), Arc::clone(&parser));
        }
        self.parsers.push(parser);
    }

    /// Find the parser responsible for `path` (extension lookup, then linear
    /// `can_parse` scan).
    pub fn find_parser(&self, path: &Path) -> Option<Arc<dyn DocumentParser>> {
        // Fast path: extension map
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if let Some(p) = self.extension_map.get(&ext.to_lowercase()) {
                return Some(Arc::clone(p));
            }
        }
        // Slow path: can_parse scan (for parsers without extensions, e.g. Dockerfile)
        self.parsers.iter().find(|p| p.can_parse(path)).cloned()
    }

    /// Parse `path` and return a structured document.
    ///
    /// Returns:
    /// - `Ok(Some(document))` — parsed successfully
    /// - `Ok(None)` — no parser available, or file too large
    /// - `Err(_)` — I/O or metadata failure (not a parse failure)
    pub fn parse_file_document(&self, path: &Path) -> Result<Option<ParsedDocument>> {
        let parser = match self.find_parser(path) {
            Some(p) => p,
            None => return Ok(None),
        };

        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > parser.max_file_size() {
                tracing::debug!(
                    "Skipping {} ({}): exceeds parser '{}' limit of {} bytes",
                    path.display(),
                    meta.len(),
                    parser.name(),
                    parser.max_file_size()
                );
                return Ok(None);
            }
        }

        match parser.parse_document(path) {
            Ok(document) => Ok(Some(document)),
            Err(e) => {
                tracing::warn!(
                    "Parser '{}' failed on {}: {}",
                    parser.name(),
                    path.display(),
                    e
                );
                Ok(None)
            }
        }
    }

    /// Parse `path` and return extracted text for compatibility with older
    /// call sites.
    pub fn parse_file(&self, path: &Path) -> Result<Option<String>> {
        Ok(self
            .parse_file_document(path)?
            .map(|document| document.to_text()))
    }

    /// All registered parsers (in registration order).
    pub fn parsers(&self) -> &[Arc<dyn DocumentParser>] {
        &self.parsers
    }

    /// Number of registered parsers.
    pub fn len(&self) -> usize {
        self.parsers.len()
    }

    /// Returns `true` if no parsers are registered.
    pub fn is_empty(&self) -> bool {
        self.parsers.is_empty()
    }
}

impl Default for DocumentParserRegistry {
    fn default() -> Self {
        Self::new()
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
    fn plain_text_parser_basic() {
        let parser = PlainTextParser;
        assert_eq!(parser.name(), "plain-text");
        assert!(parser.supported_extensions().contains(&"rs"));
        assert!(parser.supported_extensions().contains(&"md"));
        assert!(parser.supported_extensions().contains(&"json"));
    }

    #[test]
    fn registry_default_has_plain_text() {
        let r = DocumentParserRegistry::new();
        assert!(r.len() >= 2);
        assert!(r.find_parser(Path::new("main.rs")).is_some());
    }

    #[test]
    fn registry_empty_has_no_parsers() {
        let r = DocumentParserRegistry::empty();
        assert!(r.is_empty());
        assert!(r.find_parser(Path::new("main.rs")).is_none());
    }

    #[test]
    fn registry_finds_parser_by_extension() {
        let r = DocumentParserRegistry::new();
        assert!(r.find_parser(Path::new("main.rs")).is_some());
        assert!(r.find_parser(Path::new("config.toml")).is_some());
        assert!(r.find_parser(Path::new("README.md")).is_some());
    }

    #[test]
    fn registry_no_parser_for_binary() {
        let r = DocumentParserRegistry::new();
        assert!(r.find_parser(Path::new("binary.exe")).is_none());
        assert!(r.find_parser(Path::new("document.pdf")).is_some());
    }

    #[test]
    fn registry_later_registration_wins() {
        struct ParserA;
        impl DocumentParser for ParserA {
            fn name(&self) -> &str {
                "a"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["txt"]
            }
            fn parse(&self, _: &Path) -> Result<String> {
                Ok("A".into())
            }
        }

        struct ParserB;
        impl DocumentParser for ParserB {
            fn name(&self) -> &str {
                "b"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["txt"]
            }
            fn parse(&self, _: &Path) -> Result<String> {
                Ok("B".into())
            }
        }

        let mut r = DocumentParserRegistry::empty();
        r.register(Arc::new(ParserA));
        r.register(Arc::new(ParserB));

        let p = r.find_parser(Path::new("file.txt")).unwrap();
        assert_eq!(p.name(), "b");
    }

    #[test]
    fn parse_file_reads_text() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "hello.rs", "fn main() {}");

        let r = DocumentParserRegistry::new();
        let result = r.parse_file(&path).unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().contains("fn main"));
    }

    #[test]
    fn parse_file_document_returns_structured_output() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "hello.rs", "fn main() {}");

        let r = DocumentParserRegistry::new();
        let result = r.parse_file_document(&path).unwrap();
        assert!(result.is_some());
        let document = result.unwrap();
        assert!(!document.blocks.is_empty());
        assert!(document.to_text().contains("fn main"));
    }

    #[test]
    fn parsed_document_stats_helpers() {
        let document = ParsedDocument {
            title: Some("hello".to_string()),
            blocks: vec![
                DocumentBlock::new(DocumentBlockKind::Paragraph, Some("intro"), "hello world"),
                DocumentBlock::new(DocumentBlockKind::Raw, None::<String>, "   "),
            ],
        };

        assert_eq!(document.block_count(), 2);
        assert_eq!(document.non_empty_block_count(), 1);
        assert!(document.char_count() >= "hello".len());
    }

    #[test]
    fn document_block_location_builders() {
        let block = DocumentBlock::new(DocumentBlockKind::Paragraph, Some("intro"), "hello")
            .with_source("chapter1")
            .with_page(3)
            .with_ordinal(7);

        let location = block.location.expect("location should exist");
        assert_eq!(location.source.as_deref(), Some("chapter1"));
        assert_eq!(location.page, Some(3));
        assert_eq!(location.ordinal, Some(7));
    }

    #[test]
    fn parse_file_returns_none_for_unknown_extension() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "file.xyz", "data");

        let r = DocumentParserRegistry::new();
        assert!(r.parse_file(&path).unwrap().is_none());
    }

    #[test]
    fn parse_file_skips_oversized_file() {
        struct TinyMaxParser;
        impl DocumentParser for TinyMaxParser {
            fn name(&self) -> &str {
                "tiny"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["dat"]
            }
            fn parse(&self, path: &Path) -> Result<String> {
                std::fs::read_to_string(path).map_err(Into::into)
            }
            fn max_file_size(&self) -> u64 {
                3
            } // 3 bytes max
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "big.dat", "more than 3 bytes");

        let mut r = DocumentParserRegistry::empty();
        r.register(Arc::new(TinyMaxParser));

        assert!(r.parse_file(&path).unwrap().is_none());
    }
}
