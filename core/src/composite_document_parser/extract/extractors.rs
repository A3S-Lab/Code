use anyhow::Result;
use std::path::Path;

use crate::document_parser::{DocumentBlockKind, ParsedDocument};

use super::{
    is_docx_family, is_flat_odf_family, is_html_family, is_ics_family, is_image_family,
    is_legacy_doc_family, is_legacy_ppt_family, is_legacy_xls_family, is_odf_family,
    is_plain_text_family, is_pptx_family, is_vcard_family, is_xlsx_family, is_xml_family,
};
use super::{
    parse_bib, parse_csl, parse_eml, parse_emlx, parse_enw, parse_epub, parse_html_document,
    parse_hwp, parse_hwpx, parse_ics, parse_image_document, parse_ipynb, parse_iwork_package,
    parse_mbox, parse_msg, parse_nbib, parse_odf, parse_pdf_document, parse_plain_text_document,
    parse_pptx, parse_ris, parse_rtf, parse_vcf, parse_xlsb, parse_xlsx, parse_xml_document,
    parse_zip, parsed_text_document, tagged_fields_to_text, DocumentOcrProvider,
};
use super::{parse_docx, parse_legacy_doc, parse_legacy_ppt, parse_legacy_xls};

pub(super) struct ExtractorContext<'a> {
    pub path: &'a Path,
    #[allow(dead_code)]
    pub ext: &'a str,
    pub detected_ext: &'a str,
    pub config: &'a crate::config::DocumentParserConfig,
    pub ocr_provider: Option<&'a dyn DocumentOcrProvider>,
}

pub(super) trait CompositeDocumentExtractor {
    #[cfg_attr(not(test), allow(dead_code))]
    fn name(&self) -> &'static str;
    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool;
    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument>;
}

pub(super) struct PdfExtractor;

impl CompositeDocumentExtractor for PdfExtractor {
    fn name(&self) -> &'static str {
        "pdf"
    }

    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool {
        ctx.detected_ext == "pdf"
    }

    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument> {
        parse_pdf_document(ctx.path, ctx.config, ctx.ocr_provider)
    }
}

pub(super) struct OfficeExtractor;

impl CompositeDocumentExtractor for OfficeExtractor {
    fn name(&self) -> &'static str {
        "office"
    }

    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool {
        is_legacy_doc_family(ctx.detected_ext)
            || is_docx_family(ctx.detected_ext)
            || is_legacy_xls_family(ctx.detected_ext)
            || is_xlsx_family(ctx.detected_ext)
            || is_legacy_ppt_family(ctx.detected_ext)
            || is_pptx_family(ctx.detected_ext)
            || ctx.detected_ext == "hwp"
            || ctx.detected_ext == "hwpx"
    }

    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument> {
        match ctx.detected_ext {
            ext if is_legacy_doc_family(ext) => parse_legacy_doc(ctx.path),
            ext if is_docx_family(ext) => parse_docx(ctx.path, ctx.config, ctx.ocr_provider),
            ext if is_legacy_xls_family(ext) => parse_legacy_xls(ctx.path),
            "xlsb" => parse_xlsb(ctx.path, ctx.config, ctx.ocr_provider),
            ext if is_xlsx_family(ext) => parse_xlsx(ctx.path, ctx.config, ctx.ocr_provider),
            ext if is_legacy_ppt_family(ext) => parse_legacy_ppt(ctx.path),
            ext if is_pptx_family(ext) => parse_pptx(ctx.path, ctx.config, ctx.ocr_provider),
            "hwp" => parse_hwp(ctx.path),
            "hwpx" => parse_hwpx(ctx.path),
            _ => anyhow::bail!("office extractor cannot parse {}", ctx.path.display()),
        }
    }
}

pub(super) struct OdfExtractor;

impl CompositeDocumentExtractor for OdfExtractor {
    fn name(&self) -> &'static str {
        "odf"
    }

    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool {
        is_odf_family(ctx.detected_ext) || is_flat_odf_family(ctx.detected_ext)
    }

    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument> {
        if is_odf_family(ctx.detected_ext) {
            parse_odf(ctx.path, ctx.config, ctx.ocr_provider)
        } else {
            parse_xml_document(ctx.path)
        }
    }
}

pub(super) struct EmailExtractor;

impl CompositeDocumentExtractor for EmailExtractor {
    fn name(&self) -> &'static str {
        "email"
    }

    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool {
        matches!(ctx.detected_ext, "eml" | "emlx" | "mbox" | "msg")
    }

    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument> {
        match ctx.detected_ext {
            "eml" => parse_eml(ctx.path),
            "emlx" => parse_emlx(ctx.path),
            "mbox" => parse_mbox(ctx.path),
            "msg" => parse_msg(ctx.path, super::normalize_text),
            _ => anyhow::bail!("email extractor cannot parse {}", ctx.path.display()),
        }
    }
}

pub(super) struct StructuredDataExtractor;

impl CompositeDocumentExtractor for StructuredDataExtractor {
    fn name(&self) -> &'static str {
        "structured-data"
    }

    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool {
        matches!(
            ctx.detected_ext,
            "epub"
                | "zip"
                | "pages"
                | "numbers"
                | "key"
                | "ipynb"
                | "ris"
                | "enw"
                | "nbib"
                | "bib"
                | "bibtex"
                | "csl"
                | "rtf"
        ) || is_ics_family(ctx.detected_ext)
            || is_vcard_family(ctx.detected_ext)
            || is_plain_text_family(ctx.detected_ext)
    }

    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument> {
        match ctx.detected_ext {
            "epub" => parse_epub(ctx.path),
            "zip" => parse_zip(ctx.path),
            "pages" | "numbers" | "key" => parse_iwork_package(ctx.path, ctx.detected_ext),
            "ipynb" => parse_ipynb(ctx.path),
            "ris" => parse_ris(ctx.path, super::normalize_text),
            "enw" => parse_enw(ctx.path, super::normalize_text),
            "nbib" => parse_nbib(ctx.path, super::normalize_text),
            "bib" | "bibtex" => parse_bib(ctx.path, super::normalize_text),
            "csl" => parse_csl(ctx.path, super::normalize_text),
            ext if is_ics_family(ext) => {
                parse_ics(ctx.path, super::normalize_text, tagged_fields_to_text)
            }
            ext if is_vcard_family(ext) => {
                parse_vcf(ctx.path, super::normalize_text, tagged_fields_to_text)
            }
            "rtf" => {
                parsed_text_document(ctx.path, parse_rtf(ctx.path)?, DocumentBlockKind::Paragraph)
            }
            ext if is_plain_text_family(ext) => parse_plain_text_document(ctx.path),
            _ => anyhow::bail!(
                "structured-data extractor cannot parse {}",
                ctx.path.display()
            ),
        }
    }
}

pub(super) struct HtmlXmlExtractor;

impl CompositeDocumentExtractor for HtmlXmlExtractor {
    fn name(&self) -> &'static str {
        "html-xml"
    }

    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool {
        is_html_family(ctx.detected_ext) || is_xml_family(ctx.detected_ext)
    }

    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument> {
        if is_html_family(ctx.detected_ext) {
            parse_html_document(ctx.path)
        } else {
            parse_xml_document(ctx.path)
        }
    }
}

pub(super) struct ImageOcrExtractor;

impl CompositeDocumentExtractor for ImageOcrExtractor {
    fn name(&self) -> &'static str {
        "image-ocr"
    }

    fn can_extract(&self, ctx: &ExtractorContext<'_>) -> bool {
        is_image_family(ctx.detected_ext)
    }

    fn extract(&self, ctx: &ExtractorContext<'_>) -> Result<ParsedDocument> {
        parse_image_document(ctx.path, ctx.config, ctx.ocr_provider)
    }
}
