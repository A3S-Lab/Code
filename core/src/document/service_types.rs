use crate::config::{AgenticParseConfig, AgenticSearchConfig};
use crate::document_consume;
use crate::document_parser::DocumentBlockKind;
use regex::Regex;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseExecutionStrategy {
    Auto,
    Structured,
    Narrative,
    Tabular,
    Code,
}

impl ParseExecutionStrategy {
    pub(crate) fn from_str(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "structured" => Self::Structured,
            "narrative" => Self::Narrative,
            "tabular" => Self::Tabular,
            "code" => Self::Code,
            _ => Self::Auto,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Structured => "structured",
            Self::Narrative => "narrative",
            Self::Tabular => "tabular",
            Self::Code => "code",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedParseRequest {
    pub strategy: ParseExecutionStrategy,
    pub max_chars: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchExecutionMode {
    Fast,
    Deep,
    FilenameOnly,
}

impl SearchExecutionMode {
    pub(crate) fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "deep" => Self::Deep,
            "filename_only" | "filename" => Self::FilenameOnly,
            _ => Self::Fast,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedSearchRequest {
    pub mode: SearchExecutionMode,
    pub max_results: usize,
    pub context_lines: usize,
    pub include_glob: Option<String>,
}

pub(crate) fn resolve_parse_request(
    args: &serde_json::Value,
    defaults: Option<&AgenticParseConfig>,
) -> ResolvedParseRequest {
    let normalized_defaults = defaults.cloned().unwrap_or_default().normalized();

    let strategy = args
        .get("strategy")
        .and_then(|v| v.as_str())
        .map(ParseExecutionStrategy::from_str)
        .unwrap_or_else(|| ParseExecutionStrategy::from_str(&normalized_defaults.default_strategy));

    let max_chars = args
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(normalized_defaults.max_chars)
        .clamp(500, 200_000);

    ResolvedParseRequest {
        strategy,
        max_chars,
    }
}

pub(crate) fn resolve_search_request(
    args: &serde_json::Value,
    defaults: Option<&AgenticSearchConfig>,
) -> ResolvedSearchRequest {
    let normalized_defaults = defaults.cloned().unwrap_or_default().normalized();

    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .map(SearchExecutionMode::from_str)
        .unwrap_or_else(|| SearchExecutionMode::from_str(&normalized_defaults.default_mode));

    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(normalized_defaults.max_results)
        .clamp(1, 100);

    let context_lines = args
        .get("context_lines")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(normalized_defaults.context_lines)
        .min(20);

    let include_glob = args
        .get("include")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    ResolvedSearchRequest {
        mode,
        max_results,
        context_lines,
        include_glob,
    }
}

#[allow(dead_code)]
pub(crate) fn structural_summary_style_for_strategy(
    strategy: ParseExecutionStrategy,
) -> document_consume::StructuralSummaryStyle {
    match strategy {
        ParseExecutionStrategy::Code => document_consume::StructuralSummaryStyle::Code,
        ParseExecutionStrategy::Tabular => document_consume::StructuralSummaryStyle::Tabular,
        _ => document_consume::StructuralSummaryStyle::Narrative,
    }
}

pub(crate) fn structural_summary_style_for_strategy_label(
    strategy_label: &str,
) -> document_consume::StructuralSummaryStyle {
    match strategy_label {
        "code" => document_consume::StructuralSummaryStyle::Code,
        "tabular" => document_consume::StructuralSummaryStyle::Tabular,
        _ => document_consume::StructuralSummaryStyle::Narrative,
    }
}

pub(crate) fn detect_parse_strategy(path: &Path, raw_text: &str) -> ParseExecutionStrategy {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "csv" | "tsv" => return ParseExecutionStrategy::Tabular,
        "json" | "toml" | "yaml" | "yml" | "xml" | "hcl" => {
            return ParseExecutionStrategy::Structured
        }
        "md" | "markdown" | "mdx" | "rst" | "txt" | "adoc" | "org" | "tex" | "latex" | "typ"
        | "typst" => return ParseExecutionStrategy::Narrative,
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "java" | "c" | "cpp" | "h" | "hpp"
        | "cs" | "rb" | "sh" | "bash" | "zsh" | "fish" | "sql" | "graphql" | "proto" | "tf" => {
            return ParseExecutionStrategy::Code
        }
        _ => {}
    }

    let total = raw_text.lines().count().max(1);
    let comma_rows = raw_text
        .lines()
        .filter(|l| l.matches(',').count() >= 2)
        .count();
    if comma_rows * 100 / total > 50 {
        ParseExecutionStrategy::Tabular
    } else {
        ParseExecutionStrategy::Narrative
    }
}

pub(crate) fn detect_parse_strategy_label(
    path: &Path,
    raw_text: &str,
    strategy_hint: &str,
) -> String {
    if strategy_hint != "auto" {
        return strategy_hint.to_string();
    }

    detect_parse_strategy(path, raw_text).label().to_string()
}

pub(crate) fn document_block_kind_label(kind: &DocumentBlockKind) -> &'static str {
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

pub(crate) fn search_path_signal_score(workspace: &Path, path: &Path, patterns: &[Regex]) -> f32 {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    let file_name = relative
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let rel_path = relative.to_string_lossy();

    let file_name_hits = patterns
        .iter()
        .filter(|pattern| pattern.is_match(file_name))
        .count();
    let path_hits = patterns
        .iter()
        .filter(|pattern| pattern.is_match(&rel_path))
        .count();

    file_name_hits as f32 * 0.8 + path_hits as f32 * 0.25
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_parse_strategy_treats_extended_markup_as_narrative() {
        for path in [
            Path::new("notes.mdx"),
            Path::new("paper.org"),
            Path::new("report.tex"),
            Path::new("slides.typst"),
        ] {
            assert_eq!(
                detect_parse_strategy(path, "Heading\n\nSome narrative text."),
                ParseExecutionStrategy::Narrative
            );
        }
    }
}
