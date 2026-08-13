use super::types::{
    ChunkingConfig, WorkspaceChunk, WorkspaceChunkId, WorkspaceIndexError, WorkspaceIndexResult,
};
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
    pub text_bytes: usize,
}

pub(crate) fn chunk_file(
    request: ChunkFileRequest<'_>,
    config: ChunkingConfig,
) -> WorkspaceIndexResult<ChunkedFile> {
    let config = config.validate()?;
    let content_digest = digest_content(request.content);
    if request.content.is_empty() {
        return Ok(ChunkedFile {
            content_digest,
            chunks: Vec::new(),
            text_bytes: 0,
        });
    }

    let mut ranges = Vec::<ChunkRange>::new();
    let mut pending_start = None;
    let mut pending_end = 0usize;
    let mut pending_start_line = 1usize;
    let mut pending_end_line = 1usize;
    let mut pending_lines = 0usize;
    let mut line_start = 0usize;

    for (line_index, line) in request.content.split_inclusive('\n').enumerate() {
        let line_number = line_index + 1;
        let line_end = line_start + line.len();
        if line.len() > config.max_bytes {
            flush_pending(
                &mut ranges,
                &mut pending_start,
                &mut pending_end,
                &mut pending_start_line,
                &mut pending_end_line,
                &mut pending_lines,
                request.path,
                config.max_chunks_per_file,
            )?;
            let mut segment_start = line_start;
            while segment_start < line_end {
                let segment_end =
                    utf8_chunk_end(request.content, segment_start, line_end, config.max_bytes);
                push_range(
                    &mut ranges,
                    ChunkRange {
                        start_byte: segment_start,
                        end_byte: segment_end,
                        start_line: line_number,
                        end_line: line_number,
                    },
                    request.path,
                    config.max_chunks_per_file,
                )?;
                segment_start = segment_end;
            }
        } else {
            let would_exceed_lines = pending_lines >= config.max_lines;
            let would_exceed_bytes = pending_start
                .is_some_and(|start| line_end.saturating_sub(start) > config.max_bytes);
            if would_exceed_lines || would_exceed_bytes {
                flush_pending(
                    &mut ranges,
                    &mut pending_start,
                    &mut pending_end,
                    &mut pending_start_line,
                    &mut pending_end_line,
                    &mut pending_lines,
                    request.path,
                    config.max_chunks_per_file,
                )?;
            }
            if pending_start.is_none() {
                pending_start = Some(line_start);
                pending_start_line = line_number;
            }
            pending_end = line_end;
            pending_end_line = line_number;
            pending_lines += 1;
        }
        line_start = line_end;
    }
    flush_pending(
        &mut ranges,
        &mut pending_start,
        &mut pending_end,
        &mut pending_start_line,
        &mut pending_end_line,
        &mut pending_lines,
        request.path,
        config.max_chunks_per_file,
    )?;

    let path: Arc<str> = Arc::from(request.path);
    let language = request.language.map(Arc::from);
    let chunks = ranges
        .into_iter()
        .map(|range| {
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
                start_line: range.start_line,
                end_line: range.end_line,
                start_byte: range.start_byte,
                end_byte: range.end_byte,
                content_digest: Arc::clone(&content_digest),
                source_revision: request.source_revision,
                text,
            })
        })
        .collect();

    Ok(ChunkedFile {
        content_digest,
        chunks,
        text_bytes: request.content.len(),
    })
}

#[derive(Clone, Copy)]
struct ChunkRange {
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
}

#[allow(clippy::too_many_arguments)]
fn flush_pending(
    ranges: &mut Vec<ChunkRange>,
    pending_start: &mut Option<usize>,
    pending_end: &mut usize,
    pending_start_line: &mut usize,
    pending_end_line: &mut usize,
    pending_lines: &mut usize,
    path: &str,
    max_chunks: usize,
) -> WorkspaceIndexResult<()> {
    let Some(start_byte) = pending_start.take() else {
        return Ok(());
    };
    push_range(
        ranges,
        ChunkRange {
            start_byte,
            end_byte: *pending_end,
            start_line: *pending_start_line,
            end_line: *pending_end_line,
        },
        path,
        max_chunks,
    )?;
    *pending_lines = 0;
    Ok(())
}

fn push_range(
    ranges: &mut Vec<ChunkRange>,
    range: ChunkRange,
    path: &str,
    max_chunks: usize,
) -> WorkspaceIndexResult<()> {
    if ranges.len() >= max_chunks {
        return Err(WorkspaceIndexError::TooManyChunks {
            path: path.to_owned(),
            limit: max_chunks,
        });
    }
    ranges.push(range);
    Ok(())
}

fn utf8_chunk_end(content: &str, start: usize, line_end: usize, max_bytes: usize) -> usize {
    let mut end = start.saturating_add(max_bytes).min(line_end);
    while end > start && !content.is_char_boundary(end) {
        end -= 1;
    }
    if end == start {
        content[start..line_end]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| start + offset)
            .unwrap_or(line_end)
    } else {
        end
    }
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
