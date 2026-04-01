//! Document parser types used by A3S Code's context acquisition pipeline.
//!
//! These types exist so `agentic_search`, `agentic_parse`, and session wiring
//! can register a small set of document parsers when better context
//! extraction is needed.
//!
//! They are not intended to turn `a3s-code-core` into a general-purpose
//! document processing framework.
//!
//! # Architecture
//!
//! - **Contracts**: parser trait and registry live in `crate::doc`
//! - **Core defaults**: `PlainTextParser` plus the internal composite parser factory live here
//! - **Built-in tools**: `agentic_search` and `agentic_parse` consume this registry via `ToolContext`
//! - **Goal**: recover better model context from non-plaintext project files
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
//!         todo!()
//!     }
//! }
//!
//! let mut registry = DocumentParserRegistry::empty();
//! registry.register(std::sync::Arc::new(PdfParser));
//! ```

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

pub use crate::doc::{
    DocumentBlock, DocumentBlockKind, DocumentBlockLocation, DocumentConfidence, DocumentMetadata,
    DocumentParser, DocumentParserRegistry, DocumentProvenance, ParsedDocument,
};

/// Built-in parser for all common text, code, and config formats.
///
/// Handles UTF-8 files up to 1 MiB. Binary or oversized files are skipped.
pub struct PlainTextParser;

impl DocumentParser for PlainTextParser {
    fn name(&self) -> &str {
        "plain-text"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[
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
            "md",
            "mdx",
            "txt",
            "rst",
            "adoc",
            "org",
            "tex",
            "latex",
            "typ",
            "typst",
            "html",
            "htm",
            "css",
            "scss",
            "sass",
            "less",
            "csv",
            "tsv",
            "log",
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

    fn parse_extracted(&self, path: &Path) -> Result<crate::document_pipeline::ExtractedDocument> {
        Ok(crate::document_pipeline::ExtractedDocument::new(
            crate::document_parser::ParsedDocument::from_text(self.parse(path)?),
        ))
    }

    fn max_file_size(&self) -> u64 {
        1024 * 1024
    }
}

/// Build the default document parser registry using the default parser config.
pub fn default_document_parser_registry() -> DocumentParserRegistry {
    crate::document_registry_factory::build_document_parser_registry(
        crate::config::DocumentParserConfig::default(),
        None,
    )
}

/// Build the default document parser registry using an explicit parser config.
pub fn document_parser_registry_with_config(
    config: crate::config::DocumentParserConfig,
) -> DocumentParserRegistry {
    crate::document_registry_factory::build_document_parser_registry(config, None)
}

/// Build the default document parser registry using an explicit parser config
/// and OCR provider.
pub fn document_parser_registry_with_config_and_ocr(
    config: crate::config::DocumentParserConfig,
    ocr_provider: Arc<dyn crate::document_ocr::DocumentOcrProvider>,
) -> DocumentParserRegistry {
    crate::document_registry_factory::build_document_parser_registry(config, Some(ocr_provider))
}

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

    fn build_registry() -> DocumentParserRegistry {
        crate::document_registry_factory::build_document_parser_registry(
            crate::config::DocumentParserConfig::default(),
            None,
        )
    }

    #[test]
    fn plain_text_parser_basic() {
        let parser = PlainTextParser;
        assert_eq!(parser.name(), "plain-text");
        assert!(parser.supported_extensions().contains(&"rs"));
        assert!(parser.supported_extensions().contains(&"md"));
        assert!(parser.supported_extensions().contains(&"tex"));
        assert!(parser.supported_extensions().contains(&"typst"));
        assert!(parser.supported_extensions().contains(&"json"));
    }

    #[test]
    fn registry_default_has_plain_text() {
        let r = build_registry();
        assert!(r.len() >= 2);
        assert!(r.find_parser(Path::new("main.rs")).is_some());
    }

    #[test]
    fn registry_finds_parser_by_extension() {
        let r = build_registry();
        assert!(r.find_parser(Path::new("main.rs")).is_some());
        assert!(r.find_parser(Path::new("config.toml")).is_some());
        assert!(r.find_parser(Path::new("README.md")).is_some());
    }

    #[test]
    fn registry_no_parser_for_binary() {
        let r = build_registry();
        assert!(r.find_parser(Path::new("binary.exe")).is_none());
        assert!(r.find_parser(Path::new("document.pdf")).is_some());
    }

    #[test]
    fn parse_file_reads_text() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "hello.rs", "fn main() {}");

        let r = build_registry();
        let result = r.parse_file(&path).unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().contains("fn main"));
    }

    #[test]
    fn parse_file_extracted_returns_structured_output() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "hello.rs", "fn main() {}");

        let r = build_registry();
        let result = r.parse_file_extracted(&path).unwrap();
        assert!(result.is_some());
        assert!(result
            .unwrap()
            .into_parsed_document()
            .to_text()
            .contains("fn main"));
    }

    #[test]
    fn parsed_document_stats_helpers() {
        let document = ParsedDocument {
            title: Some("hello".to_string()),
            blocks: vec![
                DocumentBlock::new(DocumentBlockKind::Paragraph, Some("intro"), "hello world"),
                DocumentBlock::new(DocumentBlockKind::Raw, None::<String>, "   "),
            ],
            metadata: None,
            ..Default::default()
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

        let r = build_registry();
        assert!(r.parse_file(&path).unwrap().is_none());
    }
}
