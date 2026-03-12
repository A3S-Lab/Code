//! Document Parser Extension Point
//!
//! Allows external users to extend agentic_search with custom document parsers
//! for formats like PDF, Excel, Word, etc., without modifying core functionality.
//!
//! # Architecture
//!
//! - **Core**: agentic_search tool (unchanged)
//! - **Extension Point**: DocumentParser trait
//! - **Default**: PlainTextParser (built-in)
//! - **Custom**: User-provided parsers (PDF, Excel, Word, etc.)
//!
//! # Example
//!
//! ```rust
//! use a3s_code_core::tools::document_parser::{DocumentParser, DocumentParserRegistry};
//!
//! // 1. Implement custom parser
//! struct PdfParser;
//!
//! impl DocumentParser for PdfParser {
//!     fn name(&self) -> &str { "pdf" }
//!     fn supported_extensions(&self) -> &[&str] { &["pdf"] }
//!     fn parse(&self, path: &Path) -> Result<String> {
//!         // Use pdf-extract crate
//!         pdf_extract::extract_text(path)
//!     }
//! }
//!
//! // 2. Register parser
//! let mut registry = DocumentParserRegistry::new();
//! registry.register(Box::new(PdfParser));
//!
//! // 3. Use in agentic_search
//! let tool = AgenticSearchTool::with_parser_registry(registry);
//! ```

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Document parser trait for extending agentic_search with custom file format support.
///
/// Implement this trait to add support for binary formats (PDF, Excel, Word, etc.)
/// without modifying the core agentic_search implementation.
pub trait DocumentParser: Send + Sync {
    /// Parser name (for logging and debugging)
    fn name(&self) -> &str;

    /// File extensions this parser supports (e.g., ["pdf", "PDF"])
    fn supported_extensions(&self) -> &[&str];

    /// Extract text content from a file
    ///
    /// # Arguments
    /// * `path` - Path to the file to parse
    ///
    /// # Returns
    /// * `Ok(String)` - Extracted text content
    /// * `Err(_)` - Parsing failed (file will be skipped)
    fn parse(&self, path: &Path) -> Result<String>;

    /// Optional: Check if this parser can handle a file before attempting to parse
    ///
    /// Default implementation checks file extension against `supported_extensions()`.
    fn can_parse(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            self.supported_extensions()
                .iter()
                .any(|&supported| supported.eq_ignore_ascii_case(ext))
        } else {
            false
        }
    }

    /// Optional: Maximum file size this parser can handle (in bytes)
    ///
    /// Default: 10MB
    fn max_file_size(&self) -> u64 {
        10 * 1024 * 1024 // 10MB
    }
}

/// Built-in parser for plain text files
pub struct PlainTextParser;

impl DocumentParser for PlainTextParser {
    fn name(&self) -> &str {
        "plain-text"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[
            // Code
            "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "cpp", "h", "hpp", "cs", "rb",
            "php", "swift", "kt", "scala", "sh", "bash", "zsh", // Config
            "toml", "yaml", "yml", "json", "ini", "conf", "cfg", "env", // Documentation
            "md", "txt", "rst", "adoc", "org", // Web
            "html", "htm", "xml", "css", "scss", "sass", "less", // Data
            "csv", "tsv", "log",
        ]
    }

    fn parse(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))
    }

    fn max_file_size(&self) -> u64 {
        1024 * 1024 // 1MB for plain text
    }
}

/// Registry for document parsers
///
/// Manages multiple parsers and dispatches to the appropriate one based on file extension.
#[derive(Clone)]
pub struct DocumentParserRegistry {
    parsers: Vec<Arc<dyn DocumentParser>>,
    extension_map: HashMap<String, Arc<dyn DocumentParser>>,
}

impl DocumentParserRegistry {
    /// Create a new registry with the default PlainTextParser
    pub fn new() -> Self {
        let mut registry = Self {
            parsers: Vec::new(),
            extension_map: HashMap::new(),
        };
        registry.register(Arc::new(PlainTextParser));
        registry
    }

    /// Create an empty registry without any parsers
    pub fn empty() -> Self {
        Self {
            parsers: Vec::new(),
            extension_map: HashMap::new(),
        }
    }

    /// Register a custom parser
    ///
    /// Parsers are checked in registration order. Later parsers can override earlier ones
    /// for the same file extension.
    pub fn register(&mut self, parser: Arc<dyn DocumentParser>) {
        for ext in parser.supported_extensions() {
            self.extension_map
                .insert(ext.to_lowercase(), Arc::clone(&parser));
        }
        self.parsers.push(parser);
    }

    /// Find a parser for the given file path
    ///
    /// Returns the first parser that can handle the file, or None if no parser is found.
    pub fn find_parser(&self, path: &Path) -> Option<Arc<dyn DocumentParser>> {
        // Try extension-based lookup first (fast path)
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if let Some(parser) = self.extension_map.get(&ext.to_lowercase()) {
                return Some(Arc::clone(parser));
            }
        }

        // Fallback: check each parser's can_parse method
        for parser in &self.parsers {
            if parser.can_parse(path) {
                return Some(Arc::clone(parser));
            }
        }

        None
    }

    /// Parse a file using the appropriate parser
    ///
    /// # Returns
    /// * `Ok(Some(String))` - Successfully parsed
    /// * `Ok(None)` - No parser available for this file type
    /// * `Err(_)` - Parsing failed
    pub fn parse_file(&self, path: &Path) -> Result<Option<String>> {
        if let Some(parser) = self.find_parser(path) {
            // Check file size
            if let Ok(metadata) = std::fs::metadata(path) {
                if metadata.len() > parser.max_file_size() {
                    return Ok(None); // Skip files that are too large
                }
            }

            match parser.parse(path) {
                Ok(content) => Ok(Some(content)),
                Err(e) => {
                    // Log error but don't fail the entire search
                    eprintln!(
                        "Warning: Failed to parse {} with {}: {}",
                        path.display(),
                        parser.name(),
                        e
                    );
                    Ok(None)
                }
            }
        } else {
            Ok(None) // No parser available
        }
    }

    /// Get all registered parsers
    pub fn parsers(&self) -> &[Arc<dyn DocumentParser>] {
        &self.parsers
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

    #[test]
    fn test_plain_text_parser() {
        let parser = PlainTextParser;
        assert_eq!(parser.name(), "plain-text");
        assert!(parser.supported_extensions().contains(&"rs"));
        assert!(parser.supported_extensions().contains(&"md"));
    }

    #[test]
    fn test_registry_default() {
        let registry = DocumentParserRegistry::new();
        assert_eq!(registry.parsers().len(), 1); // PlainTextParser
    }

    #[test]
    fn test_registry_find_parser() {
        let registry = DocumentParserRegistry::new();

        // Should find parser for .rs file
        let rs_path = Path::new("test.rs");
        assert!(registry.find_parser(rs_path).is_some());

        // Should not find parser for .pdf file (no PDF parser registered)
        let pdf_path = Path::new("test.pdf");
        assert!(registry.find_parser(pdf_path).is_none());
    }

    #[test]
    fn test_parse_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "Hello, world!").unwrap();

        let registry = DocumentParserRegistry::new();
        let result = registry.parse_file(&file_path).unwrap();

        assert!(result.is_some());
        assert!(result.unwrap().contains("Hello, world!"));
    }

    #[test]
    fn test_custom_parser() {
        struct MockParser;

        impl DocumentParser for MockParser {
            fn name(&self) -> &str {
                "mock"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["mock"]
            }
            fn parse(&self, _path: &Path) -> Result<String> {
                Ok("mocked content".to_string())
            }
        }

        let mut registry = DocumentParserRegistry::empty();
        registry.register(Arc::new(MockParser));

        let mock_path = Path::new("test.mock");
        let parser = registry.find_parser(mock_path);
        assert!(parser.is_some());
        assert_eq!(parser.unwrap().name(), "mock");
    }

    #[test]
    fn test_parser_override() {
        struct Parser1;
        impl DocumentParser for Parser1 {
            fn name(&self) -> &str {
                "parser1"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["txt"]
            }
            fn parse(&self, _path: &Path) -> Result<String> {
                Ok("parser1".to_string())
            }
        }

        struct Parser2;
        impl DocumentParser for Parser2 {
            fn name(&self) -> &str {
                "parser2"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["txt"]
            }
            fn parse(&self, _path: &Path) -> Result<String> {
                Ok("parser2".to_string())
            }
        }

        let mut registry = DocumentParserRegistry::empty();
        registry.register(Arc::new(Parser1));
        registry.register(Arc::new(Parser2));

        // Parser2 should override Parser1 for .txt files
        let txt_path = Path::new("test.txt");
        let parser = registry.find_parser(txt_path).unwrap();
        assert_eq!(parser.name(), "parser2");
    }
}
