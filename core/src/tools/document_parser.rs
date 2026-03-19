//! Document Parser Extension Point
//!
//! Re-exports the canonical types from [`crate::document_parser`].
//! New code should import directly from `a3s_code_core::document_parser`.

pub use crate::document_parser::{DocumentParser, DocumentParserRegistry, PlainTextParser};
