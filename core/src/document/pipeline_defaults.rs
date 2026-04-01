use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::DocumentParserConfig;
use crate::doc::pipeline::{DocumentQualityEvaluator, DocumentQualityGrade, DocumentQualityIssue};
use crate::document_parser::{
    DocumentBlock, DocumentBlockKind, DocumentBlockLocation, DocumentMetadata, ParsedDocument,
};
use crate::document_pipeline::{
    DocumentCacheKey, DocumentCacheStore, DocumentChunk, DocumentChunkCacheKey, DocumentChunker,
    DocumentExtractionCacheKey, DocumentMetadataEnricher, DocumentOcrCacheKey,
    DocumentPipelineRegistry, DocumentPostProcessor, DocumentQualityCacheKey,
    DocumentQualityReport, DocumentValidationIssue, DocumentValidator, ExtractedDocument,
};

pub(crate) fn build_default_document_pipeline_registry() -> DocumentPipelineRegistry {
    build_default_document_pipeline_registry_for_config(&DocumentParserConfig::default())
}

pub(crate) fn build_default_document_pipeline_registry_for_config(
    config: &DocumentParserConfig,
) -> DocumentPipelineRegistry {
    let mut registry = DocumentPipelineRegistry::empty();
    registry.register_cache_store(build_default_document_cache_store(config));
    registry.register_post_processor(Arc::new(DocumentWhitespaceNormalizer));
    registry.register_metadata_enricher(Arc::new(DocumentTitleEnricher));
    registry.register_metadata_enricher(Arc::new(DocumentStageEnricher));
    registry.register_metadata_enricher(Arc::new(BlockSourceEnricher));
    registry.register_metadata_enricher(Arc::new(DocumentLanguageEnricher));
    registry.register_metadata_enricher(Arc::new(DocumentKeywordEnricher));
    registry.register_validator(Arc::new(EmptyDocumentValidator));
    registry.register_validator(Arc::new(ContentPresenceValidator));
    registry.register_chunker(Arc::new(HierarchicalDocumentChunker));
    registry.register_quality_evaluator(Arc::new(DefaultDocumentQualityEvaluator));
    registry
}

pub(crate) fn build_default_document_cache_store(
    config: &DocumentParserConfig,
) -> Arc<dyn DocumentCacheStore> {
    let cache_config = config.cache.as_ref().cloned().unwrap_or_default();
    if !cache_config.enabled {
        return Arc::new(InMemoryDocumentCache::default());
    }

    if let Some(directory) = resolve_document_cache_directory(cache_config.directory.as_deref()) {
        return Arc::new(FileSystemDocumentCache::new(directory));
    }

    Arc::new(InMemoryDocumentCache::default())
}

fn resolve_document_cache_directory(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured {
        return Some(path.to_path_buf());
    }

    dirs::cache_dir().map(|dir| dir.join("a3s").join("document-cache"))
}

#[derive(Default)]
struct InMemoryDocumentCache {
    extracted_documents: Mutex<HashMap<String, ExtractedDocument>>,
    documents: Mutex<HashMap<String, ParsedDocument>>,
    chunks: Mutex<HashMap<String, Vec<DocumentChunk>>>,
    quality_reports: Mutex<HashMap<String, DocumentQualityReport>>,
    ocr_payloads: Mutex<HashMap<String, String>>,
}

impl DocumentCacheStore for InMemoryDocumentCache {
    fn name(&self) -> &str {
        "in-memory-document-cache"
    }

    fn get_extracted_document(
        &self,
        key: &DocumentExtractionCacheKey,
    ) -> anyhow::Result<Option<ExtractedDocument>> {
        Ok(self
            .extracted_documents
            .lock()
            .expect("document extraction cache lock poisoned")
            .get(&extraction_cache_key_string(key))
            .cloned())
    }

    fn put_extracted_document(
        &self,
        key: &DocumentExtractionCacheKey,
        document: &ExtractedDocument,
    ) -> anyhow::Result<()> {
        self.extracted_documents
            .lock()
            .expect("document extraction cache lock poisoned")
            .insert(extraction_cache_key_string(key), document.clone());
        Ok(())
    }

    fn get_document(&self, key: &DocumentCacheKey) -> anyhow::Result<Option<ParsedDocument>> {
        Ok(self
            .documents
            .lock()
            .expect("document cache lock poisoned")
            .get(&cache_key_string(key))
            .cloned())
    }

    fn put_document(
        &self,
        key: &DocumentCacheKey,
        document: &ParsedDocument,
    ) -> anyhow::Result<()> {
        self.documents
            .lock()
            .expect("document cache lock poisoned")
            .insert(cache_key_string(key), document.clone());
        Ok(())
    }

    fn get_chunks(
        &self,
        key: &DocumentChunkCacheKey,
    ) -> anyhow::Result<Option<Vec<DocumentChunk>>> {
        Ok(self
            .chunks
            .lock()
            .expect("chunk cache lock poisoned")
            .get(&chunk_cache_key_string(key))
            .cloned())
    }

    fn put_chunks(
        &self,
        key: &DocumentChunkCacheKey,
        chunks: &[DocumentChunk],
    ) -> anyhow::Result<()> {
        self.chunks
            .lock()
            .expect("chunk cache lock poisoned")
            .insert(chunk_cache_key_string(key), chunks.to_vec());
        Ok(())
    }

    fn get_quality_report(
        &self,
        key: &DocumentQualityCacheKey,
    ) -> anyhow::Result<Option<DocumentQualityReport>> {
        Ok(self
            .quality_reports
            .lock()
            .expect("quality cache lock poisoned")
            .get(&quality_cache_key_string(key))
            .cloned())
    }

    fn put_quality_report(
        &self,
        key: &DocumentQualityCacheKey,
        report: &DocumentQualityReport,
    ) -> anyhow::Result<()> {
        self.quality_reports
            .lock()
            .expect("quality cache lock poisoned")
            .insert(quality_cache_key_string(key), report.clone());
        Ok(())
    }

    fn get_ocr_payload(&self, key: &DocumentOcrCacheKey) -> anyhow::Result<Option<String>> {
        Ok(self
            .ocr_payloads
            .lock()
            .expect("ocr cache lock poisoned")
            .get(&ocr_cache_key_string(key))
            .cloned())
    }

    fn put_ocr_payload(&self, key: &DocumentOcrCacheKey, payload: &str) -> anyhow::Result<()> {
        self.ocr_payloads
            .lock()
            .expect("ocr cache lock poisoned")
            .insert(ocr_cache_key_string(key), payload.to_string());
        Ok(())
    }
}

fn cache_key_string(key: &DocumentCacheKey) -> String {
    format!(
        "{}|{}|{}|{}",
        key.path, key.file_hash, key.parser, key.pipeline_signature
    )
}

fn extraction_cache_key_string(key: &DocumentExtractionCacheKey) -> String {
    format!("{}|{}|{}", key.path, key.file_hash, key.parser)
}

fn chunk_cache_key_string(key: &DocumentChunkCacheKey) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        key.path,
        key.document_hash,
        key.chunker,
        key.query.as_deref().unwrap_or_default(),
        key.pipeline_signature
    )
}

fn ocr_cache_key_string(key: &DocumentOcrCacheKey) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        key.path, key.file_hash, key.format, key.provider, key.ocr_signature
    )
}

fn quality_cache_key_string(key: &DocumentQualityCacheKey) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        key.path, key.document_hash, key.evaluator, key.validation_hash, key.pipeline_signature
    )
}

struct FileSystemDocumentCache {
    root: PathBuf,
}

impl FileSystemDocumentCache {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn extraction_entry_path(&self, key: &DocumentExtractionCacheKey) -> PathBuf {
        self.root.join("raw").join(format!(
            "{}.json",
            sha256::digest(extraction_cache_key_string(key))
        ))
    }

    fn entry_path(&self, key: &DocumentCacheKey) -> PathBuf {
        self.root
            .join(format!("{}.json", sha256::digest(cache_key_string(key))))
    }

    fn ocr_entry_path(&self, key: &DocumentOcrCacheKey) -> PathBuf {
        self.root.join("ocr").join(format!(
            "{}.json",
            sha256::digest(ocr_cache_key_string(key))
        ))
    }

    fn quality_entry_path(&self, key: &DocumentQualityCacheKey) -> PathBuf {
        self.root.join("quality").join(format!(
            "{}.json",
            sha256::digest(quality_cache_key_string(key))
        ))
    }
}

impl DocumentCacheStore for FileSystemDocumentCache {
    fn name(&self) -> &str {
        "filesystem-document-cache"
    }

    fn get_extracted_document(
        &self,
        key: &DocumentExtractionCacheKey,
    ) -> anyhow::Result<Option<ExtractedDocument>> {
        let path = self.extraction_entry_path(key);
        if !path.is_file() {
            return Ok(None);
        }

        let bytes = std::fs::read(path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    fn put_extracted_document(
        &self,
        key: &DocumentExtractionCacheKey,
        document: &ExtractedDocument,
    ) -> anyhow::Result<()> {
        let path = self.extraction_entry_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(document)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn get_document(&self, key: &DocumentCacheKey) -> anyhow::Result<Option<ParsedDocument>> {
        let path = self.entry_path(key);
        if !path.is_file() {
            return Ok(None);
        }

        let bytes = std::fs::read(path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    fn put_document(
        &self,
        key: &DocumentCacheKey,
        document: &ParsedDocument,
    ) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.entry_path(key);
        let bytes = serde_json::to_vec(document)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn get_chunks(
        &self,
        key: &DocumentChunkCacheKey,
    ) -> anyhow::Result<Option<Vec<DocumentChunk>>> {
        let path = self.root.join("chunks").join(format!(
            "{}.json",
            sha256::digest(chunk_cache_key_string(key))
        ));
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    fn put_chunks(
        &self,
        key: &DocumentChunkCacheKey,
        chunks: &[DocumentChunk],
    ) -> anyhow::Result<()> {
        let dir = self.root.join("chunks");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!(
            "{}.json",
            sha256::digest(chunk_cache_key_string(key))
        ));
        let bytes = serde_json::to_vec(chunks)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn get_quality_report(
        &self,
        key: &DocumentQualityCacheKey,
    ) -> anyhow::Result<Option<DocumentQualityReport>> {
        let path = self.quality_entry_path(key);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    fn put_quality_report(
        &self,
        key: &DocumentQualityCacheKey,
        report: &DocumentQualityReport,
    ) -> anyhow::Result<()> {
        let path = self.quality_entry_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(report)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn get_ocr_payload(&self, key: &DocumentOcrCacheKey) -> anyhow::Result<Option<String>> {
        let path = self.ocr_entry_path(key);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(std::fs::read_to_string(path)?))
    }

    fn put_ocr_payload(&self, key: &DocumentOcrCacheKey, payload: &str) -> anyhow::Result<()> {
        let path = self.ocr_entry_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, payload)?;
        Ok(())
    }
}

pub(crate) fn build_default_document_chunk(idx: usize, block: &DocumentBlock) -> DocumentChunk {
    DocumentChunk {
        label: block.label.clone(),
        context_label: None,
        content: block.content.clone(),
        keywords: block
            .attributes
            .get("keywords")
            .map(|value| split_keyword_attribute(value))
            .unwrap_or_default(),
        language: block
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.language.clone())
            .or_else(|| block.attributes.get("language").cloned()),
        location: block.location.clone(),
        location_display: block
            .location
            .as_ref()
            .map(crate::document_render::format_block_location)
            .filter(|value| !value.is_empty()),
        locator: crate::document_render::derive_locator_from_location_and_label(
            block.location.as_ref(),
            block.label.as_deref(),
        ),
        source: block
            .location
            .as_ref()
            .and_then(|location| location.source.clone()),
        page: block.location.as_ref().and_then(|location| location.page),
        ordinal: block
            .location
            .as_ref()
            .and_then(|location| location.ordinal),
        block_indices: vec![idx],
        kind: Some(block.kind.clone()),
        score: 0,
    }
}

pub(crate) fn build_default_document_chunks(document: &ParsedDocument) -> Vec<DocumentChunk> {
    document
        .blocks
        .iter()
        .enumerate()
        .map(|(idx, block)| build_default_document_chunk(idx, block))
        .collect()
}

pub(crate) fn chunk_document_with_default_pipeline(
    path: &std::path::Path,
    document: &ParsedDocument,
    query: Option<&str>,
) -> Vec<DocumentChunk> {
    build_default_document_pipeline_registry()
        .chunk_document(path, document, query)
        .unwrap_or_else(|_| build_default_document_chunks(document))
}

struct DocumentWhitespaceNormalizer;

impl DocumentPostProcessor for DocumentWhitespaceNormalizer {
    fn name(&self) -> &str {
        "whitespace-normalizer"
    }

    fn process(
        &self,
        _path: &std::path::Path,
        document: &mut ParsedDocument,
    ) -> anyhow::Result<()> {
        if let Some(title) = document.title.as_mut() {
            *title = normalize_whitespace(title);
        }

        for block in &mut document.blocks {
            if let Some(label) = block.label.as_mut() {
                *label = normalize_whitespace(label);
            }
            block.content = normalize_whitespace(&block.content);
        }

        Ok(())
    }
}

struct DocumentTitleEnricher;

impl DocumentMetadataEnricher for DocumentTitleEnricher {
    fn name(&self) -> &str {
        "title-enricher"
    }

    fn enrich(&self, path: &std::path::Path, document: &mut ParsedDocument) -> anyhow::Result<()> {
        let has_title = document
            .title
            .as_ref()
            .is_some_and(|title| !title.trim().is_empty());
        if !has_title {
            document.title = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
        }
        Ok(())
    }
}

struct BlockSourceEnricher;

impl DocumentMetadataEnricher for BlockSourceEnricher {
    fn name(&self) -> &str {
        "block-source-enricher"
    }

    fn enrich(&self, path: &std::path::Path, document: &mut ParsedDocument) -> anyhow::Result<()> {
        let source = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| path.display().to_string());

        for block in &mut document.blocks {
            let needs_source = match block
                .location
                .as_ref()
                .and_then(|location| location.source.as_ref())
            {
                Some(value) => value.trim().is_empty(),
                None => true,
            };
            if needs_source {
                block
                    .location
                    .get_or_insert_with(DocumentBlockLocation::default)
                    .source = Some(source.clone());
            }
        }

        Ok(())
    }
}

struct DocumentStageEnricher;

impl DocumentMetadataEnricher for DocumentStageEnricher {
    fn name(&self) -> &str {
        "document-stage-enricher"
    }

    fn enrich(&self, _path: &std::path::Path, document: &mut ParsedDocument) -> anyhow::Result<()> {
        document
            .metadata
            .get_or_insert_with(crate::document_parser::DocumentMetadata::default)
            .attributes
            .insert("document.stage".to_string(), "normalized".to_string());
        Ok(())
    }
}

struct EmptyDocumentValidator;

impl DocumentValidator for EmptyDocumentValidator {
    fn name(&self) -> &str {
        "empty-document-validator"
    }

    fn validate(
        &self,
        _path: &std::path::Path,
        document: &ParsedDocument,
    ) -> anyhow::Result<Vec<DocumentValidationIssue>> {
        if document.blocks.is_empty() {
            return Ok(vec![DocumentValidationIssue::error(
                self.name(),
                "document has no blocks after extraction",
            )]);
        }
        Ok(Vec::new())
    }
}

struct ContentPresenceValidator;

impl DocumentValidator for ContentPresenceValidator {
    fn name(&self) -> &str {
        "content-presence-validator"
    }

    fn validate(
        &self,
        _path: &std::path::Path,
        document: &ParsedDocument,
    ) -> anyhow::Result<Vec<DocumentValidationIssue>> {
        let has_visible_content = document.blocks.iter().any(|block| {
            !block.content.trim().is_empty()
                || block
                    .label
                    .as_ref()
                    .is_some_and(|label| !label.trim().is_empty())
        });

        if has_visible_content {
            return Ok(Vec::new());
        }

        Ok(vec![DocumentValidationIssue::error(
            self.name(),
            "document has no visible labels or text content after normalization",
        )])
    }
}

struct HierarchicalDocumentChunker;

impl DocumentChunker for HierarchicalDocumentChunker {
    fn name(&self) -> &str {
        "hierarchical-document-chunker"
    }

    fn signature(&self) -> String {
        format!("{}@v1-heading-aware", self.name())
    }

    fn chunk(
        &self,
        _path: &std::path::Path,
        document: &ParsedDocument,
        query: Option<&str>,
    ) -> anyhow::Result<Vec<DocumentChunk>> {
        let keywords = query_keywords(query);
        let mut chunks = build_hierarchical_document_chunks(document);
        for chunk in &mut chunks {
            chunk.score = score_chunk_for_keywords(chunk, &keywords);
        }

        if !keywords.is_empty() {
            chunks.sort_by(|a, b| {
                b.score.cmp(&a.score).then_with(|| {
                    a.block_indices
                        .first()
                        .copied()
                        .unwrap_or(usize::MAX)
                        .cmp(&b.block_indices.first().copied().unwrap_or(usize::MAX))
                })
            });
        }

        Ok(chunks)
    }
}

struct DocumentLanguageEnricher;

impl DocumentMetadataEnricher for DocumentLanguageEnricher {
    fn name(&self) -> &str {
        "document-language-enricher"
    }

    fn enrich(&self, _path: &std::path::Path, document: &mut ParsedDocument) -> anyhow::Result<()> {
        let language = infer_document_language(document);
        let metadata = document
            .metadata
            .get_or_insert_with(DocumentMetadata::default);
        if let Some(language) = language.clone() {
            metadata.language = Some(language.clone());
            metadata
                .attributes
                .insert("document.language".to_string(), language.clone());
        }

        for block in &mut document.blocks {
            if block
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.language.as_ref())
                .is_some()
            {
                continue;
            }
            let block_language = infer_language_from_text(&block.content)
                .or_else(|| block.label.as_deref().and_then(infer_language_from_text))
                .or_else(|| language.clone());
            if let Some(block_language) = block_language {
                let metadata = block.metadata.get_or_insert_with(DocumentMetadata::default);
                metadata.language = Some(block_language.clone());
                block
                    .attributes
                    .insert("language".to_string(), block_language);
            }
        }

        Ok(())
    }
}

struct DocumentKeywordEnricher;

impl DocumentMetadataEnricher for DocumentKeywordEnricher {
    fn name(&self) -> &str {
        "document-keyword-enricher"
    }

    fn enrich(&self, _path: &std::path::Path, document: &mut ParsedDocument) -> anyhow::Result<()> {
        let document_keywords = extract_document_keywords(document, 12);
        if !document_keywords.is_empty() {
            let metadata = document
                .metadata
                .get_or_insert_with(DocumentMetadata::default);
            metadata.attributes.insert(
                "document.keywords".to_string(),
                document_keywords.join(", "),
            );
        }

        for block in &mut document.blocks {
            let block_keywords = extract_keywords_from_parts(
                [
                    block.label.as_deref().unwrap_or_default(),
                    block.content.as_str(),
                ],
                6,
            );
            if block_keywords.is_empty() {
                continue;
            }
            block
                .attributes
                .insert("keywords".to_string(), block_keywords.join(", "));
        }
        Ok(())
    }
}

struct DefaultDocumentQualityEvaluator;

impl DocumentQualityEvaluator for DefaultDocumentQualityEvaluator {
    fn name(&self) -> &str {
        "default-document-quality-evaluator"
    }

    fn evaluate(
        &self,
        _path: &Path,
        document: &ParsedDocument,
        validation_report: &crate::document_pipeline::DocumentValidationReport,
    ) -> anyhow::Result<DocumentQualityReport> {
        let non_empty_blocks = document
            .blocks
            .iter()
            .filter(|block| !block.content.trim().is_empty())
            .count();
        let table_blocks = document
            .blocks
            .iter()
            .filter(|block| block.kind == DocumentBlockKind::Table)
            .count();
        let average_block_length = if non_empty_blocks == 0 {
            0
        } else {
            document
                .blocks
                .iter()
                .map(|block| block.content.trim().chars().count())
                .sum::<usize>()
                / non_empty_blocks
        };
        let garbled_block_count = document
            .blocks
            .iter()
            .filter(|block| looks_garbled(&block.content))
            .count();
        let ocr_block_count = document
            .blocks
            .iter()
            .filter(|block| {
                block.attributes.contains_key("ocr_page")
                    || block
                        .attributes
                        .get("extraction")
                        .is_some_and(|value| value.contains("ocr"))
            })
            .count();

        let mut score = 100i32;
        score -= (validation_report.issues.len() as i32) * 12;
        score -= (garbled_block_count as i32) * 8;
        if non_empty_blocks == 0 {
            score -= 40;
        }
        if average_block_length < 20 && non_empty_blocks > 0 {
            score -= 10;
        }
        if ocr_block_count > 0 {
            score -= 5;
        }
        score = score.clamp(0, 100);

        let grade = match score {
            90..=100 => DocumentQualityGrade::Excellent,
            75..=89 => DocumentQualityGrade::Good,
            55..=74 => DocumentQualityGrade::Fair,
            _ => DocumentQualityGrade::Poor,
        };

        let mut issues = validation_report
            .issues
            .iter()
            .map(|issue| DocumentQualityIssue {
                code: format!("validation.{}", issue.validator),
                message: issue.message.clone(),
            })
            .collect::<Vec<_>>();
        if garbled_block_count > 0 {
            issues.push(DocumentQualityIssue {
                code: "content.garbled_blocks".to_string(),
                message: format!("{garbled_block_count} blocks look garbled"),
            });
        }
        if ocr_block_count > 0 {
            issues.push(DocumentQualityIssue {
                code: "content.ocr_dependent".to_string(),
                message: format!("{ocr_block_count} blocks depend on OCR"),
            });
        }
        if average_block_length < 20 && non_empty_blocks > 0 {
            issues.push(DocumentQualityIssue {
                code: "content.short_blocks".to_string(),
                message: "average block length is very short".to_string(),
            });
        }

        Ok(DocumentQualityReport {
            score: score as u8,
            grade,
            issues,
            metrics: serde_json::json!({
                "block_count": document.block_count(),
                "non_empty_block_count": non_empty_blocks,
                "table_block_count": table_blocks,
                "ocr_block_count": ocr_block_count,
                "garbled_block_count": garbled_block_count,
                "average_block_length": average_block_length,
                "document_language": document.metadata.as_ref().and_then(|metadata| metadata.language.clone()),
            }),
        })
    }
}

fn normalize_whitespace(text: &str) -> String {
    let mut lines = Vec::new();
    let mut blank_run = 0usize;

    for raw_line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            blank_run += 1;
            if blank_run == 1 {
                lines.push(String::new());
            }
            continue;
        }

        blank_run = 0;
        lines.push(line.to_string());
    }

    while matches!(lines.last(), Some(last) if last.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

fn query_keywords(query: Option<&str>) -> Vec<String> {
    let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) else {
        return Vec::new();
    };

    let mut keywords = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for token in query.split_whitespace() {
        let normalized = token
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_ascii_lowercase();
        if normalized.len() >= 2 && seen.insert(normalized.clone()) {
            keywords.push(normalized);
        }
    }

    keywords
}

fn score_chunk_for_keywords(chunk: &DocumentChunk, keywords: &[String]) -> usize {
    let label = chunk
        .label
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let context_label = chunk
        .context_label
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let content = chunk.content.to_ascii_lowercase();
    let location = chunk
        .location_display
        .clone()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let locator = chunk
        .locator
        .clone()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let language = chunk
        .language
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let keyword_blob = chunk.keywords.join(" ").to_ascii_lowercase();

    keywords.iter().fold(0usize, |score, keyword| {
        let mut next = score;
        if label.contains(keyword) {
            next += 6;
        }
        if context_label.contains(keyword) {
            next += 5;
        }
        if location.contains(keyword) {
            next += 4;
        }
        if locator.contains(keyword) {
            next += 3;
        }
        if keyword_blob.contains(keyword) {
            next += 3;
        }
        if content.contains(keyword) {
            next += 2;
        }
        if language == keyword.as_str() {
            next += 1;
        }
        next
    })
}

fn build_hierarchical_document_chunks(document: &ParsedDocument) -> Vec<DocumentChunk> {
    const TARGET_CHARS: usize = 1200;
    const MAX_CHARS: usize = 1800;

    let mut chunks = Vec::new();
    let mut index = 0usize;

    while index < document.blocks.len() {
        let block = &document.blocks[index];
        if should_stay_atomic(block) {
            let context_label = inherited_heading_label(&document.blocks[..index]);
            chunks.push(build_contextual_atomic_chunk(index, block, context_label));
            index += 1;
            continue;
        }

        let start = index;
        let mut end = index + 1;
        let mut char_count = block.content.chars().count();
        let inherited_label = inherited_heading_label(&document.blocks[..start]);
        let mut label = block.label.clone().or(inherited_label);

        if block.kind == DocumentBlockKind::Heading {
            label = Some(
                block
                    .label
                    .clone()
                    .unwrap_or_else(|| block.content.trim().to_string()),
            );
        }

        while end < document.blocks.len() {
            let next = &document.blocks[end];
            if next.kind == DocumentBlockKind::Heading {
                break;
            }
            if should_stay_atomic(next) {
                break;
            }
            let next_len = next.content.chars().count();
            if char_count >= TARGET_CHARS && next.kind != DocumentBlockKind::Paragraph {
                break;
            }
            if char_count + next_len > MAX_CHARS {
                break;
            }
            char_count += next_len;
            end += 1;
        }

        let slice = &document.blocks[start..end];
        chunks.push(build_aggregate_chunk(slice, start, label));
        index = end;
    }

    chunks
}

fn build_aggregate_chunk(
    blocks: &[DocumentBlock],
    start_index: usize,
    preferred_label: Option<String>,
) -> DocumentChunk {
    let content = blocks
        .iter()
        .map(|block| block.content.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let first = &blocks[0];
    let first_kind = blocks
        .iter()
        .find(|block| block.kind != DocumentBlockKind::Metadata)
        .map(|block| block.kind.clone())
        .unwrap_or_else(|| first.kind.clone());
    let label = preferred_label
        .filter(|value| !value.trim().is_empty())
        .or_else(|| first.label.clone());
    let context_label = label.clone();

    let mut location_display = first
        .location
        .as_ref()
        .map(crate::document_render::format_block_location)
        .filter(|value| !value.is_empty());
    if location_display.is_none() {
        location_display = label.clone();
    }

    let keywords = extract_keywords_from_parts(
        blocks.iter().flat_map(|block| {
            [
                block.label.as_deref().unwrap_or_default(),
                block.content.as_str(),
                block
                    .attributes
                    .get("keywords")
                    .map(String::as_str)
                    .unwrap_or_default(),
            ]
        }),
        10,
    );
    let language = blocks.iter().find_map(|block| {
        block
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.language.clone())
            .or_else(|| block.attributes.get("language").cloned())
    });

    DocumentChunk {
        label: label.clone(),
        context_label,
        content,
        keywords,
        language,
        location: first.location.clone(),
        location_display,
        locator: crate::document_render::derive_locator_from_location_and_label(
            first.location.as_ref(),
            label.as_deref(),
        ),
        source: first
            .location
            .as_ref()
            .and_then(|location| location.source.clone()),
        page: first.location.as_ref().and_then(|location| location.page),
        ordinal: first
            .location
            .as_ref()
            .and_then(|location| location.ordinal),
        block_indices: (start_index..start_index + blocks.len()).collect(),
        kind: Some(first_kind),
        score: 0,
    }
}

fn build_contextual_atomic_chunk(
    idx: usize,
    block: &DocumentBlock,
    context_label: Option<String>,
) -> DocumentChunk {
    let mut chunk = build_default_document_chunk(idx, block);
    let block_label = chunk.label.clone();
    let merged_label =
        merge_heading_context_label(context_label.as_deref(), block_label.as_deref());
    if let Some(label) = merged_label {
        chunk.label = Some(label.clone());
        if chunk.location_display.is_none() {
            chunk.location_display = Some(label.clone());
        }
        chunk.locator = crate::document_render::derive_locator_from_location_and_label(
            chunk.location.as_ref(),
            Some(&label),
        );
    }
    chunk.context_label = context_label.clone();
    if chunk.keywords.is_empty() {
        chunk.keywords = extract_keywords_from_parts(
            [
                context_label.as_deref().unwrap_or_default(),
                block.label.as_deref().unwrap_or_default(),
                block.content.as_str(),
            ],
            8,
        );
    }
    chunk
}

fn should_stay_atomic(block: &DocumentBlock) -> bool {
    matches!(
        block.kind,
        DocumentBlockKind::Table
            | DocumentBlockKind::Code
            | DocumentBlockKind::Metadata
            | DocumentBlockKind::EmailHeader
            | DocumentBlockKind::Slide
    )
}

fn inherited_heading_label(blocks: &[DocumentBlock]) -> Option<String> {
    blocks.iter().rev().find_map(|block| {
        (block.kind == DocumentBlockKind::Heading)
            .then(|| {
                block
                    .label
                    .clone()
                    .unwrap_or_else(|| block.content.trim().to_string())
            })
            .filter(|value| !value.trim().is_empty())
    })
}

fn merge_heading_context_label(context: Option<&str>, label: Option<&str>) -> Option<String> {
    let context = context.map(str::trim).filter(|value| !value.is_empty());
    let label = label.map(str::trim).filter(|value| !value.is_empty());
    match (context, label) {
        (Some(context), Some(label)) if context != label => Some(format!("{context} > {label}")),
        (Some(context), Some(_)) => Some(context.to_string()),
        (Some(context), None) => Some(context.to_string()),
        (None, Some(label)) => Some(label.to_string()),
        (None, None) => None,
    }
}

fn split_keyword_attribute(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn infer_document_language(document: &ParsedDocument) -> Option<String> {
    let mut sample = String::new();
    if let Some(title) = &document.title {
        sample.push_str(title);
        sample.push('\n');
    }
    for block in &document.blocks {
        if sample.len() > 4000 {
            break;
        }
        if !block.content.trim().is_empty() {
            sample.push_str(&block.content);
            sample.push('\n');
        }
    }
    infer_language_from_text(&sample)
}

fn infer_language_from_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    let cjk = text.chars().filter(|ch| is_cjk_char(*ch)).count();
    let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    let digits = text.chars().filter(|ch| ch.is_ascii_digit()).count();
    let alpha = cjk + latin;
    if alpha == 0 && digits == 0 {
        return None;
    }

    if cjk > latin {
        return Some("zh".to_string());
    }
    if latin > 0 {
        return Some("en".to_string());
    }
    None
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0xF900..=0xFAFF
    )
}

fn extract_document_keywords(document: &ParsedDocument, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(title) = &document.title {
        parts.push(title.as_str());
    }
    for block in &document.blocks {
        if let Some(label) = &block.label {
            parts.push(label.as_str());
        }
        parts.push(block.content.as_str());
    }
    extract_keywords_from_parts(parts, limit)
}

fn extract_keywords_from_parts<'a>(
    parts: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<String> {
    let mut scores: BTreeMap<String, usize> = BTreeMap::new();
    let mut order = Vec::new();
    let mut seen = HashSet::new();

    for part in parts {
        for token in tokenize_keywords(part) {
            let weight = token.chars().count().min(12);
            *scores.entry(token.clone()).or_insert(0) += weight;
            if seen.insert(token.clone()) {
                order.push(token);
            }
        }
    }

    let mut ranked = order
        .into_iter()
        .map(|token| {
            let score = scores.get(&token).copied().unwrap_or(0);
            (token, score)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(token, _)| token)
        .collect()
}

fn tokenize_keywords(text: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the",
        "and",
        "for",
        "with",
        "this",
        "that",
        "from",
        "into",
        "have",
        "will",
        "your",
        "you",
        "are",
        "was",
        "were",
        "has",
        "had",
        "but",
        "not",
        "use",
        "using",
        "used",
        "its",
        "our",
        "their",
        "his",
        "her",
        "them",
        "they",
        "can",
        "may",
        "should",
        "could",
        "would",
        "about",
        "after",
        "before",
        "when",
        "where",
        "which",
        "while",
        "also",
        "than",
        "then",
        "there",
        "here",
        "what",
        "how",
        "why",
        "who",
        "whose",
        "been",
        "being",
        "over",
        "under",
        "more",
        "most",
        "some",
        "such",
        "other",
        "only",
        "same",
        "each",
        "any",
        "all",
        "per",
        "via",
        "our",
        "document",
        "worksheet",
        "table",
        "sheet",
        "page",
    ];
    let mut out = Vec::new();
    for token in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')) {
        let normalized = token.trim_matches('_').to_ascii_lowercase();
        if normalized.len() < 3 || STOPWORDS.contains(&normalized.as_str()) {
            continue;
        }
        out.push(normalized);
    }
    out
}

fn looks_garbled(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let weird = trimmed
        .chars()
        .filter(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        .count();
    let replacement = trimmed.chars().filter(|ch| *ch == '\u{fffd}').count();
    let punctuation_run = trimmed
        .chars()
        .filter(|ch| matches!(ch, '�' | '□' | '■'))
        .count();
    weird + replacement + punctuation_run > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn filesystem_document_cache_roundtrips_documents() {
        let dir = TempDir::new().unwrap();
        let cache = FileSystemDocumentCache::new(dir.path().join("doc-cache"));
        let key = DocumentCacheKey {
            path: "report.pdf".to_string(),
            file_hash: "abc123".to_string(),
            parser: "composite-document-parser".to_string(),
            pipeline_signature: "cache=filesystem".to_string(),
        };
        let document = ParsedDocument::from_text("cached content");

        cache.put_document(&key, &document).unwrap();
        let restored = cache.get_document(&key).unwrap().unwrap();

        assert_eq!(restored, document);
    }

    #[test]
    fn filesystem_document_cache_roundtrips_ocr_payloads() {
        let dir = TempDir::new().unwrap();
        let cache = FileSystemDocumentCache::new(dir.path().join("doc-cache"));
        let key = DocumentOcrCacheKey {
            path: "scan.png".to_string(),
            file_hash: "hash123".to_string(),
            format: "image".to_string(),
            provider: "test-ocr".to_string(),
            ocr_signature: "model=x;prompt=unset".to_string(),
        };

        cache.put_ocr_payload(&key, "{\"text\":\"hello\"}").unwrap();
        let restored = cache.get_ocr_payload(&key).unwrap().unwrap();

        assert_eq!(restored, "{\"text\":\"hello\"}");
    }

    #[test]
    fn default_pipeline_uses_filesystem_cache_when_directory_is_configured() {
        let dir = TempDir::new().unwrap();
        let registry = build_default_document_pipeline_registry_for_config(&DocumentParserConfig {
            cache: Some(crate::config::DocumentCacheConfig {
                enabled: true,
                directory: Some(dir.path().join("configured-cache")),
            }),
            ..DocumentParserConfig::default()
        });

        assert_eq!(
            registry.cache_store().map(|store| store.name()),
            Some("filesystem-document-cache")
        );
    }

    #[test]
    fn chunk_document_uses_cache_after_first_chunk() {
        #[derive(Default)]
        struct CountingChunker {
            calls: Mutex<usize>,
        }

        impl DocumentChunker for CountingChunker {
            fn name(&self) -> &str {
                "counting-chunker"
            }

            fn chunk(
                &self,
                _path: &Path,
                document: &ParsedDocument,
                _query: Option<&str>,
            ) -> anyhow::Result<Vec<DocumentChunk>> {
                *self.calls.lock().unwrap() += 1;
                Ok(build_default_document_chunks(document))
            }
        }

        let dir = TempDir::new().unwrap();
        let mut registry = DocumentPipelineRegistry::empty();
        registry.register_cache_store(Arc::new(InMemoryDocumentCache::default()));
        let chunker = Arc::new(CountingChunker::default());
        registry.register_chunker(chunker.clone());

        let doc = ParsedDocument::from_text("cached chunk content");
        let path = dir.path().join("cached.txt");

        let first = registry
            .chunk_document(&path, &doc, Some("cached"))
            .unwrap();
        let second = registry
            .chunk_document(&path, &doc, Some("cached"))
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(*chunker.calls.lock().unwrap(), 1);
    }

    #[test]
    fn quality_evaluation_uses_cache_after_first_evaluation() {
        #[derive(Default)]
        struct CountingEvaluator {
            calls: Mutex<usize>,
        }

        impl DocumentQualityEvaluator for CountingEvaluator {
            fn name(&self) -> &str {
                "counting-quality-evaluator"
            }

            fn evaluate(
                &self,
                _path: &Path,
                _document: &ParsedDocument,
                _validation_report: &crate::document_pipeline::DocumentValidationReport,
            ) -> anyhow::Result<DocumentQualityReport> {
                *self.calls.lock().unwrap() += 1;
                Ok(DocumentQualityReport {
                    score: 88,
                    grade: DocumentQualityGrade::Good,
                    issues: Vec::new(),
                    metrics: serde_json::json!({ "cached": true }),
                })
            }
        }

        let dir = TempDir::new().unwrap();
        let mut registry = DocumentPipelineRegistry::empty();
        registry.register_cache_store(Arc::new(InMemoryDocumentCache::default()));
        let evaluator = Arc::new(CountingEvaluator::default());
        registry.register_quality_evaluator(evaluator.clone());

        let doc = ParsedDocument::from_text("cached quality content");
        let report = crate::document_pipeline::DocumentValidationReport::default();
        let path = dir.path().join("cached.txt");

        let first = registry
            .evaluate_document_quality(&path, &doc, &report)
            .unwrap()
            .unwrap();
        let second = registry
            .evaluate_document_quality(&path, &doc, &report)
            .unwrap()
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(*evaluator.calls.lock().unwrap(), 1);
    }

    #[test]
    fn default_pipeline_enriches_language_and_keywords() {
        let registry = build_default_document_pipeline_registry();
        let path = Path::new("report.md");
        let mut document = ParsedDocument {
            title: Some("Quarterly Revenue Analysis".to_string()),
            blocks: vec![
                DocumentBlock::new(
                    DocumentBlockKind::Heading,
                    Some("Overview"),
                    "Revenue growth accelerated across enterprise regions.",
                ),
                DocumentBlock::new(
                    DocumentBlockKind::Paragraph,
                    None::<String>,
                    "Forecast confidence improved after margin review.",
                ),
            ],
            metadata: None,
            ..Default::default()
        };

        let report = registry.process_document(path, &mut document).unwrap();

        assert!(report.issues.is_empty());
        let metadata = document.metadata.unwrap();
        assert_eq!(metadata.language.as_deref(), Some("en"));
        assert!(metadata
            .attributes
            .get("document.keywords")
            .is_some_and(|keywords| keywords.contains("revenue")));
        assert!(document.blocks.iter().all(|block| {
            block.attributes.contains_key("keywords") && block.attributes.contains_key("language")
        }));
    }

    #[test]
    fn hierarchical_chunker_groups_heading_with_following_content() {
        let chunker = HierarchicalDocumentChunker;
        let document = ParsedDocument {
            title: Some("Doc".to_string()),
            blocks: vec![
                DocumentBlock::new(DocumentBlockKind::Heading, None::<String>, "Summary"),
                DocumentBlock::new(
                    DocumentBlockKind::Paragraph,
                    None::<String>,
                    "Revenue increased across core markets.",
                ),
                DocumentBlock::new(
                    DocumentBlockKind::Paragraph,
                    None::<String>,
                    "Margin improved after pricing changes.",
                ),
                DocumentBlock::new(
                    DocumentBlockKind::Table,
                    Some("Data Table"),
                    "Region\tScore\nAPAC\t42",
                ),
            ],
            metadata: None,
            ..Default::default()
        };

        let chunks = chunker
            .chunk(Path::new("report.md"), &document, None)
            .unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].label.as_deref(), Some("Summary"));
        assert_eq!(chunks[0].context_label.as_deref(), Some("Summary"));
        assert!(chunks[0].keywords.iter().any(|kw| kw == "revenue"));
        assert!(chunks[0].content.contains("Revenue increased"));
        assert!(chunks[0].content.contains("Margin improved"));
        assert_eq!(chunks[0].block_indices, vec![0, 1, 2]);
        assert_eq!(chunks[1].label.as_deref(), Some("Summary > Data Table"));
        assert_eq!(chunks[1].context_label.as_deref(), Some("Summary"));
    }

    #[test]
    fn default_quality_evaluator_flags_garbled_ocr_heavy_documents() {
        let evaluator = DefaultDocumentQualityEvaluator;
        let document = ParsedDocument {
            title: Some("scan".to_string()),
            blocks: vec![
                DocumentBlock::new(DocumentBlockKind::Paragraph, None::<String>, "� � �")
                    .with_attribute("ocr_page", "1"),
                DocumentBlock::new(DocumentBlockKind::Paragraph, None::<String>, "short")
                    .with_attribute("ocr_page", "2"),
            ],
            metadata: Some(DocumentMetadata {
                language: Some("en".to_string()),
                ..DocumentMetadata::default()
            }),
            ..Default::default()
        };
        let validation = crate::document_pipeline::DocumentValidationReport {
            issues: vec![DocumentValidationIssue::warning(
                "content-presence-validator",
                "weak content",
            )],
        };

        let report = evaluator
            .evaluate(Path::new("scan.pdf"), &document, &validation)
            .unwrap();

        assert!(report.score < 80);
        assert!(!report.issues.is_empty());
        assert_eq!(report.metrics["ocr_block_count"], 2);
        assert_eq!(report.metrics["document_language"], "en");
    }

    #[test]
    fn hierarchical_chunker_scores_atomic_chunks_with_heading_context() {
        let chunker = HierarchicalDocumentChunker;
        let document = ParsedDocument {
            title: Some("Doc".to_string()),
            blocks: vec![
                DocumentBlock::new(
                    DocumentBlockKind::Heading,
                    None::<String>,
                    "Security Review",
                ),
                DocumentBlock::new(
                    DocumentBlockKind::Table,
                    Some("Findings"),
                    "Issue\tSeverity\nAuth\tHigh",
                ),
                DocumentBlock::new(
                    DocumentBlockKind::Paragraph,
                    None::<String>,
                    "Remediation summary.",
                ),
            ],
            metadata: None,
            ..Default::default()
        };

        let chunks = chunker
            .chunk(Path::new("review.md"), &document, Some("security"))
            .unwrap();

        let findings_chunk = chunks
            .iter()
            .find(|chunk| chunk.label.as_deref() == Some("Security Review > Findings"))
            .unwrap();
        assert!(findings_chunk.score > 0);
        assert_eq!(
            findings_chunk.context_label.as_deref(),
            Some("Security Review")
        );
        assert!(findings_chunk.keywords.iter().any(|kw| kw == "security"));
    }
}
