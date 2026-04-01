#[path = "format/citation.rs"]
mod citation;
mod common;
#[path = "format/container.rs"]
mod container;
#[path = "extract/defaults.rs"]
mod defaults;
#[path = "probe/detect.rs"]
mod detect;
#[path = "mail/email.rs"]
mod email;
#[path = "extract/archive.rs"]
mod extract_archive;
#[path = "extract/structured.rs"]
mod extract_structured;
#[path = "extract/extractors.rs"]
mod extractors;
#[path = "probe/formats.rs"]
mod formats;
#[path = "markup/mod.rs"]
mod markup;
#[path = "markup/facade.rs"]
mod markup_facade;
#[path = "mail/msg.rs"]
mod msg;
#[path = "normalize/paged.rs"]
mod normalize_paged;
#[path = "ocr/mod.rs"]
mod ocr;
#[path = "format/office.rs"]
mod office;
#[path = "format/structured.rs"]
mod structured;
#[path = "extract/text.rs"]
mod text;

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
use crate::document_parser::ParsedDocument;
use citation::{
    parse_bib, parse_bib_string, parse_csl, parse_csl_string, parse_enw, parse_enw_string,
    parse_nbib, parse_nbib_string, parse_ris, parse_ris_string, tagged_fields_to_text,
};
use common::{
    attribute_by_local_name, enrich_document_metadata, ensure_document, extract_xml_text,
    file_title, looks_like_heading, normalize_text, table_structured_payload,
    table_text_from_cells,
};
use container::{
    open_zip, parse_7z, parse_epub, parse_gzip, parse_iwork_package, parse_tar, parse_tgz,
    parse_zip, read_docx_core_metadata, read_odf_metadata, read_zip_entry, read_zip_entry_bytes,
};
use email::{parse_eml, parse_emlx, parse_mbox};
use formats::{
    is_docx_family, is_flat_odf_family, is_html_family, is_ics_family, is_image_family,
    is_legacy_doc_family, is_legacy_ppt_family, is_legacy_xls_family, is_odf_family,
    is_plain_text_family, is_pptx_family, is_vcard_family, is_xlsx_family, is_xml_family,
    SUPPORTED_EXTENSIONS,
};
use markup_facade::{
    collect_node_text, extract_markup_title, parse_html_document, parse_markup_string, parse_rtf,
    parse_xml_document, render_html_to_text, strip_rtf,
};
use msg::parse_msg;
use normalize_paged::{label_paged_block, normalize_paged_text_pages};
pub use ocr::{
    extract_document_runtime_metadata, DocumentOcrCapabilities, DocumentOcrFormat,
    DocumentOcrOutput, DocumentOcrPageResult, DocumentOcrProvider, DocumentOcrRequest,
    DocumentOcrRuntimeInfo, DocumentRuntimeMetadata,
};
use ocr::{maybe_run_document_ocr_fallback, parse_image_document, parse_pdf_document};
use office::{
    parse_archive_office_entry, parse_docx, parse_hwp, parse_hwpx, parse_legacy_doc,
    parse_legacy_ppt, parse_legacy_xls, parse_odf, parse_pptx, parse_xlsb, parse_xlsx,
};
use structured::{parse_ics, parse_ics_string, parse_vcf, parse_vcf_string};
use text::{
    fallback_text_blocks, paged_text_blocks, parse_delimited_blocks, parse_ipynb,
    parse_json_document_blocks, parse_json_lines_blocks, parse_plain_text_document,
    parse_toml_document_blocks, parse_yaml_document_blocks, parsed_paged_text_document,
    parsed_structured_text_document, parsed_text_document, text_blocks,
};

#[derive(Default)]
pub struct CompositeDocumentParser {
    config: crate::config::DocumentParserConfig,
    ocr_provider: Option<Arc<dyn DocumentOcrProvider>>,
}

impl CompositeDocumentParser {
    pub(crate) fn with_config(config: crate::config::DocumentParserConfig) -> Self {
        Self {
            config,
            ocr_provider: None,
        }
    }

    pub(crate) fn with_config_and_ocr(
        config: crate::config::DocumentParserConfig,
        ocr_provider: Arc<dyn DocumentOcrProvider>,
    ) -> Self {
        Self {
            config,
            ocr_provider: Some(ocr_provider),
        }
    }

    #[cfg(test)]
    pub(crate) fn ocr_provider(&self) -> Option<&Arc<dyn DocumentOcrProvider>> {
        self.ocr_provider.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn ocr_provider_capabilities(&self) -> Option<DocumentOcrCapabilities> {
        self.ocr_provider
            .as_ref()
            .map(|provider| provider.capabilities())
    }

    #[cfg(test)]
    pub(crate) fn parse_document(&self, path: &Path) -> Result<ParsedDocument> {
        Ok(
            <Self as crate::document_parser::DocumentParser>::parse_extracted(self, path)?
                .into_parsed_document(),
        )
    }
}

impl crate::document_parser::DocumentParser for CompositeDocumentParser {
    fn name(&self) -> &str {
        "composite-document-parser"
    }

    fn signature(&self) -> String {
        let provider_signature = self
            .ocr_provider
            .as_ref()
            .map(|provider| {
                serde_json::to_string(&(provider.name(), provider.capabilities()))
                    .unwrap_or_else(|_| provider.name().to_string())
            })
            .unwrap_or_else(|| "none".to_string());

        format!(
            "{}@{}",
            self.name(),
            sha256::digest(
                serde_json::to_vec(&(&self.config, provider_signature)).unwrap_or_default()
            )
        )
    }

    fn supported_extensions(&self) -> &[&str] {
        SUPPORTED_EXTENSIONS
    }

    fn parse(&self, path: &Path) -> Result<String> {
        Ok(self.parse_extracted(path)?.into_parsed_document().to_text())
    }

    fn parse_extracted(&self, path: &Path) -> Result<crate::document_pipeline::ExtractedDocument> {
        Ok(crate::document_pipeline::ExtractedDocument::new(
            defaults::parse_document_with_default_extractors(
                path,
                &self.config,
                self.ocr_provider.as_deref(),
            )?,
        ))
    }

    fn max_file_size(&self) -> u64 {
        self.config.max_file_size_mb * 1024 * 1024
    }
}

#[cfg(test)]
mod tests;
