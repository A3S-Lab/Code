use super::chunking_strategy::{map_strategy_error, WorkspaceChunkRange};
use super::types::{
    ChunkingConfig, WorkspaceChunk, WorkspaceChunkId, WorkspaceIndexError, WorkspaceIndexResult,
};
use super::{WorkspaceChunkingInput, WorkspaceChunkingStrategy};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(crate) struct ChunkFileRequest<'a> {
    pub path: &'a str,
    pub language: Option<&'a str>,
    pub source_revision: u64,
    pub content: &'a str,
}

#[derive(Debug)]
pub(crate) struct ChunkedFile {
    pub content_digest: Arc<str>,
    pub chunks: Vec<Arc<WorkspaceChunk>>,
    /// Retained chunk text bytes, including intentional overlap.
    pub text_bytes: usize,
}

#[cfg(test)]
pub(crate) fn chunk_file(
    request: ChunkFileRequest<'_>,
    config: ChunkingConfig,
) -> WorkspaceIndexResult<ChunkedFile> {
    chunk_file_with_strategy(request, config, &WorkspaceChunkingStrategy::Lines)
}

pub(crate) fn chunk_file_with_strategy(
    request: ChunkFileRequest<'_>,
    config: ChunkingConfig,
    strategy: &WorkspaceChunkingStrategy,
) -> WorkspaceIndexResult<ChunkedFile> {
    let config = config.validate()?;
    let content_digest = digest_content(request.content);
    let ranges = strategy
        .split(WorkspaceChunkingInput {
            path: request.path,
            language: request.language,
            content: request.content,
            limits: config,
        })
        .map_err(|error| map_strategy_error(request.path, error))?;
    validate_ranges(request.path, request.content, &ranges, config)?;

    let path: Arc<str> = Arc::from(request.path);
    let language = request.language.map(Arc::from);
    let line_ranges = line_ranges(request.content, &ranges);
    let chunks = ranges
        .into_iter()
        .zip(line_ranges)
        .map(|(range, (start_line, end_line))| {
            let text: Arc<str> = Arc::from(&request.content[range.start_byte..range.end_byte]);
            let id = chunk_id(
                request.path,
                &content_digest,
                range.start_byte,
                range.end_byte,
            );
            Arc::new(WorkspaceChunk {
                id,
                path: Arc::clone(&path),
                language: language.clone(),
                start_line,
                end_line,
                start_byte: range.start_byte,
                end_byte: range.end_byte,
                content_digest: Arc::clone(&content_digest),
                source_revision: request.source_revision,
                text,
            })
        })
        .collect::<Vec<_>>();
    let text_bytes = chunks.iter().fold(0usize, |total, chunk| {
        total.saturating_add(chunk.text.len())
    });

    Ok(ChunkedFile {
        content_digest,
        chunks,
        text_bytes,
    })
}

fn validate_ranges(
    path: &str,
    content: &str,
    ranges: &[WorkspaceChunkRange],
    config: ChunkingConfig,
) -> WorkspaceIndexResult<()> {
    if ranges.len() > config.max_chunks_per_file {
        return Err(WorkspaceIndexError::TooManyChunks {
            path: path.to_owned(),
            limit: config.max_chunks_per_file,
        });
    }
    if content.is_empty() {
        return if ranges.is_empty() {
            Ok(())
        } else {
            Err(invalid_ranges(path, "empty source must produce no ranges"))
        };
    }
    if ranges.is_empty() {
        return Err(invalid_ranges(
            path,
            "non-empty source must produce at least one range",
        ));
    }

    let mut previous_start = None;
    let mut previous_end = 0usize;
    for (index, range) in ranges.iter().enumerate() {
        if range.start_byte >= range.end_byte || range.end_byte > content.len() {
            return Err(invalid_ranges(
                path,
                "ranges must be non-empty and in bounds",
            ));
        }
        if !content.is_char_boundary(range.start_byte) || !content.is_char_boundary(range.end_byte)
        {
            return Err(invalid_ranges(path, "ranges must use UTF-8 boundaries"));
        }
        if range.end_byte.saturating_sub(range.start_byte) > config.max_bytes {
            return Err(invalid_ranges(path, "a range exceeds max_bytes"));
        }
        if index == 0 && range.start_byte != 0 {
            return Err(invalid_ranges(path, "ranges must start at byte zero"));
        }
        if let Some(start) = previous_start {
            if range.start_byte <= start || range.end_byte <= previous_end {
                return Err(invalid_ranges(path, "ranges must make forward progress"));
            }
            if range.start_byte > previous_end {
                return Err(invalid_ranges(path, "ranges must not leave gaps"));
            }
        }
        previous_start = Some(range.start_byte);
        previous_end = range.end_byte;
    }
    if previous_end != content.len() {
        return Err(invalid_ranges(
            path,
            "ranges must cover the complete source",
        ));
    }
    Ok(())
}

fn invalid_ranges(path: &str, reason: &'static str) -> WorkspaceIndexError {
    WorkspaceIndexError::InvalidChunkRanges {
        path: path.to_owned(),
        reason,
    }
}

fn line_ranges(content: &str, ranges: &[WorkspaceChunkRange]) -> Vec<(usize, usize)> {
    let mut events = Vec::with_capacity(ranges.len().saturating_mul(2));
    for (index, range) in ranges.iter().enumerate() {
        events.push((range.start_byte, index, false));
        events.push((range.end_byte.saturating_sub(1), index, true));
    }
    events.sort_unstable();

    let bytes = content.as_bytes();
    let mut line = 1usize;
    let mut cursor = 0usize;
    let mut anchors = vec![(1usize, 1usize); ranges.len()];
    for (byte, index, is_end) in events {
        line = line.saturating_add(
            bytes[cursor..byte]
                .iter()
                .filter(|value| **value == b'\n')
                .count(),
        );
        if is_end {
            anchors[index].1 = line;
        } else {
            anchors[index].0 = line;
        }
        cursor = byte;
    }
    anchors
}

fn chunk_id(
    path: &str,
    content_digest: &str,
    start_byte: usize,
    end_byte: usize,
) -> WorkspaceChunkId {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.workspace.chunk.v1\0");
    hasher.update(path.as_bytes());
    hasher.update(b"\0");
    hasher.update(content_digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(start_byte.to_le_bytes());
    hasher.update(end_byte.to_le_bytes());
    WorkspaceChunkId::new(format!("sha256:{:x}", hasher.finalize()))
}

pub(crate) fn digest_content(content: &str) -> Arc<str> {
    Arc::from(format!("sha256:{:x}", Sha256::digest(content.as_bytes())))
}
