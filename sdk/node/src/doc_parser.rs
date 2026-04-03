//! Document parsing utilities for Node.js SDK
//!
//! Provides standalone parsing functions for document-related types.

use napi::bindgen_prelude::*;

// ============================================================================
// Document Parsing Types
// ============================================================================

/// OCR runtime info from document parsing.
#[napi(object)]
#[derive(Clone)]
pub struct DocumentOcrRuntime {
    pub used: bool,
    pub mode: Option<String>,
    pub format: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_images: Option<u32>,
    pub dpi: Option<u32>,
}

impl DocumentOcrRuntime {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            used: value.get("used").and_then(|v| v.as_bool()).unwrap_or(false),
            mode: value.get("mode").and_then(|v| v.as_str()).map(ToOwned::to_owned),
            format: value.get("format").and_then(|v| v.as_str()).map(ToOwned::to_owned),
            provider: value.get("provider").and_then(|v| v.as_str()).map(ToOwned::to_owned),
            model: value.get("model").and_then(|v| v.as_str()).map(ToOwned::to_owned),
            max_images: value.get("maxImages").and_then(|v| v.as_u64()).map(|v| v as u32),
            dpi: value.get("dpi").and_then(|v| v.as_u64()).map(|v| v as u32),
        }
    }
}

/// Document runtime metadata.
#[napi(object)]
#[derive(Clone)]
pub struct DocumentRuntime {
    pub ocr: Option<DocumentOcrRuntime>,
}

impl DocumentRuntime {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            ocr: value.get("ocr").map(DocumentOcrRuntime::from_json),
        }
    }
}

/// A single match within an agentic search result.
#[napi(object)]
#[derive(Clone)]
pub struct AgenticSearchMatch {
    pub line_number: Option<u32>,
    pub content: Option<String>,
    pub locator: Option<String>,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// A sampled line from an agentic search result.
#[napi(object)]
#[derive(Clone)]
pub struct AgenticSearchSampledLine {
    pub line_number: Option<u32>,
    pub content: Option<String>,
    pub locator: Option<String>,
    pub distance: Option<f64>,
    pub weight: Option<f64>,
}

/// An agentic search result with scoring and matches.
#[napi(object)]
#[derive(Clone)]
pub struct AgenticSearchResult {
    pub path: Option<String>,
    pub file_type: Option<String>,
    pub relevance: Option<f64>,
    pub evidence_score: Option<f64>,
    pub match_count: Option<u32>,
    pub sampled_line_count: Option<u32>,
    pub score: Option<f64>,
    pub matches: Vec<AgenticSearchMatch>,
    pub sampled_lines: Vec<AgenticSearchSampledLine>,
    pub document_runtime: Option<DocumentRuntime>,
}

/// An LLM block parsed from a document.
#[napi(object)]
#[derive(Clone)]
pub struct AgenticParseLlmBlock {
    pub index: Option<u32>,
    pub kind: Option<String>,
    pub label: Option<String>,
    pub location: Option<String>,
}

/// Enriched tool result with parsed metadata.
#[napi(object)]
#[derive(Clone)]
pub struct EnrichedToolResult {
    pub name: String,
    pub output: String,
    pub exit_code: u32,
    pub document_runtime: Option<DocumentRuntime>,
    pub metadata: Option<serde_json::Value>,
    pub agentic_search_results: Option<Vec<AgenticSearchResult>>,
    pub agentic_parse_llm_blocks: Option<Vec<AgenticParseLlmBlock>>,
}

// ============================================================================
// Parsing Functions
// ============================================================================

fn parse_document_runtime_impl(json: &str) -> Result<DocumentRuntime> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| napi::Error::from_reason(format!("Invalid document runtime payload: {}", e)))?;
    Ok(DocumentRuntime::from_json(&value))
}

fn parse_agentic_search_results_impl(json: &str) -> Result<Vec<AgenticSearchResult>> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| napi::Error::from_reason(format!("Invalid tool metadata payload: {}", e)))?;

    let results = value
        .get("results")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let document_runtime = v
                        .get("document_runtime")
                        .map(|dr| DocumentRuntime::from_json(dr));

                    Some(AgenticSearchResult {
                        path: v.get("path").and_then(|p| p.as_str()).map(ToOwned::to_owned),
                        file_type: v.get("file_type").and_then(|p| p.as_str()).map(ToOwned::to_owned),
                        relevance: v.get("relevance").and_then(|p| p.as_f64()),
                        evidence_score: v.get("evidence_score").and_then(|p| p.as_f64()),
                        match_count: v.get("match_count").and_then(|p| p.as_u64()).map(|v| v as u32),
                        sampled_line_count: v.get("sampled_line_count").and_then(|p| p.as_u64()).map(|v| v as u32),
                        score: v.get("score").and_then(|p| p.as_f64()),
                        matches: v
                            .get("matches")
                            .and_then(|m| m.as_array())
                            .map(|matches| {
                                matches.iter().map(|m| AgenticSearchMatch {
                                    line_number: m.get("line_number").and_then(|p| p.as_u64()).map(|v| v as u32),
                                    content: m.get("content").and_then(|p| p.as_str()).map(ToOwned::to_owned),
                                    locator: m.get("locator").and_then(|p| p.as_str()).map(ToOwned::to_owned),
                                    context_before: m.get("context_before")
                                        .and_then(|p| p.as_array())
                                        .map(|items| {
                                            items.iter()
                                                .filter_map(|i| i.as_str().map(ToOwned::to_owned))
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    context_after: m.get("context_after")
                                        .and_then(|p| p.as_array())
                                        .map(|items| {
                                            items.iter()
                                                .filter_map(|i| i.as_str().map(ToOwned::to_owned))
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                }).collect()
                            })
                            .unwrap_or_default(),
                        sampled_lines: v
                            .get("sampled_lines")
                            .and_then(|s| s.as_array())
                            .map(|lines| {
                                lines.iter().map(|s| AgenticSearchSampledLine {
                                    line_number: s.get("line_number").and_then(|p| p.as_u64()).map(|v| v as u32),
                                    content: s.get("content").and_then(|p| p.as_str()).map(ToOwned::to_owned),
                                    locator: s.get("locator").and_then(|p| p.as_str()).map(ToOwned::to_owned),
                                    distance: s.get("distance").and_then(|p| p.as_f64()),
                                    weight: s.get("weight").and_then(|p| p.as_f64()),
                                }).collect()
                            })
                            .unwrap_or_default(),
                        document_runtime,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

fn parse_agentic_parse_llm_blocks_impl(json: &str) -> Result<Vec<AgenticParseLlmBlock>> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| napi::Error::from_reason(format!("Invalid tool metadata payload: {}", e)))?;

    let blocks = value
        .get("llm_blocks")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter().map(|b| AgenticParseLlmBlock {
                index: b.get("index").and_then(|i| i.as_u64()).map(|v| v as u32),
                kind: b.get("kind").and_then(|k| k.as_str()).map(ToOwned::to_owned),
                label: b.get("label").and_then(|l| l.as_str()).map(ToOwned::to_owned),
                location: b.get("location").and_then(|l| l.as_str()).map(ToOwned::to_owned),
            }).collect()
        })
        .unwrap_or_default();

    Ok(blocks)
}

// ============================================================================
// Public API Functions
// ============================================================================

/// Parse a JSON string or object with documentRuntime into a DocumentRuntime object.
#[napi]
pub fn parse_document_runtime(json: serde_json::Value) -> Option<DocumentRuntime> {
    // If it's a string, parse it as JSON first
    let value = if let Some(s) = json.as_str() {
        match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v) => v,
            Err(_) => return None,
        }
    } else {
        json
    };

    if let Some(obj) = value.as_object() {
        if let Some(doc_runtime) = obj.get("documentRuntime") {
            return Some(DocumentRuntime::from_json(doc_runtime));
        }
        if let Some(doc_runtime) = obj.get("document_runtime") {
            return Some(DocumentRuntime::from_json(doc_runtime));
        }
        // If the object itself is a document runtime (has ocr field)
        if obj.get("ocr").is_some() {
            return Some(DocumentRuntime::from_json(&value));
        }
    }
    Some(DocumentRuntime::from_json(&value))
}

/// Parse a JSON string or object into AgenticSearchResult objects.
#[napi]
pub fn parse_agentic_search_results(json: serde_json::Value) -> Result<Vec<AgenticSearchResult>> {
    // If the object has agenticSearchResults field, return it directly
    if let Some(arr) = json.get("agenticSearchResults").and_then(|v| v.as_array()) {
        return parse_agentic_search_results_impl(&serde_json::to_string(arr).unwrap_or_default());
    }
    // Otherwise parse as JSON string
    let json_str = serde_json::to_string(&json)
        .map_err(|e| napi::Error::from_reason(format!("Failed to serialize JSON: {}", e)))?;
    parse_agentic_search_results_impl(&json_str)
}

/// Parse a JSON string or object into AgenticParseLlmBlock objects.
#[napi]
pub fn parse_agentic_parse_llm_blocks(json: serde_json::Value) -> Result<Vec<AgenticParseLlmBlock>> {
    // If the object has agenticParseLlmBlocks field, return it directly
    if let Some(arr) = json.get("agenticParseLlmBlocks").and_then(|v| v.as_array()) {
        let blocks: Vec<AgenticParseLlmBlock> = arr
            .iter()
            .map(|b| AgenticParseLlmBlock {
                index: b.get("index").and_then(|i| i.as_u64()).map(|v| v as u32),
                kind: b.get("kind").and_then(|k| k.as_str()).map(ToOwned::to_owned),
                label: b.get("label").and_then(|l| l.as_str()).map(ToOwned::to_owned),
                location: b.get("location").and_then(|l| l.as_str()).map(ToOwned::to_owned),
            })
            .collect();
        return Ok(blocks);
    }
    // Otherwise parse as JSON string
    let json_str = serde_json::to_string(&json)
        .map_err(|e| napi::Error::from_reason(format!("Failed to serialize JSON: {}", e)))?;
    parse_agentic_parse_llm_blocks_impl(&json_str)
}

/// Input for enrichToolResult.
#[napi(object)]
pub struct EnrichToolResultInput {
    pub name: String,
    pub output: String,
    pub exit_code: u32,
    pub metadata_json: Option<String>,
    pub document_runtime_json: Option<String>,
}

/// Enrich a raw tool result by parsing its JSON metadata fields.
#[napi]
pub fn enrich_tool_result(input: EnrichToolResultInput) -> Result<EnrichedToolResult> {
    let document_runtime = input
        .document_runtime_json
        .as_ref()
        .and_then(|json| parse_document_runtime_impl(json).ok());

    let (agentic_search_results, agentic_parse_llm_blocks, metadata) = if let Some(ref json) = input.metadata_json {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| napi::Error::from_reason(format!("Invalid metadata JSON: {}", e)))?;

        let agentic_search_results = value
            .get("results")
            .map(|_| parse_agentic_search_results_impl(json).ok())
            .flatten();

        let agentic_parse_llm_blocks = value
            .get("llm_blocks")
            .map(|_| parse_agentic_parse_llm_blocks_impl(json).ok())
            .flatten();

        (agentic_search_results, agentic_parse_llm_blocks, Some(value))
    } else {
        (None, None, None)
    };

    Ok(EnrichedToolResult {
        name: input.name,
        output: input.output,
        exit_code: input.exit_code,
        document_runtime,
        metadata,
        agentic_search_results,
        agentic_parse_llm_blocks,
    })
}
