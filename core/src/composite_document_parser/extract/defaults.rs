use anyhow::Result;
use std::path::Path;

use crate::document_parser::ParsedDocument;

use super::detect::probe_document;
use super::extractors::{
    CompositeDocumentExtractor, EmailExtractor, ExtractorContext, HtmlXmlExtractor,
    ImageOcrExtractor, OdfExtractor, OfficeExtractor, PdfExtractor, StructuredDataExtractor,
};
use super::DocumentOcrProvider;

pub(super) fn parse_document_with_default_extractors(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    if let Some(document) = super::extract_archive::parse_archive_document(path)? {
        return Ok(document);
    }

    let probe = probe_document(path)?;
    let ctx = ExtractorContext {
        path,
        ext: &probe.original_ext,
        detected_ext: &probe.detected_ext,
        config,
        ocr_provider,
    };

    for extractor in default_extractors() {
        if extractor.can_extract(&ctx) {
            let mut document = extractor.extract(&ctx)?;
            super::enrich_document_metadata(
                &mut document,
                path,
                Some(ctx.detected_ext),
                Some(extractor.name()),
            );
            return Ok(document);
        }
    }

    anyhow::bail!("unsupported extension for composite document parser")
}

fn default_extractors() -> [&'static dyn CompositeDocumentExtractor; 7] {
    [
        &PdfExtractor,
        &OfficeExtractor,
        &OdfExtractor,
        &EmailExtractor,
        &StructuredDataExtractor,
        &HtmlXmlExtractor,
        &ImageOcrExtractor,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_extractors_expose_expected_order() {
        let names = default_extractors()
            .iter()
            .map(|extractor| extractor.name())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "pdf",
                "office",
                "odf",
                "email",
                "structured-data",
                "html-xml",
                "image-ocr",
            ]
        );
    }
}
