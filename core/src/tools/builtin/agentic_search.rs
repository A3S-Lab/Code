//! Agentic Search Tool - Sirchmunk-inspired intelligent code search
//!
//! Implements multi-phase search pipeline:
//! - Phase 0: Semantic cache (future)
//! - Phase 1: Parallel probing (keywords + structure)
//! - Phase 2: Dual retrieval (content + structure)
//! - Phase 3: Result synthesis with relevance scoring

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;

/// Search mode for agentic search
#[derive(Debug, Clone, Copy)]
enum SearchMode {
    /// Fast mode: 2-level keyword cascade (2-5s)
    Fast,
    /// Deep mode: Monte Carlo evidence sampling around high-scoring regions (10-30s)
    Deep,
    /// Filename only: Quick file discovery
    FilenameOnly,
}

impl SearchMode {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "deep" => Self::Deep,
            "filename_only" | "filename" => Self::FilenameOnly,
            _ => Self::Fast,
        }
    }
}

/// File match result with relevance scoring
#[derive(Debug, Clone)]
struct FileMatch {
    path: PathBuf,
    matches: Vec<MatchLine>,
    relevance: f32,
    file_type: FileType,
}

#[derive(Debug, Clone)]
struct MatchLine {
    line_number: usize,
    content: String,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FileType {
    Code,
    Config,
    Documentation,
    Other,
}

impl FileType {
    fn from_path(path: &std::path::Path) -> Self {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext {
                "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "c" | "cpp" | "h"
                | "hpp" => Self::Code,
                "toml" | "yaml" | "yml" | "json" | "ini" | "conf" => Self::Config,
                "md" | "txt" | "rst" | "adoc" => Self::Documentation,
                _ => Self::Other,
            }
        } else {
            Self::Other
        }
    }

    fn relevance_boost(&self) -> f32 {
        match self {
            Self::Code => 1.2,
            Self::Config => 1.0,
            Self::Documentation => 0.9,
            Self::Other => 0.8,
        }
    }
}

pub struct AgenticSearchTool;

impl AgenticSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AgenticSearchTool {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AgenticSearchTool {
    fn name(&self) -> &str {
        "agentic_search"
    }

    fn description(&self) -> &str {
        "Intelligent multi-phase code search. Extracts keywords from natural language queries, \
         searches file contents and structure in parallel, then ranks results by relevance. \
         Supports fast mode (default) and filename_only mode for quick file discovery."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Required. Natural language search query, for example 'JWT token validation' or 'how does authentication work'. Always provide this exact field name: 'query'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["fast", "deep", "filename_only"],
                    "description": "Optional. Search mode. Default: fast."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Optional. Maximum number of files to return. Default: 10."
                },
                "include": {
                    "type": "string",
                    "description": "Optional. Glob pattern to filter files, for example '*.rs' or '*.{ts,tsx}'."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Optional. Lines of context around each match. Default: 2."
                }
            },
            "required": ["query"],
            "examples": [
                {
                    "query": "JWT token validation"
                },
                {
                    "query": "authentication flow",
                    "mode": "deep",
                    "max_results": 8,
                    "include": "*.rs",
                    "context_lines": 3
                }
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("query parameter is required"))?;

        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .map(SearchMode::from_str)
            .unwrap_or(SearchMode::Fast);

        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let include_glob = args.get("include").and_then(|v| v.as_str());

        let context_lines = args
            .get("context_lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;

        match mode {
            SearchMode::Fast => {
                self.fast_search(query, max_results, include_glob, context_lines, ctx)
                    .await
            }
            SearchMode::Deep => {
                self.deep_search(query, max_results, include_glob, context_lines, ctx)
                    .await
            }
            SearchMode::FilenameOnly => {
                self.filename_search(query, max_results, include_glob, ctx)
                    .await
            }
        }
    }
}

impl AgenticSearchTool {
    /// Phase 1+2: Parallel keyword extraction + content search
    async fn fast_search(
        &self,
        query: &str,
        max_results: usize,
        include_glob: Option<&str>,
        context_lines: usize,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let workspace = ctx.workspace.clone();
        let keywords = extract_keywords(query);
        let include = include_glob.map(|s| s.to_string());
        let registry = ctx.document_parsers.clone();

        // Phase 1+2: Parallel search across all keywords
        let matches = tokio::task::spawn_blocking(move || {
            search_workspace(
                &workspace,
                &keywords,
                max_results,
                include.as_deref(),
                context_lines,
                registry.as_deref(),
            )
        })
        .await
        .map_err(|e| anyhow::anyhow!("Search task failed: {}", e))??;

        if matches.is_empty() {
            return Ok(ToolOutput::success(format!(
                "No results found for: {}\n\nTry broader keywords or check the search path.",
                query
            )));
        }

        let output = format_results(&matches, query);
        Ok(ToolOutput::success(output).with_metadata(json!({
            "result_count": matches.len(),
            "keywords": extract_keywords(query),
            "mode": "fast"
        })))
    }

    /// DEEP mode: Monte Carlo evidence sampling for comprehensive analysis
    ///
    /// Implements a three-act exploration strategy:
    /// - Act 1: Fuzzy anchoring - find lexically similar regions
    /// - Act 2: Gaussian importance sampling - explore around high-scoring regions
    /// - Act 3: Synthesis - combine evidence into coherent result
    async fn deep_search(
        &self,
        query: &str,
        max_results: usize,
        include_glob: Option<&str>,
        context_lines: usize,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let workspace = ctx.workspace.clone();
        let keywords = extract_keywords(query);
        let include = include_glob.map(|s| s.to_string());
        let registry = ctx.document_parsers.clone();

        // Phase 1: Initial broad search (2x max_results for sampling pool)
        let initial_matches = tokio::task::spawn_blocking(move || {
            search_workspace(
                &workspace,
                &keywords,
                max_results * 2,
                include.as_deref(),
                context_lines,
                registry.as_deref(),
            )
        })
        .await
        .map_err(|e| anyhow::anyhow!("Search task failed: {}", e))??;

        if initial_matches.is_empty() {
            return Ok(ToolOutput::success(format!(
                "No results found for: {}\n\nTry broader keywords or check the search path.",
                query
            )));
        }

        // Phase 2: Monte Carlo evidence sampling
        let sampled_matches = monte_carlo_sample(&initial_matches, max_results);

        // Phase 3: Enhanced formatting with evidence synthesis
        let output = format_deep_results(&sampled_matches, query);
        Ok(ToolOutput::success(output).with_metadata(json!({
            "result_count": sampled_matches.len(),
            "initial_pool_size": initial_matches.len(),
            "keywords": extract_keywords(query),
            "mode": "deep"
        })))
    }

    /// Filename-only search: quick file discovery
    async fn filename_search(
        &self,
        query: &str,
        max_results: usize,
        include_glob: Option<&str>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let workspace = ctx.workspace.clone();
        let keywords = extract_keywords(query);
        let include = include_glob.map(|s| s.to_string());

        let files = tokio::task::spawn_blocking(move || {
            find_matching_files(&workspace, &keywords, max_results, include.as_deref())
        })
        .await
        .map_err(|e| anyhow::anyhow!("File search task failed: {}", e))??;

        if files.is_empty() {
            return Ok(ToolOutput::success(format!(
                "No files found matching: {}",
                query
            )));
        }

        let output = files
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolOutput::success(output).with_metadata(json!({
            "result_count": files.len(),
            "mode": "filename_only"
        })))
    }
}

// ============================================================================
// Phase 1: Keyword extraction
// ============================================================================

/// Extract search keywords from a natural language query.
///
/// Strips stop words and returns meaningful search terms at multiple granularities:
/// - Full phrase (for exact match)
/// - Individual words (for broad match)
fn extract_keywords(query: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "need", "dare", "ought", "how", "what", "where", "when", "why", "who", "which", "that",
        "this", "these", "those", "it", "its", "in", "on", "at", "to", "for", "of", "with", "by",
        "from", "up", "about", "into", "through", "during", "and", "or", "but", "not", "no", "nor",
        "so", "yet", "both", "either", "work", "works", "working", "find", "get", "use", "make",
        "look",
    ];

    let mut keywords: Vec<String> = Vec::new();

    // Multi-word phrases (2-3 word combinations)
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.len() >= 2 {
        for window in words.windows(2) {
            let phrase = window.join(" ");
            if !phrase.chars().all(|c| c.is_whitespace()) {
                keywords.push(phrase);
            }
        }
    }

    // Individual meaningful words
    for word in &words {
        let clean = word
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_lowercase();
        if clean.len() >= 3 && !STOP_WORDS.contains(&clean.as_str()) {
            keywords.push(clean);
        }
    }

    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    keywords.retain(|k| seen.insert(k.clone()));

    // Limit to top 8 keywords to avoid over-searching
    keywords.truncate(8);
    keywords
}

// ============================================================================
// Phase 2: Content search
// ============================================================================

/// Search workspace files for keyword matches.
///
/// Uses IDF-weighted scoring: files matching more keywords rank higher.
/// When `parser_registry` is None, only plain text files are searched (default behavior).
/// When provided, the registry handles file reading and enables custom format support.
fn search_workspace(
    workspace: &std::path::Path,
    keywords: &[String],
    max_results: usize,
    include_glob: Option<&str>,
    context_lines: usize,
    parser_registry: Option<&crate::document_parser::DocumentParserRegistry>,
) -> Result<Vec<FileMatch>> {
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    // Build regex patterns for each keyword
    let patterns: Vec<Regex> = keywords
        .iter()
        .filter_map(|kw| Regex::new(&format!("(?i){}", regex::escape(kw))).ok())
        .collect();

    if patterns.is_empty() {
        return Ok(Vec::new());
    }

    let mut builder = WalkBuilder::new(workspace);
    builder.hidden(false).git_ignore(true).git_global(true);

    if let Some(glob) = include_glob {
        let mut types = ignore::types::TypesBuilder::new();
        if types.add("custom", glob).is_ok() {
            types.select("custom");
            if let Ok(built) = types.build() {
                builder.types(built);
            }
        }
    }

    let mut file_matches: Vec<FileMatch> = Vec::new();

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Read file content: use parser registry if available, otherwise plain text only
        let content = if let Some(registry) = parser_registry {
            // Use parser registry (supports custom formats)
            match registry.parse_file(path) {
                Ok(Some(c)) => c,
                Ok(None) => continue, // no parser available or file too large
                Err(_) => continue,   // parse error, skip
            }
        } else {
            // Fallback: plain text only (default behavior)
            // Skip files > 1MB
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > 1024 * 1024 {
                    continue;
                }
            }
            match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // skip binary files
            }
        };

        if content.trim().is_empty() {
            continue;
        }

        let lines: Vec<&str> = content.lines().collect();
        let mut all_matches: Vec<MatchLine> = Vec::new();
        let mut keyword_hits: HashMap<usize, usize> = HashMap::new(); // line_idx -> hit count

        // Find all matches across all patterns
        for pattern in &patterns {
            for (idx, line) in lines.iter().enumerate() {
                if pattern.is_match(line) {
                    *keyword_hits.entry(idx).or_insert(0) += 1;
                }
            }
        }

        if keyword_hits.is_empty() {
            continue;
        }

        // Build match results with context
        let mut seen_lines = std::collections::HashSet::new();
        let mut sorted_hits: Vec<(usize, usize)> = keyword_hits.into_iter().collect();
        sorted_hits.sort_by(|a, b| b.1.cmp(&a.1)); // sort by hit count desc

        for (line_idx, _) in sorted_hits.iter().take(20) {
            if seen_lines.contains(line_idx) {
                continue;
            }
            seen_lines.insert(*line_idx);

            let before_start = line_idx.saturating_sub(context_lines);
            let after_end = (line_idx + context_lines + 1).min(lines.len());

            all_matches.push(MatchLine {
                line_number: line_idx + 1,
                content: lines[*line_idx].to_string(),
                context_before: lines[before_start..*line_idx]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                context_after: lines[line_idx + 1..after_end]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }

        all_matches.sort_by_key(|m| m.line_number);

        // IDF-weighted relevance: more keyword hits + file type boost
        let unique_keywords_matched = patterns
            .iter()
            .filter(|p| lines.iter().any(|l| p.is_match(l)))
            .count();
        let file_type = FileType::from_path(path);
        let base_score = (all_matches.len() as f32) / (lines.len() as f32).sqrt();
        let idf_boost = (unique_keywords_matched as f32) / (patterns.len() as f32);
        let relevance = base_score * idf_boost * file_type.relevance_boost();

        file_matches.push(FileMatch {
            path: path.to_path_buf(),
            matches: all_matches,
            relevance,
            file_type,
        });
    }

    // Sort by relevance descending
    file_matches.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    file_matches.truncate(max_results);

    Ok(file_matches)
}

/// Find files whose names match keywords (filename_only mode).
fn find_matching_files(
    workspace: &std::path::Path,
    keywords: &[String],
    max_results: usize,
    include_glob: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let patterns: Vec<Regex> = keywords
        .iter()
        .filter_map(|kw| Regex::new(&format!("(?i){}", regex::escape(kw))).ok())
        .collect();

    let mut builder = WalkBuilder::new(workspace);
    builder.hidden(false).git_ignore(true);

    if let Some(glob) = include_glob {
        let mut types = ignore::types::TypesBuilder::new();
        if types.add("custom", glob).is_ok() {
            types.select("custom");
            if let Ok(built) = types.build() {
                builder.types(built);
            }
        }
    }

    let mut results: Vec<PathBuf> = Vec::new();

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if patterns.iter().any(|p| p.is_match(name)) {
            results.push(path.to_path_buf());
            if results.len() >= max_results {
                break;
            }
        }
    }

    Ok(results)
}

// ============================================================================
// Phase 3: Result formatting
// ============================================================================

fn format_results(matches: &[FileMatch], query: &str) -> String {
    let mut out = format!("Found {} file(s) matching \"{}\"\n\n", matches.len(), query);

    for (i, fm) in matches.iter().enumerate() {
        let rel_path = fm.path.display();
        let type_label = match fm.file_type {
            FileType::Code => "code",
            FileType::Config => "config",
            FileType::Documentation => "docs",
            FileType::Other => "file",
        };

        out.push_str(&format!(
            "─── {} ({}, relevance: {:.2}) ───\n",
            rel_path, type_label, fm.relevance
        ));

        for m in fm.matches.iter().take(5) {
            for ctx in &m.context_before {
                out.push_str(&format!("  {}\n", ctx));
            }
            out.push_str(&format!("▶ L{}: {}\n", m.line_number, m.content));
            for ctx in &m.context_after {
                out.push_str(&format!("  {}\n", ctx));
            }
            out.push('\n');
        }

        if fm.matches.len() > 5 {
            out.push_str(&format!("  ... {} more matches\n", fm.matches.len() - 5));
        }

        if i < matches.len() - 1 {
            out.push('\n');
        }
    }

    out
}

// ============================================================================
// Monte Carlo Evidence Sampling (DEEP mode)
// ============================================================================

/// Evidence region: a contiguous block of lines with a score
#[derive(Debug, Clone)]
struct EvidenceRegion {
    file_match: FileMatch,
    /// Gaussian-sampled lines around high-scoring matches
    sampled_lines: Vec<SampledLine>,
    /// Composite evidence score
    evidence_score: f32,
}

#[derive(Debug, Clone)]
struct SampledLine {
    line_number: usize,
    content: String,
    /// Distance from nearest match (0 = exact match)
    distance: usize,
    /// Gaussian weight: exp(-distance^2 / (2 * sigma^2))
    weight: f32,
}

/// Monte Carlo evidence sampling.
///
/// Three-act strategy:
/// - Act 1: Fuzzy anchoring — identify high-scoring regions
/// - Act 2: Gaussian importance sampling — sample around anchors with decaying sigma
/// - Act 3: Top-K selection — return highest-evidence regions
fn monte_carlo_sample(matches: &[FileMatch], top_k: usize) -> Vec<EvidenceRegion> {
    let mut regions: Vec<EvidenceRegion> = Vec::new();

    for fm in matches {
        // Act 1: Fuzzy anchoring — collect all match line numbers as anchors
        let anchors: Vec<usize> = fm.matches.iter().map(|m| m.line_number).collect();
        if anchors.is_empty() {
            continue;
        }

        // Read file lines for Gaussian sampling
        let lines: Vec<String> = match std::fs::read_to_string(&fm.path) {
            Ok(content) => content.lines().map(|l| l.to_string()).collect(),
            Err(_) => continue,
        };

        if lines.is_empty() {
            continue;
        }

        // Act 2: Gaussian importance sampling
        // sigma starts at 5 lines, decays toward 2 for denser matches
        let match_density = anchors.len() as f32 / lines.len() as f32;
        let sigma = (5.0_f32 - match_density * 20.0).max(2.0);

        let mut sampled: Vec<SampledLine> = Vec::new();
        let mut seen_lines = std::collections::HashSet::new();

        for &anchor in &anchors {
            // Sample a window around each anchor using Gaussian weights
            let window_radius = (sigma * 3.0) as usize;
            let start = anchor.saturating_sub(window_radius + 1);
            let end = (anchor + window_radius).min(lines.len());

            #[allow(clippy::needless_range_loop)]
            for line_idx in start..end {
                if seen_lines.contains(&line_idx) {
                    continue;
                }
                seen_lines.insert(line_idx);

                let distance = anchor.abs_diff(line_idx + 1);

                // Gaussian weight: exp(-d^2 / (2 * sigma^2))
                let weight = (-((distance * distance) as f32) / (2.0 * sigma * sigma)).exp();

                sampled.push(SampledLine {
                    line_number: line_idx + 1,
                    content: lines[line_idx].clone(),
                    distance,
                    weight,
                });
            }
        }

        // Sort sampled lines by line number for coherent output
        sampled.sort_by_key(|s| s.line_number);

        // Act 3: Compute composite evidence score
        // = file relevance * sum(gaussian weights) / sqrt(total sampled)
        let weight_sum: f32 = sampled.iter().map(|s| s.weight).sum();
        let evidence_score = fm.relevance * weight_sum / (sampled.len() as f32).sqrt().max(1.0);

        regions.push(EvidenceRegion {
            file_match: fm.clone(),
            sampled_lines: sampled,
            evidence_score,
        });
    }

    // Sort by evidence score descending, take top-K
    regions.sort_by(|a, b| {
        b.evidence_score
            .partial_cmp(&a.evidence_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    regions.truncate(top_k);
    regions
}

/// Format deep search results with evidence regions and Gaussian weights
fn format_deep_results(regions: &[EvidenceRegion], query: &str) -> String {
    let mut out = format!(
        "Deep search found {} evidence region(s) for \"{}\"\n\n",
        regions.len(),
        query
    );

    for (i, region) in regions.iter().enumerate() {
        let rel_path = region.file_match.path.display();
        let type_label = match region.file_match.file_type {
            FileType::Code => "code",
            FileType::Config => "config",
            FileType::Documentation => "docs",
            FileType::Other => "file",
        };

        out.push_str(&format!(
            "━━━ {} ({}, evidence: {:.3}) ━━━\n",
            rel_path, type_label, region.evidence_score
        ));

        // Group sampled lines into contiguous blocks for readability
        let blocks = group_into_blocks(&region.sampled_lines);
        for block in &blocks {
            for line in block {
                let marker = if line.distance == 0 { "▶" } else { " " };
                out.push_str(&format!(
                    "{} L{:4}: {}\n",
                    marker, line.line_number, line.content
                ));
            }
            out.push('\n');
        }

        if i < regions.len() - 1 {
            out.push('\n');
        }
    }

    out
}

/// Group sampled lines into contiguous blocks (gap > 2 lines = new block)
fn group_into_blocks(lines: &[SampledLine]) -> Vec<Vec<&SampledLine>> {
    let mut blocks: Vec<Vec<&SampledLine>> = Vec::new();
    let mut current: Vec<&SampledLine> = Vec::new();

    for line in lines {
        if let Some(last) = current.last() {
            if line.line_number > last.line_number + 3 && !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        }
        current.push(line);
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        let mut f = File::create(root.join("auth.rs")).unwrap();
        writeln!(f, "use jwt::Token;\n\npub fn verify_token(token: &str) -> Result<Claims> {{\n    // JWT verification\n    todo!()\n}}").unwrap();

        let mut f = File::create(root.join("session.rs")).unwrap();
        writeln!(f, "pub struct Session {{\n    pub user_id: String,\n    pub token: String,\n}}\n\nimpl Session {{\n    pub fn new(user_id: &str, token: &str) -> Self {{\n        Self {{ user_id: user_id.to_string(), token: token.to_string() }}\n    }}\n}}").unwrap();

        let mut f = File::create(root.join("README.md")).unwrap();
        writeln!(
            f,
            "# Auth Service\n\nHandles JWT authentication and session management."
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_extract_keywords_basic() {
        let kws = extract_keywords("JWT token validation");
        assert!(kws.iter().any(|k| k.contains("jwt") || k.contains("JWT")));
        assert!(kws.iter().any(|k| k.contains("token")));
        assert!(kws.iter().any(|k| k.contains("validation")));
    }

    #[test]
    fn test_extract_keywords_stop_words_removed() {
        let kws = extract_keywords("how does the authentication work");
        assert!(!kws.iter().any(|k| k == "how"));
        assert!(!kws.iter().any(|k| k == "the"));
        assert!(kws.iter().any(|k| k.contains("authentication")));
    }

    #[test]
    fn test_extract_keywords_deduplication() {
        let kws = extract_keywords("auth auth authentication");
        let unique: std::collections::HashSet<_> = kws.iter().collect();
        assert_eq!(kws.len(), unique.len());
    }

    #[test]
    fn test_file_type_detection() {
        assert_eq!(
            FileType::from_path(std::path::Path::new("main.rs")),
            FileType::Code
        );
        assert_eq!(
            FileType::from_path(std::path::Path::new("config.toml")),
            FileType::Config
        );
        assert_eq!(
            FileType::from_path(std::path::Path::new("README.md")),
            FileType::Documentation
        );
    }

    #[test]
    fn test_agentic_search_schema_is_canonical() {
        let tool = AgenticSearchTool::new();
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["query"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["query"], "JWT token validation");
        assert!(examples[0].get("q").is_none());
    }

    #[tokio::test]
    async fn test_fast_search_finds_results() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "JWT token"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("auth.rs") || output.content.contains("session.rs"));
    }

    #[tokio::test]
    async fn test_filename_only_mode() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "auth", "mode": "filename_only"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("auth.rs"));
    }

    #[tokio::test]
    async fn test_no_results() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "xyznonexistentterm12345"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("No results"));
    }

    #[tokio::test]
    async fn test_tool_name() {
        let tool = AgenticSearchTool::new();
        assert_eq!(tool.name(), "agentic_search");
    }

    #[tokio::test]
    async fn test_max_results_respected() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "pub", "max_results": 1});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        if let Some(meta) = &output.metadata {
            let count = meta
                .get("result_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            assert!(count <= 1);
        }
    }

    #[tokio::test]
    async fn test_deep_mode() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "JWT token", "mode": "deep"});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("evidence"));
        if let Some(meta) = &output.metadata {
            assert_eq!(meta.get("mode").and_then(|v| v.as_str()), Some("deep"));
            assert!(meta.get("initial_pool_size").is_some());
        }
    }

    #[tokio::test]
    async fn test_deep_mode_monte_carlo_sampling() {
        let dir = setup_workspace();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let tool = AgenticSearchTool::new();

        let args = json!({"query": "token", "mode": "deep", "max_results": 2});
        let output = tool.execute(&args, &ctx).await.unwrap();

        assert!(output.success);
        // Deep mode should show evidence scores
        assert!(output.content.contains("evidence:"));
    }
}
