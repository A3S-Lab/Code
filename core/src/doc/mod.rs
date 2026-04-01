//! Document model and pipeline contracts for A3S Code context extraction.
//!
//! This module provides the core document data types and pipeline traits used by
//! `agentic_search`, `agentic_parse`, and the document parser registry.

pub(crate) mod model;
pub(crate) mod parser;
pub(crate) mod pipeline;
pub(crate) mod registry;

pub use model::{
    DocumentBlock, DocumentBlockKind, DocumentBlockLocation, DocumentConfidence,
    DocumentExtractionMetadata, DocumentMetadata, DocumentProvenance, ExtractedDocument,
    ParsedDocument, StructuredElement, StructuredElementKind,
};
pub use parser::DocumentParser;
pub use registry::DocumentParserRegistry;
