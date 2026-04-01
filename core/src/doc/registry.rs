use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::{DocumentExtractionMetadata, DocumentParser, ExtractedDocument};

#[derive(Clone)]
pub struct DocumentParserRegistry {
    parsers: Vec<Arc<dyn DocumentParser>>,
    extension_map: HashMap<String, Arc<dyn DocumentParser>>,
}

impl DocumentParserRegistry {
    pub fn empty() -> Self {
        Self {
            parsers: Vec::new(),
            extension_map: HashMap::new(),
        }
    }

    pub fn register(&mut self, parser: Arc<dyn DocumentParser>) {
        for ext in parser.supported_extensions() {
            self.extension_map
                .insert(ext.to_lowercase(), Arc::clone(&parser));
        }
        self.parsers.push(parser);
    }

    pub fn find_parser(&self, path: &Path) -> Option<Arc<dyn DocumentParser>> {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if let Some(parser) = self.extension_map.get(&ext.to_lowercase()) {
                return Some(Arc::clone(parser));
            }
        }

        self.parsers
            .iter()
            .find(|parser| parser.can_parse(path))
            .cloned()
    }

    pub fn parse_file_extracted(&self, path: &Path) -> Result<Option<ExtractedDocument>> {
        let parser = match self.find_parser(path) {
            Some(parser) => parser,
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

        match parser.parse_extracted(path) {
            Ok(document) => Ok(Some(annotate_extracted_document(document, parser.as_ref()))),
            Err(error) => {
                tracing::warn!(
                    "Parser '{}' failed on {}: {}",
                    parser.name(),
                    path.display(),
                    error
                );
                Ok(None)
            }
        }
    }

    pub fn parse_file(&self, path: &Path) -> Result<Option<String>> {
        Ok(self
            .parse_file_extracted(path)?
            .map(ExtractedDocument::into_parsed_document)
            .map(|document| document.to_text()))
    }

    pub fn parsers(&self) -> &[Arc<dyn DocumentParser>] {
        &self.parsers
    }

    pub fn len(&self) -> usize {
        self.parsers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parsers.is_empty()
    }
}

impl Default for DocumentParserRegistry {
    fn default() -> Self {
        Self::empty()
    }
}

fn annotate_extracted_document(
    mut extracted: ExtractedDocument,
    parser: &dyn DocumentParser,
) -> ExtractedDocument {
    let metadata = extracted
        .extraction_metadata
        .get_or_insert_with(DocumentExtractionMetadata::default);
    if metadata.parser_name.is_none() {
        metadata.parser_name = Some(parser.name().to_string());
    }
    if metadata.parser_signature.is_none() {
        metadata.parser_signature = Some(parser.signature());
    }
    if metadata.extractor.is_none() {
        metadata.extractor = extracted
            .document
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.provenance.as_ref())
            .and_then(|provenance| provenance.extractor.clone());
    }
    if metadata.detected_file_type.is_none() {
        metadata.detected_file_type = extracted
            .document
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.detected_file_type.clone());
    }
    extracted
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

    #[test]
    fn registry_empty_has_no_parsers() {
        let r = DocumentParserRegistry::empty();
        assert!(r.is_empty());
        assert!(r.find_parser(Path::new("main.rs")).is_none());
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
    fn parse_file_extracted_returns_structured_output() {
        struct TextParser;
        impl DocumentParser for TextParser {
            fn name(&self) -> &str {
                "text"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["rs"]
            }
            fn parse(&self, path: &Path) -> Result<String> {
                Ok(std::fs::read_to_string(path)?)
            }
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "hello.rs", "fn main() {}");

        let mut r = DocumentParserRegistry::empty();
        r.register(Arc::new(TextParser));
        let result = r.parse_file_extracted(&path).unwrap();
        assert!(result.is_some());
        assert!(result
            .unwrap()
            .into_parsed_document()
            .to_text()
            .contains("fn main"));
    }

    #[test]
    fn parse_file_extracted_annotates_parser_metadata() {
        struct TextParser;
        impl DocumentParser for TextParser {
            fn name(&self) -> &str {
                "text"
            }
            fn signature(&self) -> String {
                "text@v1".to_string()
            }
            fn supported_extensions(&self) -> &[&str] {
                &["rs"]
            }
            fn parse(&self, path: &Path) -> Result<String> {
                Ok(std::fs::read_to_string(path)?)
            }
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "hello.rs", "fn main() {}");

        let mut r = DocumentParserRegistry::empty();
        r.register(Arc::new(TextParser));
        let result = r.parse_file_extracted(&path).unwrap().unwrap();

        assert_eq!(
            result
                .extraction_metadata
                .as_ref()
                .and_then(|metadata| metadata.parser_name.as_deref()),
            Some("text")
        );
        assert_eq!(
            result
                .extraction_metadata
                .as_ref()
                .and_then(|metadata| metadata.parser_signature.as_deref()),
            Some("text@v1")
        );
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
                Ok(std::fs::read_to_string(path)?)
            }
            fn max_file_size(&self) -> u64 {
                3
            }
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "big.dat", "more than 3 bytes");

        let mut r = DocumentParserRegistry::empty();
        r.register(Arc::new(TinyMaxParser));

        assert!(r.parse_file(&path).unwrap().is_none());
    }
}
