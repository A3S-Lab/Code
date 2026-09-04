//! Generic evaluation result contracts and a bounded result store.

use super::identity::{digest_json, validate_digest, ExecutionTargetV1};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use thiserror::Error;

pub const EVALUATION_RESULT_SCHEMA_V1: &str = "a3s.code.evaluation-result.v1";
pub const EVALUATION_RECORD_SCHEMA_V1: &str = "a3s.code.evaluation-record.v1";
const MAX_EVALUATOR_ID_BYTES: usize = 256;
const MAX_DECISION_BYTES: usize = 128;
const MAX_SUMMARY_BYTES: usize = 16 * 1024;

/// A host-defined outcome token.  Core validates shape and provenance only;
/// it deliberately does not enumerate reviewer/business dispositions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationResultV1 {
    pub schema: String,
    pub evaluator_id: String,
    pub target: ExecutionTargetV1,
    pub auxiliary_run_id: String,
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_bps: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub payload: serde_json::Value,
    pub evidence_digest: String,
    pub result_digest: String,
}

impl EvaluationResultV1 {
    pub fn new(
        evaluator_id: impl Into<String>,
        target: ExecutionTargetV1,
        auxiliary_run_id: impl Into<String>,
        decision: impl Into<String>,
        payload: serde_json::Value,
        evidence_digest: impl Into<String>,
    ) -> Result<Self, EvaluationStoreError> {
        let mut result = Self {
            schema: EVALUATION_RESULT_SCHEMA_V1.to_string(),
            evaluator_id: evaluator_id.into(),
            target,
            auxiliary_run_id: auxiliary_run_id.into(),
            decision: decision.into(),
            confidence_bps: None,
            summary: None,
            payload,
            evidence_digest: evidence_digest.into(),
            result_digest: String::new(),
        };
        result.validate_without_digest()?;
        result.result_digest = result.expected_digest()?;
        Ok(result)
    }

    pub fn with_confidence(mut self, confidence_bps: u16) -> Self {
        // Keep invalid caller input visible to `validate` instead of silently
        // changing a host decision.  The builder remains infallible for
        // ergonomics, while `finalize`/store admission fail closed.
        self.confidence_bps = Some(confidence_bps);
        self.result_digest = String::new();
        self
    }

    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self.result_digest = String::new();
        self
    }

    /// Recompute the result digest after using one of the builder methods.
    pub fn finalize(mut self) -> Result<Self, EvaluationStoreError> {
        self.validate_without_digest()?;
        self.result_digest = self.expected_digest()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), EvaluationStoreError> {
        self.validate_without_digest()?;
        validate_digest(&self.result_digest)
            .map_err(|_| EvaluationStoreError::InvalidField("result_digest"))?;
        if self.result_digest != self.expected_digest()? {
            return Err(EvaluationStoreError::DigestMismatch("result_digest"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, EvaluationStoreError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            evaluator_id: &'a str,
            target: &'a ExecutionTargetV1,
            auxiliary_run_id: &'a str,
            decision: &'a str,
            confidence_bps: Option<u16>,
            summary: Option<&'a str>,
            payload: &'a serde_json::Value,
            evidence_digest: &'a str,
        }
        digest_json(
            "a3s.code.evaluation-result.identity.v1",
            &Identity {
                schema: &self.schema,
                evaluator_id: &self.evaluator_id,
                target: &self.target,
                auxiliary_run_id: &self.auxiliary_run_id,
                decision: &self.decision,
                confidence_bps: self.confidence_bps,
                summary: self.summary.as_deref(),
                payload: &self.payload,
                evidence_digest: &self.evidence_digest,
            },
        )
        .map_err(|error| EvaluationStoreError::Serialization(error.to_string()))
    }

    fn validate_without_digest(&self) -> Result<(), EvaluationStoreError> {
        if self.schema != EVALUATION_RESULT_SCHEMA_V1 {
            return Err(EvaluationStoreError::UnsupportedSchema);
        }
        self.target
            .validate()
            .map_err(|_| EvaluationStoreError::InvalidField("target"))?;
        validate_text("evaluator_id", &self.evaluator_id, MAX_EVALUATOR_ID_BYTES)?;
        validate_text(
            "auxiliary_run_id",
            &self.auxiliary_run_id,
            MAX_EVALUATOR_ID_BYTES,
        )?;
        validate_text("decision", &self.decision, MAX_DECISION_BYTES)?;
        if self.confidence_bps.is_some_and(|value| value > 10_000) {
            return Err(EvaluationStoreError::InvalidField("confidence_bps"));
        }
        if self
            .summary
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SUMMARY_BYTES || value.contains('\0'))
        {
            return Err(EvaluationStoreError::InvalidField("summary"));
        }
        validate_digest(&self.evidence_digest)
            .map_err(|_| EvaluationStoreError::InvalidField("evidence_digest"))?;
        let payload_bytes = serde_json::to_vec(&self.payload)
            .map_err(|error| EvaluationStoreError::Serialization(error.to_string()))?
            .len();
        if payload_bytes > MAX_SUMMARY_BYTES * 16 {
            return Err(EvaluationStoreError::InvalidField("payload"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationRecordV1 {
    pub schema: String,
    pub result: EvaluationResultV1,
    pub observed_at_ms: u64,
    pub record_digest: String,
}

impl EvaluationRecordV1 {
    pub fn new(
        result: EvaluationResultV1,
        observed_at_ms: u64,
    ) -> Result<Self, EvaluationStoreError> {
        result.validate()?;
        if observed_at_ms == 0 {
            return Err(EvaluationStoreError::InvalidField("observed_at_ms"));
        }
        let mut record = Self {
            schema: EVALUATION_RECORD_SCHEMA_V1.to_string(),
            result,
            observed_at_ms,
            record_digest: String::new(),
        };
        record.record_digest = record.expected_digest()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), EvaluationStoreError> {
        if self.schema != EVALUATION_RECORD_SCHEMA_V1 {
            return Err(EvaluationStoreError::UnsupportedSchema);
        }
        self.result.validate()?;
        if self.observed_at_ms == 0 {
            return Err(EvaluationStoreError::InvalidField("observed_at_ms"));
        }
        validate_digest(&self.record_digest)
            .map_err(|_| EvaluationStoreError::InvalidField("record_digest"))?;
        if self.record_digest != self.expected_digest()? {
            return Err(EvaluationStoreError::DigestMismatch("record_digest"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, EvaluationStoreError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            result: &'a EvaluationResultV1,
            observed_at_ms: u64,
        }
        digest_json(
            "a3s.code.evaluation-record.identity.v1",
            &Identity {
                schema: &self.schema,
                result: &self.result,
                observed_at_ms: self.observed_at_ms,
            },
        )
        .map_err(|error| EvaluationStoreError::Serialization(error.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationWriteOutcomeV1 {
    pub written: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvaluationStoreError {
    #[error("evaluation result schema is unsupported")]
    UnsupportedSchema,
    #[error("evaluation result field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("evaluation result digest for `{0}` does not match")]
    DigestMismatch(&'static str),
    #[error("evaluation result conflicts with an existing record")]
    Conflict,
    #[error("evaluation result serialization failed: {0}")]
    Serialization(String),
}

#[async_trait]
pub trait EvaluationResultSink: Send + Sync {
    async fn write(
        &self,
        record: EvaluationRecordV1,
    ) -> Result<EvaluationWriteOutcomeV1, EvaluationStoreError>;
    async fn get(&self, record_digest: &str) -> Option<EvaluationRecordV1>;
    async fn list_for_target(&self, target: &ExecutionTargetV1) -> Vec<EvaluationRecordV1>;
}

#[derive(Debug, Default)]
struct ResultState {
    by_digest: HashMap<String, EvaluationRecordV1>,
    by_identity: HashMap<EvaluationIdentityKey, String>,
    order: VecDeque<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EvaluationIdentityKey {
    target: ExecutionTargetV1,
    evaluator_id: String,
    auxiliary_run_id: String,
}

/// In-memory CAS result sink.  A durable host can implement the same trait
/// with an append-only object store and an external retention policy.
#[derive(Debug, Clone)]
pub struct InMemoryEvaluationResultStore {
    state: Arc<RwLock<ResultState>>,
    max_records: Option<usize>,
}

impl InMemoryEvaluationResultStore {
    pub fn new() -> Self {
        Self::with_max_records(None)
    }

    pub fn with_max_records(max_records: Option<usize>) -> Self {
        Self {
            state: Arc::new(RwLock::new(ResultState::default())),
            max_records,
        }
    }
}

impl Default for InMemoryEvaluationResultStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EvaluationResultSink for InMemoryEvaluationResultStore {
    async fn write(
        &self,
        record: EvaluationRecordV1,
    ) -> Result<EvaluationWriteOutcomeV1, EvaluationStoreError> {
        record.validate()?;
        let digest = record.record_digest.clone();
        let mut state = self
            .state
            .write()
            .map_err(|_| EvaluationStoreError::Conflict)?;
        if let Some(existing) = state.by_digest.get(&digest) {
            if existing == &record {
                return Ok(EvaluationWriteOutcomeV1 {
                    written: false,
                    replayed: true,
                });
            }
            return Err(EvaluationStoreError::Conflict);
        }
        let identity = EvaluationIdentityKey {
            target: record.result.target.clone(),
            evaluator_id: record.result.evaluator_id.clone(),
            auxiliary_run_id: record.result.auxiliary_run_id.clone(),
        };
        if state.by_identity.contains_key(&identity) {
            // A single evaluator/auxiliary pair is immutable.  This catches a
            // conflicting result even when the caller recomputed a different
            // valid content digest, while exact replay above remains
            // idempotent.
            return Err(EvaluationStoreError::Conflict);
        }
        state.order.push_back(digest.clone());
        state.by_identity.insert(identity, digest.clone());
        state.by_digest.insert(digest, record);
        if let Some(limit) = self.max_records {
            while state.order.len() > limit {
                if let Some(oldest) = state.order.pop_front() {
                    if let Some(removed) = state.by_digest.remove(&oldest) {
                        let identity = EvaluationIdentityKey {
                            target: removed.result.target,
                            evaluator_id: removed.result.evaluator_id,
                            auxiliary_run_id: removed.result.auxiliary_run_id,
                        };
                        if state
                            .by_identity
                            .get(&identity)
                            .is_some_and(|digest| digest == &oldest)
                        {
                            state.by_identity.remove(&identity);
                        }
                    }
                }
            }
        }
        Ok(EvaluationWriteOutcomeV1 {
            written: true,
            replayed: false,
        })
    }

    async fn get(&self, record_digest: &str) -> Option<EvaluationRecordV1> {
        self.state
            .read()
            .ok()?
            .by_digest
            .get(record_digest)
            .cloned()
    }

    async fn list_for_target(&self, target: &ExecutionTargetV1) -> Vec<EvaluationRecordV1> {
        let Ok(state) = self.state.read() else {
            return Vec::new();
        };
        state
            .order
            .iter()
            .filter_map(|digest| state.by_digest.get(digest))
            .filter(|record| record.result.target == *target)
            .cloned()
            .collect()
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), EvaluationStoreError> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.contains('\0')
        || value.lines().count() != 1
    {
        return Err(EvaluationStoreError::InvalidField(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> ExecutionTargetV1 {
        ExecutionTargetV1::new("session-1", "run-1")
    }

    fn result() -> EvaluationResultV1 {
        EvaluationResultV1::new(
            "fixture-evaluator",
            target(),
            "aux-1",
            "inconclusive",
            serde_json::json!({"issues": []}),
            super::super::identity::digest_bytes("evidence", b"fixture"),
        )
        .unwrap()
    }

    #[test]
    fn result_builder_requires_finalization_after_mutation() {
        let result = result().with_summary("bounded summary");
        assert!(result.validate().is_err());
        let result = result.finalize().unwrap();
        assert!(result.validate().is_ok());
    }

    #[tokio::test]
    async fn result_store_is_idempotent_and_lists_by_target() {
        let store = InMemoryEvaluationResultStore::new();
        let record = EvaluationRecordV1::new(result(), 1).unwrap();
        let first = store.write(record.clone()).await.unwrap();
        assert!(first.written);
        let replay = store.write(record.clone()).await.unwrap();
        assert!(replay.replayed);
        assert_eq!(store.list_for_target(&target()).await, vec![record.clone()]);
        assert_eq!(store.get(&record.record_digest).await, Some(record));
    }

    #[test]
    fn host_decision_is_open_text_not_a_core_enum() {
        let mut result = result();
        result.decision = "product-specific-token".to_string();
        result = result.finalize().unwrap();
        assert!(result.validate().is_ok());
    }

    #[test]
    fn confidence_overflow_is_rejected_instead_of_clamped() {
        let result = result().with_confidence(10_001).finalize();
        assert!(matches!(
            result,
            Err(EvaluationStoreError::InvalidField("confidence_bps"))
        ));
    }

    #[tokio::test]
    async fn result_store_rejects_conflicting_identity() {
        let store = InMemoryEvaluationResultStore::new();
        let first = EvaluationRecordV1::new(result(), 1).unwrap();
        store.write(first).await.unwrap();
        let second_result = EvaluationResultV1::new(
            "fixture-evaluator",
            target(),
            "aux-1",
            "inconclusive",
            serde_json::json!({"issues": ["different"]}),
            super::super::identity::digest_bytes("evidence", b"fixture"),
        )
        .unwrap();
        let second = EvaluationRecordV1::new(second_result, 2).unwrap();
        assert!(matches!(
            store.write(second).await,
            Err(EvaluationStoreError::Conflict)
        ));
    }
}
