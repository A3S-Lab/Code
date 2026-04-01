use crate::config::DocumentParserConfig;
use crate::document_ocr::DocumentOcrProvider;
use crate::document_parser::DocumentParserRegistry;
use std::sync::Arc;

pub(crate) fn build_document_parser_registry(
    config: DocumentParserConfig,
    ocr_provider: Option<Arc<dyn DocumentOcrProvider>>,
) -> DocumentParserRegistry {
    let mut registry = DocumentParserRegistry::empty();
    crate::document_parser_defaults::configure_default_document_parsers(
        &mut registry,
        config,
        ocr_provider,
    );
    registry
}

pub(crate) fn resolve_document_parser_registry(
    explicit_registry: Option<Arc<DocumentParserRegistry>>,
    config: DocumentParserConfig,
    ocr_provider: Option<Arc<dyn DocumentOcrProvider>>,
) -> Arc<DocumentParserRegistry> {
    explicit_registry
        .unwrap_or_else(|| Arc::new(build_document_parser_registry(config, ocr_provider)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_document_parser_registry_includes_plain_text_when_document_parser_disabled() {
        let registry = build_document_parser_registry(
            DocumentParserConfig {
                enabled: false,
                ..DocumentParserConfig::default()
            },
            None,
        );

        let txt = std::path::Path::new("notes.txt");
        let pdf = std::path::Path::new("report.pdf");
        assert!(registry.find_parser(txt).is_some());
        assert!(registry.find_parser(pdf).is_none());
    }

    #[test]
    fn resolve_document_parser_registry_prefers_explicit_registry() {
        let explicit = Arc::new(DocumentParserRegistry::empty());
        let resolved = resolve_document_parser_registry(
            Some(Arc::clone(&explicit)),
            DocumentParserConfig::default(),
            None,
        );

        assert!(Arc::ptr_eq(&explicit, &resolved));
    }
}
