use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, MemoryChangeSet, MemoryNamespace,
    MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus, RevisionMode,
    MAX_CHANGE_OPERATIONS,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

pub const TARGET_INDEX: usize = 4_242;
pub const DRIFT_INDEX: usize = 17;
pub const TARGET_QUERY: &str = "cobalt archway validation signal";

#[derive(Clone, Debug)]
pub struct CorpusSummary {
    pub content_bytes: usize,
    pub change_sets: usize,
    pub target_id: String,
    pub target_content: String,
}

#[derive(Debug)]
pub struct QualificationProvider {
    dimension: usize,
    target_content: String,
    calls: AtomicUsize,
    inputs: AtomicUsize,
}

impl QualificationProvider {
    pub fn new(dimension: usize, target_content: String) -> Self {
        Self {
            dimension,
            target_content,
            calls: AtomicUsize::new(0),
            inputs: AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn inputs(&self) -> usize {
        self.inputs.load(Ordering::SeqCst)
    }

    fn vector_for(&self, text: &str) -> Vec<f32> {
        let semantic_text = if text == TARGET_QUERY {
            &self.target_content
        } else {
            text
        };
        deterministic_unit_vector(semantic_text, self.dimension)
    }
}

#[async_trait]
impl EmbeddingProvider for QualificationProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new(
            "deterministic-qualification",
            "semantic-refresh-local-v1",
            self.dimension,
        )
        .with_revision("fixture-r1")
        .with_normalization(EmbeddingNormalization::Unit)
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs
            .fetch_add(request.inputs().len(), Ordering::SeqCst);
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), self.vector_for(input.text())))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

pub async fn seed_repository(
    repository: &dyn MemoryRepository,
    namespace: &MemoryNamespace,
    records: usize,
) -> Result<CorpusSummary> {
    if records <= TARGET_INDEX {
        anyhow::bail!("qualification corpus must include target index {TARGET_INDEX}");
    }
    let mut content_bytes = 0usize;
    let mut change_sets = 0usize;
    let mut target_content = None;
    for start in (0..records).step_by(MAX_CHANGE_OPERATIONS) {
        let end = (start + MAX_CHANGE_OPERATIONS).min(records);
        let timestamp = timestamp(i64::try_from(change_sets)?);
        let operations = (start..end)
            .map(|index| {
                let content = corpus_content(index);
                content_bytes = content_bytes.saturating_add(content.len());
                if index == TARGET_INDEX {
                    target_content = Some(content.clone());
                }
                let node = MemoryNodeDraft::new(
                    node_id(index),
                    namespace.clone(),
                    DurableMemoryKind::Semantic,
                    MemoryStatus::Active,
                    content,
                    vec![evidence(index, timestamp)?],
                    timestamp,
                )
                .with_confidence(0.9)
                .with_importance(0.7);
                Ok(MemoryOperation::Create { node })
            })
            .collect::<Result<Vec<_>>>()?;
        repository
            .apply(MemoryChangeSet::new(
                format!("seed-{start:05}-{end:05}"),
                namespace.clone(),
                timestamp,
                operations,
            ))
            .await
            .with_context(|| format!("could not persist corpus range {start}..{end}"))?;
        change_sets += 1;
    }
    Ok(CorpusSummary {
        content_bytes,
        change_sets,
        target_id: node_id(TARGET_INDEX),
        target_content: target_content.context("qualification target was not constructed")?,
    })
}

pub async fn revise_source_node(
    repository: &dyn MemoryRepository,
    namespace: &MemoryNamespace,
) -> Result<String> {
    let content = revised_content();
    let occurred_at = timestamp(10_000);
    repository
        .apply(MemoryChangeSet::new(
            "single-node-source-drift",
            namespace.clone(),
            occurred_at,
            vec![MemoryOperation::Revise {
                node_id: node_id(DRIFT_INDEX),
                expected_revision: 1,
                content: content.clone(),
                mode: RevisionMode::Correction,
                evidence: vec![EvidenceRef::try_new(
                    "a3s://qualification/source-drift",
                    format!("sha256:{}", "d".repeat(64)),
                    EvidenceKind::Verification,
                    occurred_at,
                )?],
                confidence: Some(0.95),
                importance: Some(0.8),
            }],
        ))
        .await
        .context("could not persist the single-node source drift")?;
    Ok(content)
}

fn corpus_content(index: usize) -> String {
    if index == TARGET_INDEX {
        return "The verified durable target requires rotating the amber recovery credential before gateway restoration.".to_string();
    }
    format!(
        "Durable corpus entry {index:05} records subsystem {:03}, protocol {:03}, and bounded operation {:03} for deterministic refresh qualification.",
        index % 257,
        (index * 17) % 389,
        (index * 31) % 521,
    )
}

fn revised_content() -> String {
    "Corrected durable corpus entry 00017 now requires protocol 311 before bounded operation 527."
        .to_string()
}

fn node_id(index: usize) -> String {
    format!("memory-{index:05}")
}

fn evidence(index: usize, occurred_at: DateTime<Utc>) -> Result<EvidenceRef> {
    EvidenceRef::try_new(
        format!("a3s://qualification/evidence/{index:05}"),
        format!("sha256:{index:064x}"),
        EvidenceKind::Verification,
        occurred_at,
    )
    .map_err(Into::into)
}

fn timestamp(offset_seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + TimeDelta::seconds(offset_seconds)
}

fn deterministic_unit_vector(text: &str, dimension: usize) -> Vec<f32> {
    let digest = Sha256::digest(text.as_bytes());
    let mut seed_bytes = [0u8; 8];
    seed_bytes.copy_from_slice(&digest[..8]);
    let mut seed = u64::from_le_bytes(seed_bytes);
    if seed == 0 {
        seed = 0x9e37_79b9_7f4a_7c15;
    }
    let mut values = Vec::with_capacity(dimension);
    let mut squared_norm = 0.0f64;
    for _ in 0..dimension {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let mixed = seed.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let unit = ((mixed >> 40) as f32) / ((1u32 << 24) - 1) as f32;
        let value = unit.mul_add(2.0, -1.0);
        squared_norm += f64::from(value) * f64::from(value);
        values.push(value);
    }
    let norm = squared_norm.sqrt() as f32;
    for value in &mut values {
        *value /= norm;
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_vectors_are_unit_length_and_query_maps_to_target() {
        let target = corpus_content(TARGET_INDEX);
        let provider = QualificationProvider::new(384, target.clone());
        let target_vector = provider.vector_for(&target);
        let query_vector = provider.vector_for(TARGET_QUERY);
        assert_eq!(target_vector, query_vector);
        let norm = target_vector
            .iter()
            .map(|value| f64::from(*value) * f64::from(*value))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
