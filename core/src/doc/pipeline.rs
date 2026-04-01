//! Internal pipeline contracts for A3S Code document context preparation.
//!
//! These traits and cache keys support the built-in normalization, chunking,
//! validation, and OCR/cache stages used before document content is surfaced to
//! `agentic_search` and `agentic_parse`.

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

use super::{DocumentBlockKind, DocumentBlockLocation, ExtractedDocument, ParsedDocument};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentCacheKey {
    pub path: String,
    pub file_hash: String,
    pub parser: String,
    pub pipeline_signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentExtractionCacheKey {
    pub path: String,
    pub file_hash: String,
    pub parser: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentChunkCacheKey {
    pub path: String,
    pub document_hash: String,
    pub chunker: String,
    pub query: Option<String>,
    pub pipeline_signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentOcrCacheKey {
    pub path: String,
    pub file_hash: String,
    pub format: String,
    pub provider: String,
    pub ocr_signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentQualityCacheKey {
    pub path: String,
    pub document_hash: String,
    pub evaluator: String,
    pub validation_hash: String,
    pub pipeline_signature: String,
}

pub trait DocumentCacheStore: Send + Sync {
    fn name(&self) -> &str;

    fn get_extracted_document(
        &self,
        _key: &DocumentExtractionCacheKey,
    ) -> Result<Option<ExtractedDocument>> {
        Ok(None)
    }

    fn put_extracted_document(
        &self,
        _key: &DocumentExtractionCacheKey,
        _document: &ExtractedDocument,
    ) -> Result<()> {
        Ok(())
    }

    fn get_document(&self, key: &DocumentCacheKey) -> Result<Option<ParsedDocument>>;

    fn put_document(&self, key: &DocumentCacheKey, document: &ParsedDocument) -> Result<()>;

    fn get_chunks(&self, _key: &DocumentChunkCacheKey) -> Result<Option<Vec<DocumentChunk>>> {
        Ok(None)
    }

    fn put_chunks(&self, _key: &DocumentChunkCacheKey, _chunks: &[DocumentChunk]) -> Result<()> {
        Ok(())
    }

    fn get_ocr_payload(&self, _key: &DocumentOcrCacheKey) -> Result<Option<String>> {
        Ok(None)
    }

    fn put_ocr_payload(&self, _key: &DocumentOcrCacheKey, _payload: &str) -> Result<()> {
        Ok(())
    }

    fn get_quality_report(
        &self,
        _key: &DocumentQualityCacheKey,
    ) -> Result<Option<DocumentQualityReport>> {
        Ok(None)
    }

    fn put_quality_report(
        &self,
        _key: &DocumentQualityCacheKey,
        _report: &DocumentQualityReport,
    ) -> Result<()> {
        Ok(())
    }
}

pub trait DocumentPostProcessor: Send + Sync {
    fn name(&self) -> &str;

    fn signature(&self) -> String {
        self.name().to_string()
    }

    fn process(&self, path: &Path, document: &mut ParsedDocument) -> Result<()>;
}

pub trait DocumentMetadataEnricher: Send + Sync {
    fn name(&self) -> &str;

    fn signature(&self) -> String {
        self.name().to_string()
    }

    fn enrich(&self, path: &Path, document: &mut ParsedDocument) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DocumentValidationSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentValidationIssue {
    pub validator: String,
    pub severity: DocumentValidationSeverity,
    pub message: String,
}

impl DocumentValidationIssue {
    pub fn warning(validator: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            validator: validator.into(),
            severity: DocumentValidationSeverity::Warning,
            message: message.into(),
        }
    }

    pub fn error(validator: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            validator: validator.into(),
            severity: DocumentValidationSeverity::Error,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentValidationReport {
    pub issues: Vec<DocumentValidationIssue>,
}

impl DocumentValidationReport {
    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == DocumentValidationSeverity::Error)
    }

    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

pub trait DocumentValidator: Send + Sync {
    fn name(&self) -> &str;

    fn signature(&self) -> String {
        self.name().to_string()
    }

    fn validate(
        &self,
        path: &Path,
        document: &ParsedDocument,
    ) -> Result<Vec<DocumentValidationIssue>>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentChunk {
    pub label: Option<String>,
    pub context_label: Option<String>,
    pub content: String,
    pub keywords: Vec<String>,
    pub language: Option<String>,
    pub location: Option<DocumentBlockLocation>,
    pub location_display: Option<String>,
    pub locator: Option<String>,
    pub source: Option<String>,
    pub page: Option<usize>,
    pub ordinal: Option<usize>,
    pub block_indices: Vec<usize>,
    pub kind: Option<DocumentBlockKind>,
    pub score: usize,
}

pub trait DocumentChunker: Send + Sync {
    fn name(&self) -> &str;

    fn signature(&self) -> String {
        self.name().to_string()
    }

    fn chunk(
        &self,
        path: &Path,
        document: &ParsedDocument,
        query: Option<&str>,
    ) -> Result<Vec<DocumentChunk>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DocumentQualityGrade {
    Excellent,
    Good,
    Fair,
    Poor,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentQualityIssue {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentQualityReport {
    pub score: u8,
    pub grade: DocumentQualityGrade,
    pub issues: Vec<DocumentQualityIssue>,
    pub metrics: serde_json::Value,
}

pub trait DocumentQualityEvaluator: Send + Sync {
    fn name(&self) -> &str;

    fn signature(&self) -> String {
        self.name().to_string()
    }

    fn evaluate(
        &self,
        path: &Path,
        document: &ParsedDocument,
        validation_report: &DocumentValidationReport,
    ) -> Result<DocumentQualityReport>;
}

#[derive(Clone, Default)]
pub struct DocumentPipelineRegistry {
    post_processors: Vec<Arc<dyn DocumentPostProcessor>>,
    metadata_enrichers: Vec<Arc<dyn DocumentMetadataEnricher>>,
    validators: Vec<Arc<dyn DocumentValidator>>,
    chunkers: Vec<Arc<dyn DocumentChunker>>,
    quality_evaluators: Vec<Arc<dyn DocumentQualityEvaluator>>,
    cache_store: Option<Arc<dyn DocumentCacheStore>>,
}

impl DocumentPipelineRegistry {
    pub fn empty() -> Self {
        Self {
            post_processors: Vec::new(),
            metadata_enrichers: Vec::new(),
            validators: Vec::new(),
            chunkers: Vec::new(),
            quality_evaluators: Vec::new(),
            cache_store: None,
        }
    }

    pub fn register_cache_store(&mut self, cache_store: Arc<dyn DocumentCacheStore>) {
        self.cache_store = Some(cache_store);
    }

    pub fn register_post_processor(&mut self, processor: Arc<dyn DocumentPostProcessor>) {
        self.post_processors.push(processor);
    }

    pub fn register_metadata_enricher(&mut self, enricher: Arc<dyn DocumentMetadataEnricher>) {
        self.metadata_enrichers.push(enricher);
    }

    pub fn register_validator(&mut self, validator: Arc<dyn DocumentValidator>) {
        self.validators.push(validator);
    }

    pub fn register_chunker(&mut self, chunker: Arc<dyn DocumentChunker>) {
        self.chunkers.push(chunker);
    }

    pub fn register_quality_evaluator(&mut self, evaluator: Arc<dyn DocumentQualityEvaluator>) {
        self.quality_evaluators.push(evaluator);
    }

    pub fn process_document(
        &self,
        path: &Path,
        document: &mut ParsedDocument,
    ) -> Result<DocumentValidationReport> {
        for processor in &self.post_processors {
            processor.process(path, document)?;
        }
        for enricher in &self.metadata_enrichers {
            enricher.enrich(path, document)?;
        }
        let mut report = DocumentValidationReport::default();
        for validator in &self.validators {
            report.issues.extend(validator.validate(path, document)?);
        }
        Ok(report)
    }

    pub fn post_processors(&self) -> &[Arc<dyn DocumentPostProcessor>] {
        &self.post_processors
    }

    pub fn metadata_enrichers(&self) -> &[Arc<dyn DocumentMetadataEnricher>] {
        &self.metadata_enrichers
    }

    pub fn validators(&self) -> &[Arc<dyn DocumentValidator>] {
        &self.validators
    }

    pub fn chunkers(&self) -> &[Arc<dyn DocumentChunker>] {
        &self.chunkers
    }

    pub fn quality_evaluators(&self) -> &[Arc<dyn DocumentQualityEvaluator>] {
        &self.quality_evaluators
    }

    pub fn cache_store(&self) -> Option<&Arc<dyn DocumentCacheStore>> {
        self.cache_store.as_ref()
    }

    pub fn signature(&self) -> String {
        let cache = self
            .cache_store
            .as_ref()
            .map(|store| store.name())
            .unwrap_or("none");
        format!(
            "cache={cache};post={};enrich={};validate={};chunk={};quality={}",
            self.post_processors
                .iter()
                .map(|stage| stage.signature())
                .collect::<Vec<_>>()
                .join(","),
            self.metadata_enrichers
                .iter()
                .map(|stage| stage.signature())
                .collect::<Vec<_>>()
                .join(","),
            self.validators
                .iter()
                .map(|stage| stage.signature())
                .collect::<Vec<_>>()
                .join(","),
            self.chunkers
                .iter()
                .map(|stage| stage.signature())
                .collect::<Vec<_>>()
                .join(","),
            self.quality_evaluators
                .iter()
                .map(|stage| stage.signature())
                .collect::<Vec<_>>()
                .join(","),
        )
    }

    pub fn chunk_document(
        &self,
        path: &Path,
        document: &ParsedDocument,
        query: Option<&str>,
    ) -> Result<Vec<DocumentChunk>> {
        match self.chunkers.first() {
            Some(chunker) => {
                let cache_key = DocumentChunkCacheKey {
                    path: path.display().to_string(),
                    document_hash: sha256::digest(serde_json::to_vec(document)?),
                    chunker: chunker.signature(),
                    query: query.map(str::to_string),
                    pipeline_signature: self.signature(),
                };

                if let Some(cache_store) = &self.cache_store {
                    if let Some(chunks) = cache_store.get_chunks(&cache_key)? {
                        return Ok(chunks);
                    }
                }

                let chunks = chunker.chunk(path, document, query)?;
                if let Some(cache_store) = &self.cache_store {
                    cache_store.put_chunks(&cache_key, &chunks)?;
                }
                Ok(chunks)
            }
            None => Ok(Vec::new()),
        }
    }

    pub fn evaluate_document_quality(
        &self,
        path: &Path,
        document: &ParsedDocument,
        validation_report: &DocumentValidationReport,
    ) -> Result<Option<DocumentQualityReport>> {
        match self.quality_evaluators.first() {
            Some(evaluator) => {
                let cache_key = DocumentQualityCacheKey {
                    path: path.display().to_string(),
                    document_hash: sha256::digest(serde_json::to_vec(document)?),
                    evaluator: evaluator.signature(),
                    validation_hash: sha256::digest(serde_json::to_vec(validation_report)?),
                    pipeline_signature: self.signature(),
                };

                if let Some(cache_store) = &self.cache_store {
                    if let Some(report) = cache_store.get_quality_report(&cache_key)? {
                        return Ok(Some(report));
                    }
                }

                let report = evaluator.evaluate(path, document, validation_report)?;
                if let Some(cache_store) = &self.cache_store {
                    cache_store.put_quality_report(&cache_key, &report)?;
                }
                Ok(Some(report))
            }
            None => Ok(None),
        }
    }
}

impl std::fmt::Debug for DocumentPipelineRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentPipelineRegistry")
            .field(
                "post_processors",
                &self
                    .post_processors
                    .iter()
                    .map(|stage| stage.name())
                    .collect::<Vec<_>>(),
            )
            .field(
                "metadata_enrichers",
                &self
                    .metadata_enrichers
                    .iter()
                    .map(|stage| stage.name())
                    .collect::<Vec<_>>(),
            )
            .field(
                "validators",
                &self
                    .validators
                    .iter()
                    .map(|stage| stage.name())
                    .collect::<Vec<_>>(),
            )
            .field(
                "chunkers",
                &self
                    .chunkers
                    .iter()
                    .map(|stage| stage.name())
                    .collect::<Vec<_>>(),
            )
            .field(
                "quality_evaluators",
                &self
                    .quality_evaluators
                    .iter()
                    .map(|stage| stage.name())
                    .collect::<Vec<_>>(),
            )
            .field(
                "cache_store",
                &self.cache_store.as_ref().map(|store| store.name()),
            )
            .finish()
    }
}
