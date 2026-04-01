#[path = "consume_parse.rs"]
mod parse;
#[path = "consume_search.rs"]
mod search;

#[cfg(test)]
pub(crate) use parse::parse_document_from_path;
#[allow(unused_imports)]
pub(crate) use parse::{
    build_document_chunks, build_parse_file_not_found_output, build_parse_llm_failure_message,
    build_parse_llm_request_for_prepared, build_parse_tool_output_from_prepared,
    build_parse_unreadable_output, extract_document_runtime_metadata,
    load_extracted_document_from_path, prepare_document_from_path,
    prepare_parse_document_from_path, PreparedParseDocument, StructuralSummaryStyle,
};

#[cfg(test)]
pub(crate) use parse::{
    build_parse_llm_request, build_parse_tool_output, build_structural_summary,
    describe_document_parser_config, llm_block_metadata, render_document_for_llm,
    summarize_document_runtime, ParseLlmRequest, ParseResultInput,
};
#[allow(unused_imports)]
pub(crate) use search::{
    build_deep_evidence_blocks, build_deep_search_response, build_deep_search_results_metadata,
    build_deep_search_results_report, build_deep_search_tool_output, build_fast_search_response,
    build_filename_search_no_results_output, build_filename_search_response,
    build_match_line_metadata_values, build_match_lines_metadata,
    build_sampled_line_metadata_values, build_sampled_lines_metadata,
    build_search_document_substrate, build_search_line_entries_from_chunks, build_search_lines,
    build_search_no_results_output, build_search_results_metadata, build_search_results_report,
    build_search_score_metadata, build_search_tool_output, clone_search_match_lines,
    clone_search_score_metadata, extract_search_keywords, prepare_search_document_substrate,
    rank_search_sampling_document, sample_deep_search_regions, DeepEvidenceBlockRenderInput,
    DeepSampledLineRenderInput, DeepSearchDocumentRegion, DeepSearchResponseInput,
    DeepSearchResultMetadataInput, DeepSearchResultRenderInput, DeepSearchSampledLine,
    DeepSearchSamplingDocument, MatchLineMetadataInput, SampledLineMetadataInput,
    SearchDocumentMatch, SearchDocumentMatchLine, SearchDocumentMetadata, SearchDocumentSubstrate,
    SearchFileType, SearchMatchRenderInput, SearchResponseInput, SearchResultMetadataInput,
    SearchResultRenderInput, SearchScoreMetadataInput,
};

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use parse::{
    build_parse_result, BuiltParseResult, ParseOutputHeader, ParseReportSections,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_parser::{
        DocumentBlock, DocumentBlockKind, DocumentParser, DocumentParserRegistry, ParsedDocument,
    };
    use crate::document_pipeline::{
        DocumentCacheKey, DocumentCacheStore, DocumentExtractionCacheKey, DocumentPipelineRegistry,
        ExtractedDocument,
    };
    use serde_json::json;
    use std::io::Write;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    fn write_temp(dir: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content).unwrap();
        path
    }

    fn block_kind_label(kind: &DocumentBlockKind) -> &'static str {
        match kind {
            DocumentBlockKind::Paragraph => "paragraph",
            DocumentBlockKind::Heading => "heading",
            DocumentBlockKind::Table => "table",
            DocumentBlockKind::Section => "section",
            DocumentBlockKind::Metadata => "metadata",
            DocumentBlockKind::Slide => "slide",
            DocumentBlockKind::EmailHeader => "email_header",
            DocumentBlockKind::Code => "code",
            DocumentBlockKind::Raw => "raw",
        }
    }

    struct MockDocParser;

    impl DocumentParser for MockDocParser {
        fn name(&self) -> &str {
            "mock-doc"
        }

        fn supported_extensions(&self) -> &[&str] {
            &["mock"]
        }

        fn parse(&self, _path: &Path) -> anyhow::Result<String> {
            Ok("mock parsed text".to_string())
        }
    }

    #[derive(Default)]
    struct TestDocumentCache {
        extracted_documents: Mutex<std::collections::HashMap<String, ExtractedDocument>>,
        documents: Mutex<std::collections::HashMap<String, ParsedDocument>>,
    }

    impl DocumentCacheStore for TestDocumentCache {
        fn name(&self) -> &str {
            "test-document-cache"
        }

        fn get_extracted_document(
            &self,
            key: &DocumentExtractionCacheKey,
        ) -> anyhow::Result<Option<ExtractedDocument>> {
            Ok(self
                .extracted_documents
                .lock()
                .unwrap()
                .get(&format!("{}|{}|{}", key.path, key.file_hash, key.parser))
                .cloned())
        }

        fn put_extracted_document(
            &self,
            key: &DocumentExtractionCacheKey,
            document: &ExtractedDocument,
        ) -> anyhow::Result<()> {
            self.extracted_documents.lock().unwrap().insert(
                format!("{}|{}|{}", key.path, key.file_hash, key.parser),
                document.clone(),
            );
            Ok(())
        }

        fn get_document(&self, key: &DocumentCacheKey) -> anyhow::Result<Option<ParsedDocument>> {
            Ok(self
                .documents
                .lock()
                .unwrap()
                .get(&format!(
                    "{}|{}|{}|{}",
                    key.path, key.file_hash, key.parser, key.pipeline_signature
                ))
                .cloned())
        }

        fn put_document(
            &self,
            key: &DocumentCacheKey,
            document: &ParsedDocument,
        ) -> anyhow::Result<()> {
            self.documents.lock().unwrap().insert(
                format!(
                    "{}|{}|{}|{}",
                    key.path, key.file_hash, key.parser, key.pipeline_signature
                ),
                document.clone(),
            );
            Ok(())
        }
    }

    #[test]
    fn parse_document_from_path_uses_registry_when_available() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "sample.mock", b"ignored");
        let mut registry = DocumentParserRegistry::empty();
        registry.register(Arc::new(MockDocParser));

        let parsed = parse_document_from_path(&path, Some(&registry), None, None).unwrap();
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap().to_text(), "mock parsed text");
    }

    #[test]
    fn load_extracted_document_from_path_uses_registry_when_available() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "sample.mock", b"ignored");
        let mut registry = DocumentParserRegistry::empty();
        registry.register(Arc::new(MockDocParser));

        let extracted =
            load_extracted_document_from_path(&path, Some(&registry), None, None).unwrap();
        assert!(extracted.is_some());
        assert_eq!(
            extracted.unwrap().into_parsed_document().to_text(),
            "mock parsed text"
        );
    }

    #[test]
    fn parse_document_from_path_falls_back_to_plain_text_reads() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "notes.bin", b"plain text fallback");

        let parsed = parse_document_from_path(&path, None, None, None).unwrap();
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap().to_text(), "plain text fallback");
    }

    #[test]
    fn parse_document_from_path_returns_none_for_unreadable_binary_without_parser() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "image.bin", &[0, 159, 146, 150]);

        let parsed = parse_document_from_path(&path, None, None, None).unwrap();
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_document_from_path_respects_plaintext_size_limit() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "large.txt", b"abcdefghijklmnopqrstuvwxyz");

        let parsed = parse_document_from_path(&path, None, None, Some(8)).unwrap();
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_document_from_path_uses_pipeline_cache_after_first_parse() {
        struct CountingParser {
            calls: Arc<Mutex<usize>>,
        }

        impl DocumentParser for CountingParser {
            fn name(&self) -> &str {
                "counting"
            }

            fn supported_extensions(&self) -> &[&str] {
                &["count"]
            }

            fn parse(&self, _path: &Path) -> anyhow::Result<String> {
                *self.calls.lock().unwrap() += 1;
                Ok("cached parse result".to_string())
            }
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "sample.count", b"ignored");
        let calls = Arc::new(Mutex::new(0usize));

        let mut registry = DocumentParserRegistry::empty();
        registry.register(Arc::new(CountingParser {
            calls: Arc::clone(&calls),
        }));

        let mut pipeline = DocumentPipelineRegistry::empty();
        pipeline.register_cache_store(Arc::new(TestDocumentCache::default()));

        let first = parse_document_from_path(&path, Some(&registry), Some(&pipeline), None)
            .unwrap()
            .unwrap();
        let second = parse_document_from_path(&path, Some(&registry), Some(&pipeline), None)
            .unwrap()
            .unwrap();

        assert_eq!(first.to_text(), "cached parse result");
        assert_eq!(second.to_text(), "cached parse result");
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn parse_document_from_path_reuses_raw_extraction_across_pipeline_variants() {
        struct CountingParser {
            calls: Arc<Mutex<usize>>,
        }

        impl DocumentParser for CountingParser {
            fn name(&self) -> &str {
                "counting"
            }

            fn supported_extensions(&self) -> &[&str] {
                &["count"]
            }

            fn parse(&self, _path: &Path) -> anyhow::Result<String> {
                *self.calls.lock().unwrap() += 1;
                Ok("raw parser result".to_string())
            }
        }

        struct AppendProcessor(&'static str);

        impl crate::document_pipeline::DocumentPostProcessor for AppendProcessor {
            fn name(&self) -> &str {
                self.0
            }

            fn process(&self, _path: &Path, document: &mut ParsedDocument) -> anyhow::Result<()> {
                if let Some(first) = document.blocks.first_mut() {
                    first.content.push(' ');
                    first.content.push_str(self.0);
                }
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "sample.count", b"ignored");
        let calls = Arc::new(Mutex::new(0usize));
        let cache = Arc::new(TestDocumentCache::default());

        let mut registry = DocumentParserRegistry::empty();
        registry.register(Arc::new(CountingParser {
            calls: Arc::clone(&calls),
        }));

        let mut pipeline_a = DocumentPipelineRegistry::empty();
        pipeline_a.register_cache_store(cache.clone());
        pipeline_a.register_post_processor(Arc::new(AppendProcessor("post-a")));

        let mut pipeline_b = DocumentPipelineRegistry::empty();
        pipeline_b.register_cache_store(cache);
        pipeline_b.register_post_processor(Arc::new(AppendProcessor("post-b")));

        let first = parse_document_from_path(&path, Some(&registry), Some(&pipeline_a), None)
            .unwrap()
            .unwrap();
        let second = parse_document_from_path(&path, Some(&registry), Some(&pipeline_b), None)
            .unwrap()
            .unwrap();

        assert_eq!(first.to_text(), "raw parser result post-a");
        assert_eq!(second.to_text(), "raw parser result post-b");
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn parse_document_from_path_separates_extracted_and_normalized_stage_metadata() {
        struct MetadataParser;

        impl DocumentParser for MetadataParser {
            fn name(&self) -> &str {
                "metadata-parser"
            }

            fn supported_extensions(&self) -> &[&str] {
                &["meta"]
            }

            fn parse_extracted(
                &self,
                _path: &Path,
            ) -> anyhow::Result<crate::document_pipeline::ExtractedDocument> {
                let mut doc = ParsedDocument::from_text("hello");
                doc.metadata = Some(crate::document_parser::DocumentMetadata {
                    detected_file_type: Some("meta".to_string()),
                    provenance: Some(crate::document_parser::DocumentProvenance {
                        extractor: Some("unit-extractor".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                Ok(crate::document_pipeline::ExtractedDocument::new(doc))
            }

            fn parse(&self, _path: &Path) -> anyhow::Result<String> {
                unreachable!("parse_extracted is used in this test")
            }
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "sample.meta", b"ignored");
        let cache = Arc::new(TestDocumentCache::default());

        let mut registry = DocumentParserRegistry::empty();
        registry.register(Arc::new(MetadataParser));

        let mut pipeline =
            crate::document_pipeline_defaults::build_default_document_pipeline_registry();
        pipeline.register_cache_store(cache.clone());

        let parsed = parse_document_from_path(&path, Some(&registry), Some(&pipeline), None)
            .unwrap()
            .unwrap();

        let extracted = cache
            .extracted_documents
            .lock()
            .unwrap()
            .values()
            .next()
            .cloned()
            .expect("extracted document cached");

        assert_eq!(
            extracted
                .as_parsed_document()
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.attributes.get("document.stage"))
                .map(String::as_str),
            None
        );
        assert_eq!(
            extracted
                .extraction_metadata
                .as_ref()
                .and_then(|metadata| metadata.extractor.as_deref()),
            Some("unit-extractor")
        );
        assert_eq!(
            parsed
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.attributes.get("document.stage"))
                .map(String::as_str),
            Some("normalized")
        );
    }

    #[test]
    fn parse_document_from_path_invalidates_raw_cache_when_parser_signature_changes() {
        struct VersionedParser {
            calls: Arc<Mutex<usize>>,
            version: &'static str,
            value: &'static str,
        }

        impl DocumentParser for VersionedParser {
            fn name(&self) -> &str {
                "versioned"
            }

            fn signature(&self) -> String {
                format!("{}@{}", self.name(), self.version)
            }

            fn supported_extensions(&self) -> &[&str] {
                &["count"]
            }

            fn parse(&self, _path: &Path) -> anyhow::Result<String> {
                *self.calls.lock().unwrap() += 1;
                Ok(self.value.to_string())
            }
        }

        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "sample.count", b"ignored");
        let calls = Arc::new(Mutex::new(0usize));
        let cache = Arc::new(TestDocumentCache::default());

        let mut registry_v1 = DocumentParserRegistry::empty();
        registry_v1.register(Arc::new(VersionedParser {
            calls: Arc::clone(&calls),
            version: "v1",
            value: "parser output v1",
        }));

        let mut registry_v2 = DocumentParserRegistry::empty();
        registry_v2.register(Arc::new(VersionedParser {
            calls: Arc::clone(&calls),
            version: "v2",
            value: "parser output v2",
        }));

        let mut pipeline = DocumentPipelineRegistry::empty();
        pipeline.register_cache_store(cache);

        let first = parse_document_from_path(&path, Some(&registry_v1), Some(&pipeline), None)
            .unwrap()
            .unwrap();
        let second = parse_document_from_path(&path, Some(&registry_v2), Some(&pipeline), None)
            .unwrap()
            .unwrap();

        assert_eq!(first.to_text(), "parser output v1");
        assert_eq!(second.to_text(), "parser output v2");
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    #[test]
    fn build_search_lines_includes_locations_labels_and_content() {
        let doc = ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![
                DocumentBlock::new(
                    DocumentBlockKind::Section,
                    Some("Overview"),
                    "Line one\nLine two",
                )
                .with_source("report.pdf")
                .with_page(2)
                .with_ordinal(4),
                DocumentBlock::new(DocumentBlockKind::EmailHeader, Some("Subject"), "Hello"),
            ],
            metadata: None,
            ..Default::default()
        };

        let lines = build_search_lines(&doc);
        assert_eq!(lines[0], "# report.pdf");
        assert!(lines
            .iter()
            .any(|line| line == "[loc] source=report.pdf, page=2, ordinal=4"));
        assert!(lines.iter().any(|line| line == "[section] Overview"));
        assert!(lines.iter().any(|line| line == "[email] Subject"));
        assert!(lines.iter().any(|line| line == "Line one"));
        assert!(lines.iter().any(|line| line == "Hello"));
    }

    #[test]
    fn llm_block_metadata_includes_location_display() {
        let doc = ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![DocumentBlock::new(
                DocumentBlockKind::Section,
                Some("page 2: 1. Overview"),
                "hello world",
            )
            .with_source("report.pdf")
            .with_page(2)
            .with_ordinal(4)],
            metadata: None,
            ..Default::default()
        };

        let metadata = llm_block_metadata(&doc, &[0], block_kind_label);
        assert_eq!(metadata[0]["index"], json!(1));
        assert_eq!(metadata[0]["kind"], json!("section"));
        assert_eq!(metadata[0]["label"], json!("page 2: 1. Overview"));
        assert_eq!(
            metadata[0]["location"]["display"],
            json!("source=report.pdf, page=2, ordinal=4")
        );
    }

    #[test]
    fn build_parse_llm_request_includes_query_and_document_context() {
        let request = build_parse_llm_request(&ParseLlmRequest {
            path_display: "report.pdf",
            strategy_label: "narrative",
            rendered_content: "# Document: report.pdf\n\n## Block 1: section\nhello",
            query: "What are the findings?",
            max_chars: 8000,
            llm_input_truncated: false,
            llm_blocks_included: 1,
        });

        assert!(request
            .system_prompt
            .contains("document analysis assistant"));
        assert!(request.user_prompt.contains("Document: `report.pdf`"));
        assert!(request.user_prompt.contains("Parse strategy: narrative"));
        assert!(request
            .user_prompt
            .contains("Query: What are the findings?"));
        assert!(!request.user_prompt.contains("LLM input was truncated"));
    }

    #[test]
    fn build_parse_llm_request_includes_truncation_note_when_needed() {
        let request = build_parse_llm_request(&ParseLlmRequest {
            path_display: "report.pdf",
            strategy_label: "tabular",
            rendered_content: "content",
            query: "summarize",
            max_chars: 1200,
            llm_input_truncated: true,
            llm_blocks_included: 3,
        });

        assert!(request
            .user_prompt
            .contains("LLM input was truncated to 1200 chars across 3 block(s)."));
    }

    #[test]
    fn build_parse_result_includes_report_and_metadata() {
        let input = ParseResultInput {
            path_display: "report.pdf",
            strategy_label: "narrative",
            document_parser_config: Some(&crate::config::DocumentParserConfig {
                enabled: true,
                max_file_size_mb: 64,
                ocr: Some(crate::config::DocumentOcrConfig {
                    enabled: true,
                    model: Some("moonshot/kimi-vl".to_string()),
                    prompt: Some("Read scan".to_string()),
                    max_images: 4,
                    dpi: 180,
                }),
                ..Default::default()
            }),
            document_runtime: Some(&json!({
                "ocr": {
                    "used": true,
                    "format": "pdf",
                    "provider": "mock-ocr",
                    "model": "moonshot/kimi-vl",
                    "max_images": 4,
                    "dpi": 180
                }
            })),
            document_quality: Some(&crate::document_pipeline::DocumentQualityReport {
                score: 82,
                grade: crate::document_pipeline::DocumentQualityGrade::Good,
                issues: vec![crate::document_pipeline::DocumentQualityIssue {
                    code: "content.ocr_dependent".to_string(),
                    message: "1 blocks depend on OCR".to_string(),
                }],
                metrics: json!({
                    "ocr_block_count": 1
                }),
            }),
            block_count: 3,
            non_empty_block_count: 2,
            line_count: 20,
            word_count: 100,
            document_language: Some("en"),
            document_keywords: &["overview".to_string(), "finding".to_string()],
            document_provenance: Some(&json!({
                "parser": "mock-parser",
                "extractor": "mock-extractor",
                "provider": "mock-provider"
            })),
            document_confidence: Some(&json!({
                "score_percent": 93,
                "label": "high"
            })),
            query_used: true,
            llm_input_truncated: false,
            llm_blocks_included: 1,
            llm_block_details: &[json!({
                "index": 1,
                "kind": "section",
                "label": "Overview",
                "location": {
                    "display": "page=2"
                }
            })],
            chunk_highlights: &[json!({
                "label": "Overview",
                "locator": "page=2",
                "language": "en",
                "keywords": ["overview", "finding"],
                "score": 12,
                "preview": "Key finding preview"
            })],
            structured_payloads: &[json!({
                "index": 1,
                "kind": "table",
                "label": "sheet1",
                "payload_summary": "object keys=headers, rows",
                "payload_preview": "{\n  \"headers\": [\"name\", \"value\"]\n}",
                "structured_payload": {
                    "headers": ["name", "value"],
                    "rows": [["foo", "1"]]
                }
            })],
            max_chars: 8000,
            structural_summary: "\n## Structural Summary\n\nOverview\n",
            llm_answer: Some("Key finding"),
            tables: &[],
            pages: &[],
        };

        let built = build_parse_result(&input);
        assert!(built.content.contains("# Agentic Parse: `report.pdf`"));
        assert!(built.content.contains("Language"));
        assert!(built.content.contains("Keywords"));
        assert!(built.content.contains("Provenance"));
        assert!(built.content.contains("Confidence"));
        assert!(built.content.contains("Key Chunks"));
        assert!(built.content.contains("Structured Payloads"));
        assert!(built.content.contains("Key finding preview"));
        assert!(built.content.contains("LLM Blocks"));
        assert!(built.content.contains("Quality"));
        assert!(built.content.contains("Key finding"));
        assert_eq!(built.metadata["file"], json!("report.pdf"));
        assert_eq!(built.metadata["strategy"], json!("narrative"));
        assert_eq!(built.metadata["document_language"], json!("en"));
        assert_eq!(built.metadata["document_keywords"][0], json!("overview"));
        assert_eq!(
            built.metadata["document_provenance"]["parser"],
            json!("mock-parser")
        );
        assert_eq!(
            built.metadata["document_confidence"]["score_percent"],
            json!(93)
        );
        assert_eq!(
            built.metadata["chunk_highlights"][0]["label"],
            json!("Overview")
        );
        assert_eq!(
            built.metadata["document_runtime"]["ocr"]["provider"],
            json!("mock-ocr")
        );
        assert_eq!(built.metadata["document_quality"]["score"], json!(82));
        assert_eq!(built.metadata["llm_blocks_included"], json!(1));
        assert_eq!(
            built.metadata["structured_payloads"][0]["structured_payload"]["headers"][0],
            json!("name")
        );
    }

    #[test]
    fn build_parse_tool_output_wraps_built_parse_result() {
        let input = ParseResultInput {
            path_display: "report.pdf",
            strategy_label: "narrative",
            document_parser_config: None,
            document_runtime: None,
            document_quality: None,
            block_count: 2,
            non_empty_block_count: 2,
            line_count: 10,
            word_count: 20,
            document_language: None,
            document_keywords: &[],
            document_provenance: None,
            document_confidence: None,
            query_used: false,
            llm_input_truncated: false,
            llm_blocks_included: 0,
            llm_block_details: &[],
            chunk_highlights: &[],
            structured_payloads: &[],
            max_chars: 8000,
            structural_summary: "\n## Structural Summary\n\nOverview\n",
            llm_answer: None,
            tables: &[],
            pages: &[],
        };

        let output = build_parse_tool_output(&input);
        assert!(output.success);
        assert!(output.content.contains("# Agentic Parse: `report.pdf`"));
        assert_eq!(
            output.metadata.as_ref().unwrap()["strategy"],
            json!("narrative")
        );
    }

    #[test]
    fn build_search_results_metadata_includes_match_locators() {
        let matches = build_match_lines_metadata(&[MatchLineMetadataInput {
            line_number: 12,
            content: "The parser now emits structured search labels.",
            locator: Some("page 2 | page 2: 1. Overview"),
            context_before: &["[section] page 2: 1. Overview".to_string()],
            context_after: &["Additional supporting text.".to_string()],
        }]);

        let metadata = build_search_results_metadata(&[SearchResultMetadataInput {
            path_display: "docs/report.pdf".to_string(),
            file_type: "docs".to_string(),
            relevance: 1.5,
            score: SearchScoreMetadataInput {
                base_score: 1.0,
                path_signal: 0.2,
                idf_boost: 1.0,
                file_type_boost: 0.9,
                unique_keywords_matched: 1,
            },
            matches,
            document_metadata: Some(json!({
                "title": "report.pdf",
                "stage": "normalized",
                "line_count": 3,
                "chunk_count": 1,
                "has_runtime_metadata": false
            })),
            document_runtime: None,
        }]);

        assert_eq!(
            metadata[0]["matches"][0]["locator"],
            json!("page 2 | page 2: 1. Overview")
        );
        assert_eq!(
            metadata[0]["matches"][0]["context_after"][0],
            json!("Additional supporting text.")
        );
        assert_eq!(
            metadata[0]["document_metadata"]["title"],
            json!("report.pdf")
        );
    }

    #[test]
    fn build_search_results_report_includes_locator_and_context() {
        let report = build_search_results_report(
            &[SearchResultRenderInput {
                path_display: "docs/report.pdf".to_string(),
                file_type: "docs".to_string(),
                relevance: 1.5,
                score: SearchScoreMetadataInput {
                    base_score: 1.0,
                    path_signal: 0.2,
                    idf_boost: 1.0,
                    file_type_boost: 0.9,
                    unique_keywords_matched: 1,
                },
                matches: vec![SearchMatchRenderInput {
                    line_number: 12,
                    content: "The parser now emits structured search labels.".to_string(),
                    locator: Some("page 2 | page 2: 1. Overview".to_string()),
                    context_before: vec!["[section] page 2: 1. Overview".to_string()],
                    context_after: vec![],
                }],
            }],
            "parser labels",
        );

        assert!(report.contains("Found 1 file(s) matching \"parser labels\""));
        assert!(report.contains(
            "▶ L12 [page 2 | page 2: 1. Overview]: The parser now emits structured search labels."
        ));
    }

    #[test]
    fn build_deep_search_results_report_includes_evidence_header() {
        let report = build_deep_search_results_report(
            &[DeepSearchResultRenderInput {
                path_display: "src/auth.rs".to_string(),
                file_type: "code".to_string(),
                evidence_score: 1.234,
                score: SearchScoreMetadataInput {
                    base_score: 1.0,
                    path_signal: 0.3,
                    idf_boost: 1.1,
                    file_type_boost: 1.2,
                    unique_keywords_matched: 2,
                },
                blocks: vec![DeepEvidenceBlockRenderInput {
                    lines: vec![DeepSampledLineRenderInput {
                        line_number: 10,
                        content: "verify_token(token)".to_string(),
                        distance: 0,
                    }],
                }],
            }],
            "token",
        );

        assert!(report.contains("Deep search found 1 evidence region(s) for \"token\""));
        assert!(report.contains("evidence: 1.234"));
        assert!(report.contains("▶ L  10: verify_token(token)"));
    }

    #[test]
    fn build_search_tool_output_includes_fast_metadata_shape() {
        let output = build_search_tool_output(SearchResponseInput {
            keywords: vec!["parser".to_string()],
            result_count: 1,
            report: "Found 1 file(s) matching \"parser\"".to_string(),
            results_metadata: vec![json!({"path": "docs/report.pdf"})],
        });

        assert!(output.success);
        assert_eq!(output.metadata.as_ref().unwrap()["mode"], json!("fast"));
        assert_eq!(output.metadata.as_ref().unwrap()["result_count"], json!(1));
    }

    #[test]
    fn build_deep_search_tool_output_includes_deep_metadata_shape() {
        let output = build_deep_search_tool_output(DeepSearchResponseInput {
            keywords: vec!["token".to_string()],
            result_count: 2,
            initial_pool_size: 4,
            report: "Deep search found 2 evidence region(s) for \"token\"".to_string(),
            results_metadata: vec![json!({"path": "src/auth.rs"})],
        });

        assert!(output.success);
        assert_eq!(output.metadata.as_ref().unwrap()["mode"], json!("deep"));
        assert_eq!(
            output.metadata.as_ref().unwrap()["initial_pool_size"],
            json!(4)
        );
    }
}
