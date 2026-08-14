use super::WorkspaceChunk;
use crate::embedding::{EmbeddingExecutorConfig, EmbeddingInput};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SemanticBatchFlushReason {
    InputLimit,
    TextByteLimit,
    VectorByteLimit,
    GenerationComplete,
}

pub(super) struct SemanticBatchEntry {
    pub(super) path: String,
    pub(super) input: EmbeddingInput,
}

pub(super) struct SemanticBatch {
    pub(super) entries: Vec<SemanticBatchEntry>,
    pub(super) flush_reason: SemanticBatchFlushReason,
}

pub(super) struct SemanticBatchPlan {
    pub(super) batches: Vec<SemanticBatch>,
    pub(super) document_inputs: usize,
    pub(super) document_text_bytes: usize,
    pub(super) batch_limit_lower_bound: usize,
}

pub(super) fn plan_semantic_batches<'a>(
    partitions: impl IntoIterator<Item = (&'a str, &'a [Arc<WorkspaceChunk>])>,
    dimension: usize,
    config: EmbeddingExecutorConfig,
) -> SemanticBatchPlan {
    let vector_bytes_per_input = dimension.saturating_mul(std::mem::size_of::<f32>());
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_text_bytes = 0usize;
    let mut document_inputs = 0usize;
    let mut document_text_bytes = 0usize;

    for (path, chunks) in partitions {
        for chunk in chunks {
            let input =
                EmbeddingInput::new(Arc::<str>::from(chunk.id.as_str()), Arc::clone(&chunk.text));
            document_inputs = document_inputs.saturating_add(1);
            document_text_bytes = document_text_bytes.saturating_add(input.text_bytes());

            if let Some(reason) = exceeded_reason(
                current.len(),
                current_text_bytes,
                input.text_bytes(),
                vector_bytes_per_input,
                config,
            ) {
                flush(&mut batches, &mut current, reason);
                current_text_bytes = 0;
            }

            current_text_bytes = current_text_bytes.saturating_add(input.text_bytes());
            current.push(SemanticBatchEntry {
                path: path.to_owned(),
                input,
            });

            if let Some(reason) = reached_reason(
                current.len(),
                current_text_bytes,
                vector_bytes_per_input,
                config,
            ) {
                flush(&mut batches, &mut current, reason);
                current_text_bytes = 0;
            }
        }
    }

    flush(
        &mut batches,
        &mut current,
        SemanticBatchFlushReason::GenerationComplete,
    );
    let vector_bytes = vector_bytes_per_input.saturating_mul(document_inputs);
    SemanticBatchPlan {
        batches,
        document_inputs,
        document_text_bytes,
        batch_limit_lower_bound: document_inputs
            .div_ceil(config.max_batch_inputs)
            .max(document_text_bytes.div_ceil(config.max_batch_text_bytes))
            .max(vector_bytes.div_ceil(config.max_batch_vector_bytes)),
    }
}

fn exceeded_reason(
    current_inputs: usize,
    current_text_bytes: usize,
    next_text_bytes: usize,
    vector_bytes_per_input: usize,
    config: EmbeddingExecutorConfig,
) -> Option<SemanticBatchFlushReason> {
    if current_inputs == 0 {
        return None;
    }
    if current_inputs.saturating_add(1) > config.max_batch_inputs {
        return Some(SemanticBatchFlushReason::InputLimit);
    }
    if current_text_bytes.saturating_add(next_text_bytes) > config.max_batch_text_bytes {
        return Some(SemanticBatchFlushReason::TextByteLimit);
    }
    if vector_bytes_per_input.saturating_mul(current_inputs.saturating_add(1))
        > config.max_batch_vector_bytes
    {
        return Some(SemanticBatchFlushReason::VectorByteLimit);
    }
    None
}

fn reached_reason(
    current_inputs: usize,
    current_text_bytes: usize,
    vector_bytes_per_input: usize,
    config: EmbeddingExecutorConfig,
) -> Option<SemanticBatchFlushReason> {
    if current_inputs == config.max_batch_inputs {
        return Some(SemanticBatchFlushReason::InputLimit);
    }
    if current_text_bytes == config.max_batch_text_bytes {
        return Some(SemanticBatchFlushReason::TextByteLimit);
    }
    if vector_bytes_per_input.saturating_mul(current_inputs) == config.max_batch_vector_bytes {
        return Some(SemanticBatchFlushReason::VectorByteLimit);
    }
    None
}

fn flush(
    batches: &mut Vec<SemanticBatch>,
    current: &mut Vec<SemanticBatchEntry>,
    flush_reason: SemanticBatchFlushReason,
) {
    if current.is_empty() {
        return;
    }
    batches.push(SemanticBatch {
        entries: std::mem::take(current),
        flush_reason,
    });
}
