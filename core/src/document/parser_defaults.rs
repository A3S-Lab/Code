use crate::composite_document_parser::CompositeDocumentParser;
use crate::config::DocumentParserConfig;
use crate::document_ocr::DocumentOcrProvider;
use crate::document_parser::{DocumentParser, DocumentParserRegistry, PlainTextParser};
use std::sync::Arc;

pub(crate) fn configure_default_document_parsers(
    registry: &mut DocumentParserRegistry,
    config: DocumentParserConfig,
    ocr_provider: Option<Arc<dyn DocumentOcrProvider>>,
) {
    registry.register(Arc::new(PlainTextParser));
    if config.enabled {
        registry.register(build_composite_document_parser(config, ocr_provider));
    }
}

pub(crate) fn build_composite_document_parser(
    config: DocumentParserConfig,
    ocr_provider: Option<Arc<dyn DocumentOcrProvider>>,
) -> Arc<dyn DocumentParser> {
    Arc::new(build_composite_parser(config, ocr_provider))
}

pub(crate) fn build_composite_parser(
    config: DocumentParserConfig,
    ocr_provider: Option<Arc<dyn DocumentOcrProvider>>,
) -> CompositeDocumentParser {
    match ocr_provider {
        Some(provider) => CompositeDocumentParser::with_config_and_ocr(config, provider),
        None => CompositeDocumentParser::with_config(config),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_default_document_parsers_always_registers_plain_text() {
        let mut registry = DocumentParserRegistry::empty();
        configure_default_document_parsers(
            &mut registry,
            DocumentParserConfig {
                enabled: false,
                ..DocumentParserConfig::default()
            },
            None,
        );

        assert!(registry
            .find_parser(std::path::Path::new("notes.txt"))
            .is_some());
        assert!(registry
            .find_parser(std::path::Path::new("report.pdf"))
            .is_none());
    }

    #[test]
    fn build_composite_document_parser_returns_trait_object() {
        let parser = build_composite_document_parser(DocumentParserConfig::default(), None);
        assert_eq!(parser.name(), "composite-document-parser");
        assert!(parser.can_parse(std::path::Path::new("report.pdf")));
    }
}
