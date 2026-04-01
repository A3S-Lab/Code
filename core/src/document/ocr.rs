//! OCR integration surface for A3S Code document context extraction.
//!
//! The stable external contract here is the OCR backend trait and its request /
//! result types so hosts can improve `agentic_search` and `agentic_parse`
//! context extraction on scanned or image-heavy files.

use crate::document_parser::ParsedDocument;

pub use crate::composite_document_parser::{
    DocumentOcrCapabilities, DocumentOcrFormat, DocumentOcrOutput, DocumentOcrPageResult,
    DocumentOcrProvider, DocumentOcrRequest, DocumentOcrRuntimeInfo, DocumentRuntimeMetadata,
};

pub(crate) fn extract_document_ocr_runtime_metadata(
    doc: &ParsedDocument,
) -> Option<DocumentRuntimeMetadata> {
    crate::composite_document_parser::extract_document_runtime_metadata(doc)
}
