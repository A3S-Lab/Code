//! Pluggable, bounded workspace text splitting strategies.

use super::{ChunkingConfig, WorkspaceIndexError};
use std::fmt;
use std::sync::Arc;

const MIN_TARGET_BYTES: usize = 4;
const MAX_SEPARATOR_COUNT: usize = 16;
const MAX_SEPARATOR_BYTES: usize = 64;

/// One zero-based, half-open UTF-8 byte range proposed by a chunking strategy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceChunkRange {
    pub start_byte: usize,
    pub end_byte: usize,
}

impl WorkspaceChunkRange {
    pub const fn new(start_byte: usize, end_byte: usize) -> Self {
        Self {
            start_byte,
            end_byte,
        }
    }
}

/// Redacted input supplied to a host-owned custom chunking strategy.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct WorkspaceChunkingInput<'a> {
    /// Normalized workspace-relative source path.
    pub path: &'a str,
    /// Manifest language hint when one is available.
    pub language: Option<&'a str>,
    /// Complete admitted UTF-8 source text.
    pub content: &'a str,
    /// Hard limits enforced again after the strategy returns.
    pub limits: ChunkingConfig,
}

impl fmt::Debug for WorkspaceChunkingInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceChunkingInput")
            .field("has_language", &self.language.is_some())
            .field("content_bytes", &self.content.len())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// Bounded failure returned by built-in or host-owned chunking code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum WorkspaceChunkingError {
    #[error("invalid chunking option '{field}': {reason}")]
    InvalidConfiguration {
        field: &'static str,
        reason: &'static str,
    },
    #[error("custom workspace chunking strategy failed")]
    StrategyFailed,
}

/// Host extension point for deterministic workspace text splitting.
///
/// Implementations return ranges only. A3S Code validates complete coverage,
/// ordering, UTF-8 boundaries, chunk size/count budgets, then computes line
/// anchors, source digests, and stable IDs itself. Implementations must be pure,
/// deterministic, and must not retain `input` after this call.
pub trait CustomWorkspaceChunkingStrategy: Send + Sync {
    fn split(
        &self,
        input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError>;
}

/// Options for fixed UTF-8 byte windows with bounded overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct FixedWindowChunkingOptions {
    /// Maximum target bytes before adjustment to a UTF-8 boundary.
    pub target_bytes: usize,
    /// Maximum source bytes repeated from the preceding window.
    pub overlap_bytes: usize,
}

impl FixedWindowChunkingOptions {
    pub fn new(target_bytes: usize, overlap_bytes: usize) -> Result<Self, WorkspaceChunkingError> {
        validate_window(target_bytes, overlap_bytes)?;
        Ok(Self {
            target_bytes,
            overlap_bytes,
        })
    }
}

/// Options for recursive separator-aware text splitting.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RecursiveChunkingOptions {
    /// Maximum target bytes before choosing a preferred separator.
    pub target_bytes: usize,
    /// Maximum source bytes repeated from the preceding window.
    pub overlap_bytes: usize,
    separators: Arc<[Arc<str>]>,
}

impl RecursiveChunkingOptions {
    pub fn new(target_bytes: usize, overlap_bytes: usize) -> Result<Self, WorkspaceChunkingError> {
        validate_window(target_bytes, overlap_bytes)?;
        Ok(Self {
            target_bytes,
            overlap_bytes,
            separators: default_recursive_separators(),
        })
    }

    /// Override the prioritized separator list. Empty separators are rejected;
    /// a UTF-8-safe hard boundary is always the final fallback.
    pub fn with_separators<I, S>(mut self, separators: I) -> Result<Self, WorkspaceChunkingError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let separators = separators.into_iter().map(Into::into).collect::<Vec<_>>();
        if separators.is_empty() || separators.len() > MAX_SEPARATOR_COUNT {
            return Err(WorkspaceChunkingError::InvalidConfiguration {
                field: "separators",
                reason: "must contain between one and sixteen entries",
            });
        }
        if separators.iter().any(|separator| {
            separator.is_empty()
                || separator.len() > MAX_SEPARATOR_BYTES
                || separator.contains('\0')
        }) {
            return Err(WorkspaceChunkingError::InvalidConfiguration {
                field: "separators",
                reason: "entries must contain one to sixty-four bytes and no NUL",
            });
        }
        let mut unique = std::collections::HashSet::with_capacity(separators.len());
        if !separators
            .iter()
            .all(|separator| unique.insert(separator.as_str()))
        {
            return Err(WorkspaceChunkingError::InvalidConfiguration {
                field: "separators",
                reason: "entries must be unique",
            });
        }
        self.separators = separators
            .into_iter()
            .map(Arc::<str>::from)
            .collect::<Vec<_>>()
            .into();
        Ok(self)
    }

    pub fn separators(&self) -> &[Arc<str>] {
        &self.separators
    }
}

/// Built-in and host-injected workspace chunking strategies.
#[derive(Clone, Default)]
#[non_exhaustive]
pub enum WorkspaceChunkingStrategy {
    /// Compatibility default: at most `ChunkingConfig::max_lines` and
    /// `ChunkingConfig::max_bytes`, without overlap.
    #[default]
    Lines,
    FixedWindow(FixedWindowChunkingOptions),
    Recursive(RecursiveChunkingOptions),
    Custom(Arc<dyn CustomWorkspaceChunkingStrategy>),
}

impl WorkspaceChunkingStrategy {
    /// Wrap a trusted Rust host range splitter.
    pub fn custom(strategy: Arc<dyn CustomWorkspaceChunkingStrategy>) -> Self {
        Self::Custom(strategy)
    }

    pub(crate) fn split(
        &self,
        input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
        match self {
            Self::Lines => Ok(split_lines(input.content, input.limits)),
            Self::FixedWindow(options) => split_fixed(input.content, input.limits, *options),
            Self::Recursive(options) => split_recursive(input.content, input.limits, options),
            Self::Custom(strategy) => {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| strategy.split(input)))
                    .unwrap_or(Err(WorkspaceChunkingError::StrategyFailed))
            }
        }
    }

    pub(crate) fn validate_for(
        &self,
        limits: ChunkingConfig,
    ) -> Result<(), WorkspaceChunkingError> {
        match self {
            Self::Lines | Self::Custom(_) => Ok(()),
            Self::FixedWindow(options) => {
                validate_bounded_window(options.target_bytes, options.overlap_bytes, limits)
            }
            Self::Recursive(options) => {
                validate_bounded_window(options.target_bytes, options.overlap_bytes, limits)
            }
        }
    }
}

impl fmt::Debug for WorkspaceChunkingStrategy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lines => formatter.write_str("Lines"),
            Self::FixedWindow(options) => {
                formatter.debug_tuple("FixedWindow").field(options).finish()
            }
            Self::Recursive(options) => formatter.debug_tuple("Recursive").field(options).finish(),
            Self::Custom(_) => formatter.write_str("Custom(<host-injected>)"),
        }
    }
}

fn validate_window(
    target_bytes: usize,
    overlap_bytes: usize,
) -> Result<(), WorkspaceChunkingError> {
    if target_bytes < MIN_TARGET_BYTES {
        return Err(WorkspaceChunkingError::InvalidConfiguration {
            field: "target_bytes",
            reason: "must be at least four",
        });
    }
    if overlap_bytes >= target_bytes {
        return Err(WorkspaceChunkingError::InvalidConfiguration {
            field: "overlap_bytes",
            reason: "must be smaller than target_bytes",
        });
    }
    Ok(())
}

fn validate_target(
    target_bytes: usize,
    limits: ChunkingConfig,
) -> Result<(), WorkspaceChunkingError> {
    if target_bytes > limits.max_bytes {
        return Err(WorkspaceChunkingError::InvalidConfiguration {
            field: "target_bytes",
            reason: "must not exceed the catalog max_bytes limit",
        });
    }
    Ok(())
}

fn validate_bounded_window(
    target_bytes: usize,
    overlap_bytes: usize,
    limits: ChunkingConfig,
) -> Result<(), WorkspaceChunkingError> {
    validate_window(target_bytes, overlap_bytes)?;
    validate_target(target_bytes, limits)
}

fn default_recursive_separators() -> Arc<[Arc<str>]> {
    vec![
        Arc::<str>::from("\n\n"),
        Arc::<str>::from("\n"),
        Arc::<str>::from(". "),
        Arc::<str>::from("。"),
        Arc::<str>::from(" "),
    ]
    .into()
}

fn split_lines(content: &str, config: ChunkingConfig) -> Vec<WorkspaceChunkRange> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut pending_start = None;
    let mut pending_end = 0usize;
    let mut pending_lines = 0usize;
    let mut line_start = 0usize;

    for line in content.split_inclusive('\n') {
        let line_end = line_start + line.len();
        if line.len() > config.max_bytes {
            flush_pending_range(
                &mut ranges,
                &mut pending_start,
                pending_end,
                &mut pending_lines,
                config.max_chunks_per_file,
            );
            let mut segment_start = line_start;
            while segment_start < line_end && ranges.len() <= config.max_chunks_per_file {
                let segment_end = utf8_end(content, segment_start, line_end, config.max_bytes);
                ranges.push(WorkspaceChunkRange::new(segment_start, segment_end));
                segment_start = segment_end;
            }
        } else {
            let would_exceed_lines = pending_lines >= config.max_lines;
            let would_exceed_bytes = pending_start
                .is_some_and(|start| line_end.saturating_sub(start) > config.max_bytes);
            if would_exceed_lines || would_exceed_bytes {
                flush_pending_range(
                    &mut ranges,
                    &mut pending_start,
                    pending_end,
                    &mut pending_lines,
                    config.max_chunks_per_file,
                );
            }
            if pending_start.is_none() {
                pending_start = Some(line_start);
            }
            pending_end = line_end;
            pending_lines += 1;
        }
        line_start = line_end;
    }
    flush_pending_range(
        &mut ranges,
        &mut pending_start,
        pending_end,
        &mut pending_lines,
        config.max_chunks_per_file,
    );
    ranges
}

fn flush_pending_range(
    ranges: &mut Vec<WorkspaceChunkRange>,
    pending_start: &mut Option<usize>,
    pending_end: usize,
    pending_lines: &mut usize,
    max_chunks: usize,
) {
    let Some(start) = pending_start.take() else {
        return;
    };
    if ranges.len() <= max_chunks {
        ranges.push(WorkspaceChunkRange::new(start, pending_end));
    }
    *pending_lines = 0;
}

fn split_fixed(
    content: &str,
    limits: ChunkingConfig,
    options: FixedWindowChunkingOptions,
) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
    validate_bounded_window(options.target_bytes, options.overlap_bytes, limits)?;
    Ok(split_windows(
        content,
        limits.max_chunks_per_file,
        options.target_bytes,
        options.overlap_bytes,
        |_, hard_end| hard_end,
    ))
}

fn split_recursive(
    content: &str,
    limits: ChunkingConfig,
    options: &RecursiveChunkingOptions,
) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
    validate_bounded_window(options.target_bytes, options.overlap_bytes, limits)?;
    Ok(split_windows(
        content,
        limits.max_chunks_per_file,
        options.target_bytes,
        options.overlap_bytes,
        |start, hard_end| {
            if hard_end == content.len() {
                return hard_end;
            }
            let minimum_end = start.saturating_add(options.target_bytes / 2).max(
                start
                    .saturating_add(options.overlap_bytes)
                    .saturating_add(1),
            );
            options
                .separators
                .iter()
                .find_map(|separator| {
                    content[start..hard_end]
                        .rfind(separator.as_ref())
                        .map(|offset| start + offset + separator.len())
                        .filter(|end| *end >= minimum_end)
                })
                .unwrap_or(hard_end)
        },
    ))
}

fn split_windows(
    content: &str,
    max_chunks: usize,
    target_bytes: usize,
    overlap_bytes: usize,
    choose_end: impl Fn(usize, usize) -> usize,
) -> Vec<WorkspaceChunkRange> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < content.len() && ranges.len() <= max_chunks {
        let hard_end = utf8_end(content, start, content.len(), target_bytes);
        let end = choose_end(start, hard_end);
        ranges.push(WorkspaceChunkRange::new(start, end));
        if end == content.len() {
            break;
        }
        start = overlap_start(content, start, end, overlap_bytes);
    }
    ranges
}

fn overlap_start(content: &str, previous_start: usize, end: usize, overlap: usize) -> usize {
    let mut start = end.saturating_sub(overlap);
    while start < end && !content.is_char_boundary(start) {
        start += 1;
    }
    if start <= previous_start {
        content[previous_start..end]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| previous_start + offset)
            .unwrap_or(end)
    } else {
        start
    }
}

pub(crate) fn utf8_end(content: &str, start: usize, limit: usize, max_bytes: usize) -> usize {
    let mut end = start.saturating_add(max_bytes).min(limit);
    while end > start && !content.is_char_boundary(end) {
        end -= 1;
    }
    if end == start {
        content[start..limit]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| start + offset)
            .unwrap_or(limit)
    } else {
        end
    }
}

pub(crate) fn map_strategy_error(path: &str, error: WorkspaceChunkingError) -> WorkspaceIndexError {
    match error {
        WorkspaceChunkingError::InvalidConfiguration { field, reason } => {
            WorkspaceIndexError::InvalidConfig(format!("chunking {field} {reason}"))
        }
        WorkspaceChunkingError::StrategyFailed => WorkspaceIndexError::ChunkingStrategyFailed {
            path: path.to_owned(),
        },
    }
}
