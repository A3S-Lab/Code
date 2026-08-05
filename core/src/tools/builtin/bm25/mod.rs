//! Native BM25 workspace search.

mod ranking;
#[cfg(test)]
mod tests;

use self::ranking::{query_terms, score_documents, tokenize, Bm25Document, B, K1};
use crate::text::truncate_utf8;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::workspace::{
    escape_control_chars_for_display, WorkspaceGlobRequest, WorkspaceGrepRequest, WorkspacePath,
};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use std::collections::{HashMap, HashSet};

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 25;
const DEFAULT_CONTEXT_LINES: usize = 2;
const MAX_CONTEXT_LINES: usize = 8;
const MAX_QUERY_BYTES: usize = 2_048;
const MAX_QUERY_TERMS: usize = 32;
const MAX_CANDIDATE_FILES: usize = 256;
const MAX_FILE_BYTES: usize = 512 * 1_024;
const MAX_TOTAL_BYTES: usize = 16 * 1_024 * 1_024;
const CHUNK_LINES: usize = 80;
const MAX_CHUNKS_PER_FILE: usize = 128;
const MAX_RESULTS_PER_FILE: usize = 2;
const READ_CONCURRENCY: usize = 8;
const MAX_RENDERED_LINE_BYTES: usize = 500;

pub struct Bm25Tool;

#[derive(Debug)]
struct CandidateSelection {
    paths: Vec<WorkspacePath>,
    matching_files: usize,
    backend_truncated: bool,
    candidate_truncated: bool,
    used_glob_fallback: bool,
}

#[derive(Debug)]
struct ChunkSource {
    path: WorkspacePath,
    start_line: usize,
    lines: Vec<String>,
}

#[derive(Debug, Default)]
struct ScanOutcome {
    sources: Vec<ChunkSource>,
    documents: Vec<Bm25Document>,
    read_files: usize,
    failed_reads: usize,
    oversized_files: usize,
    scanned_bytes: usize,
    truncated: bool,
}

#[derive(Debug)]
struct RenderedResult {
    content: String,
    metadata: serde_json::Value,
    source_anchor: String,
}

#[async_trait]
impl Tool for Bm25Tool {
    fn name(&self) -> &str {
        "bm25"
    }

    fn description(&self) -> &str {
        "Rank workspace text chunks with native BM25 lexical relevance. Use for multi-term or natural-language repository searches; use grep for exact strings and regular expressions."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_QUERY_BYTES,
                    "description": "Required. Plain-text relevance query. Code identifiers are expanded into camelCase and snake_case subterms."
                },
                "path": {
                    "type": "string",
                    "description": "Optional. Directory or file to search. Default: workspace root."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional. File glob filter, for example '*.rs' or '*.{ts,tsx}'."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_LIMIT,
                    "description": "Optional. Maximum ranked chunks to return. Default: 10; maximum: 25."
                },
                "context": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_CONTEXT_LINES,
                    "description": "Optional. Context lines before and after the strongest matching line in each chunk. Default: 2; maximum: 8."
                }
            },
            "required": ["query"],
            "examples": [
                {
                    "query": "session permission policy"
                },
                {
                    "query": "workspace path validation",
                    "path": "core/src",
                    "glob": "*.rs",
                    "limit": 8,
                    "context": 3
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::parallel_safe_read(2)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let query = match args.get("query").and_then(serde_json::Value::as_str) {
            Some(query) if !query.trim().is_empty() => query.trim(),
            _ => return Ok(ToolOutput::error("query parameter is required")),
        };
        if query.len() > MAX_QUERY_BYTES {
            return Ok(ToolOutput::error(format!(
                "query exceeds the {MAX_QUERY_BYTES}-byte limit"
            )));
        }
        let terms = query_terms(query, MAX_QUERY_TERMS);
        if terms.is_empty() {
            return Ok(ToolOutput::error(
                "query must contain at least one letter, number, underscore, or CJK character; use grep for punctuation-only searches",
            ));
        }

        let limit = match bounded_usize(args, "limit", DEFAULT_LIMIT, 1, MAX_LIMIT) {
            Ok(value) => value,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        let context_lines =
            match bounded_usize(args, "context", DEFAULT_CONTEXT_LINES, 0, MAX_CONTEXT_LINES) {
                Ok(value) => value,
                Err(error) => return Ok(ToolOutput::error(error)),
            };
        let path = match ctx.resolve_workspace_path(
            args.get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("."),
        ) {
            Ok(path) => path,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to resolve path: {error}"
                )))
            }
        };
        let glob = args
            .get("glob")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|glob| !glob.is_empty());

        let Some(search) = ctx.workspace_services.search() else {
            return Ok(ToolOutput::error(
                "bm25 is not available: this workspace backend did not provide search",
            ));
        };
        let candidates = match select_candidates(&terms, &path, glob, search, ctx).await {
            Ok(candidates) => candidates,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "BM25 candidate search failed: {error}"
                )))
            }
        };
        if candidates.paths.is_empty() {
            return Ok(no_matches_output(
                query,
                &terms,
                &candidates,
                &ScanOutcome::default(),
            ));
        }

        let scan = scan_candidates(candidates.paths.clone(), ctx).await;
        if scan.documents.is_empty() {
            if scan.read_files == 0 && scan.failed_reads > 0 {
                return Ok(ToolOutput::error(format!(
                    "BM25 could not read any of the {} candidate file(s)",
                    candidates.paths.len()
                ))
                .with_metadata(search_metadata(&terms, &candidates, &scan)));
            }
            return Ok(no_matches_output(query, &terms, &candidates, &scan));
        }

        let scores = score_documents(&terms, &scan.documents);
        let mut ranked = scores
            .into_iter()
            .enumerate()
            .filter(|(_, score)| score.is_finite() && *score > 0.0)
            .collect::<Vec<_>>();
        ranked.sort_by(|(left_index, left_score), (right_index, right_score)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| {
                    scan.sources[*left_index]
                        .path
                        .as_str()
                        .cmp(scan.sources[*right_index].path.as_str())
                })
                .then_with(|| {
                    scan.sources[*left_index]
                        .start_line
                        .cmp(&scan.sources[*right_index].start_line)
                })
        });

        let mut per_file = HashMap::new();
        let mut rendered = Vec::new();
        for (index, score) in ranked {
            let source = &scan.sources[index];
            let count = per_file.entry(source.path.as_str()).or_insert(0usize);
            if *count >= MAX_RESULTS_PER_FILE {
                continue;
            }
            *count += 1;
            rendered.push(render_result(
                rendered.len() + 1,
                source,
                score,
                &terms,
                context_lines,
            ));
            if rendered.len() >= limit {
                break;
            }
        }
        if rendered.is_empty() {
            return Ok(no_matches_output(query, &terms, &candidates, &scan));
        }

        let safe_query = escape_control_chars_for_display(truncate_utf8(query, 256));
        let mut content = format!("BM25 results for: {safe_query}\n\n");
        content.push_str(
            &rendered
                .iter()
                .map(|result| result.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        content.push_str(&format!(
            "\n\n{} result(s); {} file(s) read; {} chunk(s) scored",
            rendered.len(),
            scan.read_files,
            scan.documents.len()
        ));
        if candidates.backend_truncated || candidates.candidate_truncated || scan.truncated {
            content.push_str("\nWarning: search limits truncated the candidate corpus.");
        }

        let mut seen_anchors = HashSet::new();
        let source_anchors = rendered
            .iter()
            .filter(|result| seen_anchors.insert(result.source_anchor.clone()))
            .map(|result| result.source_anchor.clone())
            .collect::<Vec<_>>();
        let results = rendered
            .into_iter()
            .map(|result| result.metadata)
            .collect::<Vec<_>>();
        let mut metadata = search_metadata(&terms, &candidates, &scan);
        metadata["source_anchors"] = serde_json::json!(source_anchors);
        metadata["results"] = serde_json::json!(results);
        metadata["returned_results"] = serde_json::json!(results.len());

        Ok(ToolOutput::success(content).with_metadata(metadata))
    }
}

fn bounded_usize(
    args: &serde_json::Value,
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> std::result::Result<usize, String> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    let Some(value) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
        return Err(format!(
            "{name} must be an integer from {minimum} to {maximum}"
        ));
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{name} must be from {minimum} to {maximum}"));
    }
    Ok(value)
}

async fn select_candidates(
    terms: &[String],
    path: &WorkspacePath,
    glob: Option<&str>,
    search: std::sync::Arc<dyn crate::workspace::WorkspaceSearch>,
    ctx: &ToolContext,
) -> Result<CandidateSelection> {
    let mut pattern_terms = terms.to_vec();
    pattern_terms.sort_by_key(|term| std::cmp::Reverse(term.len()));
    let pattern = pattern_terms
        .iter()
        .map(|term| regex::escape(term))
        .collect::<Vec<_>>()
        .join("|");
    let request = WorkspaceGrepRequest {
        base: path.clone(),
        pattern,
        glob: glob.map(str::to_string),
        context_lines: 0,
        case_insensitive: true,
        max_output_size: 0,
    };
    let grep_search = std::sync::Arc::clone(&search);
    let outcome = ctx
        .workspace_services
        .run_with_timeout("bm25 candidate search", async move {
            grep_search.grep_with_sources(request).await
        })
        .await?;
    let matching_files = outcome.result.file_count;
    let backend_truncated = outcome.result.truncated;

    let (raw_paths, used_glob_fallback) = match outcome.matched_paths {
        Some(paths) => (paths, false),
        None => {
            let request = WorkspaceGlobRequest {
                base: path.clone(),
                pattern: glob.unwrap_or("**/*").to_string(),
            };
            let paths = ctx
                .workspace_services
                .run_with_timeout(
                    "bm25 candidate glob",
                    async move { search.glob(request).await },
                )
                .await?
                .matches;
            (paths, true)
        }
    };
    let raw_path_count = raw_paths.len();
    let mut seen = HashSet::new();
    let paths = raw_paths
        .into_iter()
        .filter_map(|path| ctx.resolve_workspace_path(path.as_str()).ok())
        .filter(|path| !path.is_root() && seen.insert(path.as_str().to_string()))
        .take(MAX_CANDIDATE_FILES)
        .collect::<Vec<_>>();

    Ok(CandidateSelection {
        paths,
        matching_files,
        backend_truncated,
        candidate_truncated: raw_path_count > MAX_CANDIDATE_FILES,
        used_glob_fallback,
    })
}

async fn scan_candidates(paths: Vec<WorkspacePath>, ctx: &ToolContext) -> ScanOutcome {
    let calls = paths.into_iter().map(|path| {
        let services = ctx.workspace_services.clone();
        async move {
            let fs = services.fs();
            let read_path = path.clone();
            let result = services
                .run_with_timeout("bm25 read", async move { fs.read_text(&read_path).await })
                .await;
            (path, result)
        }
    });
    let mut reads = stream::iter(calls).buffered(READ_CONCURRENCY);
    let mut outcome = ScanOutcome::default();

    while let Some((path, result)) = reads.next().await {
        let content = match result {
            Ok(content) => content,
            Err(_) => {
                outcome.failed_reads += 1;
                continue;
            }
        };
        if content.len() > MAX_FILE_BYTES {
            outcome.oversized_files += 1;
            continue;
        }
        if outcome.scanned_bytes.saturating_add(content.len()) > MAX_TOTAL_BYTES {
            outcome.truncated = true;
            break;
        }
        outcome.scanned_bytes += content.len();
        outcome.read_files += 1;

        let lines = content.lines().map(str::to_string).collect::<Vec<_>>();
        let chunk_count = lines.len().div_ceil(CHUNK_LINES);
        if chunk_count > MAX_CHUNKS_PER_FILE {
            outcome.truncated = true;
        }
        for (chunk_index, chunk_lines) in lines
            .chunks(CHUNK_LINES)
            .take(MAX_CHUNKS_PER_FILE)
            .enumerate()
        {
            let document = Bm25Document::from_text(&chunk_lines.join("\n"));
            if document.length == 0 {
                continue;
            }
            outcome.sources.push(ChunkSource {
                path: path.clone(),
                start_line: chunk_index * CHUNK_LINES,
                lines: chunk_lines.to_vec(),
            });
            outcome.documents.push(document);
        }
    }
    outcome
}

fn render_result(
    rank: usize,
    source: &ChunkSource,
    score: f64,
    terms: &[String],
    context_lines: usize,
) -> RenderedResult {
    let query_terms = terms.iter().map(String::as_str).collect::<HashSet<_>>();
    let best_line = source
        .lines
        .iter()
        .enumerate()
        .max_by(|(left_index, left), (right_index, right)| {
            line_match_count(left, &query_terms)
                .cmp(&line_match_count(right, &query_terms))
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, _)| index)
        .unwrap_or_default();
    let local_start = best_line.saturating_sub(context_lines);
    let local_end = (best_line + context_lines + 1).min(source.lines.len());
    let start_line = source.start_line + local_start + 1;
    let end_line = source.start_line + local_end;
    let safe_path = escape_control_chars_for_display(source.path.as_str());
    let mut content = format!("{rank}. {safe_path}:{start_line}-{end_line} (score {score:.4})");
    for (offset, line) in source.lines[local_start..local_end].iter().enumerate() {
        let safe_line = escape_control_chars_for_display(line);
        let rendered = truncate_utf8(&safe_line, MAX_RENDERED_LINE_BYTES);
        content.push_str(&format!("\n{:>6} | {rendered}", start_line + offset));
        if rendered.len() < safe_line.len() {
            content.push('…');
        }
    }

    RenderedResult {
        content,
        metadata: serde_json::json!({
            "path": source.path.as_str(),
            "start_line": start_line,
            "end_line": end_line,
            "score": score,
        }),
        source_anchor: source.path.as_str().to_string(),
    }
}

fn line_match_count(line: &str, query_terms: &HashSet<&str>) -> usize {
    tokenize(line)
        .iter()
        .filter(|term| query_terms.contains(term.as_str()))
        .count()
}

fn no_matches_output(
    query: &str,
    terms: &[String],
    candidates: &CandidateSelection,
    scan: &ScanOutcome,
) -> ToolOutput {
    let safe_query = escape_control_chars_for_display(truncate_utf8(query, 256));
    ToolOutput::success(format!("No BM25 matches found for query: {safe_query}"))
        .with_metadata(search_metadata(terms, candidates, scan))
}

fn search_metadata(
    terms: &[String],
    candidates: &CandidateSelection,
    scan: &ScanOutcome,
) -> serde_json::Value {
    serde_json::json!({
        "algorithm": "bm25",
        "parameters": {
            "k1": K1,
            "b": B,
            "chunk_lines": CHUNK_LINES,
        },
        "query_terms": terms,
        "candidate_search": {
            "matching_files": candidates.matching_files,
            "selected_files": candidates.paths.len(),
            "backend_truncated": candidates.backend_truncated,
            "candidate_truncated": candidates.candidate_truncated,
            "used_glob_fallback": candidates.used_glob_fallback,
        },
        "scan": {
            "read_files": scan.read_files,
            "failed_reads": scan.failed_reads,
            "oversized_files": scan.oversized_files,
            "scanned_bytes": scan.scanned_bytes,
            "scored_chunks": scan.documents.len(),
            "truncated": scan.truncated,
        },
    })
}
