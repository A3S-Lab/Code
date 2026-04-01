use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentBlockKind {
    Paragraph,
    Heading,
    Table,
    Section,
    Metadata,
    Slide,
    EmailHeader,
    Code,
    Raw,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentBlockLocation {
    pub source: Option<String>,
    pub page: Option<usize>,
    pub ordinal: Option<usize>,
    pub continued_from_previous_page: bool,
    pub continued_to_next_page: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentProvenance {
    pub parser: Option<String>,
    pub extractor: Option<String>,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentConfidence {
    pub score_percent: Option<u8>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub source_mime_type: Option<String>,
    pub detected_file_type: Option<String>,
    pub language: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub provenance: Option<DocumentProvenance>,
    pub confidence: Option<DocumentConfidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentBlock {
    pub kind: DocumentBlockKind,
    pub label: Option<String>,
    pub content: String,
    pub location: Option<DocumentBlockLocation>,
    pub attributes: BTreeMap<String, String>,
    pub structured_payload: Option<String>,
    pub metadata: Option<DocumentMetadata>,
}

impl DocumentBlock {
    pub fn new(
        kind: DocumentBlockKind,
        label: Option<impl Into<String>>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            label: label.map(Into::into),
            content: content.into(),
            location: None,
            attributes: BTreeMap::new(),
            structured_payload: None,
            metadata: None,
        }
    }

    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    pub fn with_structured_payload(mut self, payload: impl Into<String>) -> Self {
        self.structured_payload = Some(payload.into());
        self
    }

    pub fn with_metadata(mut self, metadata: DocumentMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .source = Some(source.into());
        self
    }

    pub fn with_page(mut self, page: usize) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .page = Some(page);
        self
    }

    pub fn with_ordinal(mut self, ordinal: usize) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .ordinal = Some(ordinal);
        self
    }

    pub fn with_continued_from_previous_page(mut self, continued: bool) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .continued_from_previous_page = continued;
        self
    }

    pub fn with_continued_to_next_page(mut self, continued: bool) -> Self {
        self.location
            .get_or_insert_with(DocumentBlockLocation::default)
            .continued_to_next_page = continued;
        self
    }
}

/// Structured table representation for stable machine-readable table output.
/// This provides a consistent format for table data across all document types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredTable {
    /// Index of the table block in the document blocks array.
    pub index: usize,
    /// Page number where this table appears, if known.
    pub page: Option<usize>,
    /// Source file or section reference.
    pub source: Option<String>,
    /// Number of data rows (excluding header).
    pub row_count: usize,
    /// Number of columns.
    pub column_count: usize,
    /// Header row values, if detected.
    pub headers: Vec<String>,
    /// All data rows (excluding header if detected).
    pub rows: Vec<Vec<String>>,
    /// Location metadata for this table.
    pub location: Option<DocumentBlockLocation>,
    /// Original block label.
    pub label: Option<String>,
}

impl StructuredTable {
    /// Create a new structured table from block data.
    pub fn new(
        index: usize,
        row_count: usize,
        column_count: usize,
        rows: Vec<Vec<String>>,
    ) -> Self {
        let headers = rows.first().cloned().unwrap_or_default();
        Self {
            index,
            page: None,
            source: None,
            row_count,
            column_count,
            headers,
            rows,
            location: None,
            label: None,
        }
    }

    /// Set the page number.
    pub fn with_page(mut self, page: usize) -> Self {
        self.page = Some(page);
        self
    }

    /// Set the source.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the location.
    pub fn with_location(mut self, location: DocumentBlockLocation) -> Self {
        self.location = Some(location);
        self
    }

    /// Set the label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// Page-level information for documents that support page structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageInfo {
    /// Page number (1-indexed).
    pub page: usize,
    /// Source file or section reference.
    pub source: Option<String>,
    /// Number of blocks on this page.
    pub block_count: usize,
    /// Section headings or labels on this page.
    pub labels: Vec<String>,
    /// First few lines of content for preview.
    pub preview: Option<String>,
    /// Whether content continues from previous page.
    pub continued_from_previous_page: bool,
    /// Whether content continues to next page.
    pub continued_to_next_page: bool,
}

impl PageInfo {
    /// Create a new page info.
    pub fn new(page: usize) -> Self {
        Self {
            page,
            source: None,
            block_count: 0,
            labels: Vec::new(),
            preview: None,
            continued_from_previous_page: false,
            continued_to_next_page: false,
        }
    }

    /// Set the source.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set block count.
    pub fn with_block_count(mut self, count: usize) -> Self {
        self.block_count = count;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedDocument {
    pub title: Option<String>,
    pub blocks: Vec<DocumentBlock>,
    pub metadata: Option<DocumentMetadata>,
    /// Stable table output for machine-readable consumption.
    pub tables: Vec<StructuredTable>,
    /// Page-level information for documents with page structure.
    pub pages: Vec<PageInfo>,
}

impl ParsedDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            title: None,
            blocks: vec![DocumentBlock::new(
                DocumentBlockKind::Raw,
                None::<String>,
                text,
            )],
            metadata: None,
            tables: Vec::new(),
            pages: Vec::new(),
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn push(&mut self, block: DocumentBlock) {
        self.blocks.push(block);
    }

    pub fn with_metadata(mut self, metadata: DocumentMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn non_empty_block_count(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| !block.content.trim().is_empty())
            .count()
    }

    pub fn char_count(&self) -> usize {
        self.to_text().chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|b| b.content.trim().is_empty())
    }

    pub fn to_text(&self) -> String {
        let mut parts = Vec::new();
        if let Some(title) = &self.title {
            if !title.trim().is_empty() {
                parts.push(title.trim().to_string());
            }
        }
        for block in &self.blocks {
            let mut chunk = String::new();
            if let Some(label) = &block.label {
                if !label.trim().is_empty() {
                    chunk.push_str(label.trim());
                    chunk.push('\n');
                }
            }
            chunk.push_str(block.content.trim());
            if !chunk.trim().is_empty() {
                parts.push(chunk.trim().to_string());
            }
        }
        parts.join("\n\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedDocument {
    pub document: ParsedDocument,
    pub extraction_metadata: Option<DocumentExtractionMetadata>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentExtractionMetadata {
    pub parser_name: Option<String>,
    pub parser_signature: Option<String>,
    pub extractor: Option<String>,
    pub detected_file_type: Option<String>,
}

impl ExtractedDocument {
    pub fn new(document: ParsedDocument) -> Self {
        Self {
            document,
            extraction_metadata: None,
        }
    }

    pub fn into_parsed_document(self) -> ParsedDocument {
        self.document
    }

    pub fn as_parsed_document(&self) -> &ParsedDocument {
        &self.document
    }

    pub fn with_extraction_metadata(mut self, metadata: DocumentExtractionMetadata) -> Self {
        self.extraction_metadata = Some(metadata);
        self
    }
}

impl From<ParsedDocument> for ExtractedDocument {
    fn from(value: ParsedDocument) -> Self {
        Self::new(value)
    }
}
