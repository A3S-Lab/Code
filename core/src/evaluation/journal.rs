//! Append-only, bounded execution facts.

use super::identity::{
    digest_bytes, digest_json, validate_digest, ExecutionFrameV1, ExecutionTargetV1,
};
use crate::agent::AgentEvent;
use crate::run::RunEventRecord;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use thiserror::Error;

pub const EXECUTION_FACT_SCHEMA_V1: &str = "a3s.code.execution-fact.v1";
const MAX_EVENT_TYPE_BYTES: usize = 256;
const MAX_ARTIFACT_REFS: usize = 128;
const MAX_ARTIFACT_URI_BYTES: usize = 1024;
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionFactKindV1 {
    Lifecycle,
    Turn,
    Model,
    Tool,
    Permission,
    Context,
    Child,
    Control,
    Memory,
    Other,
}

impl ExecutionFactKindV1 {
    pub(crate) fn from_event_type(event_type: &str) -> Self {
        if event_type == "agent_start"
            || event_type == "agent_end"
            || event_type == "error"
            || event_type == "agent_mode_changed"
        {
            return Self::Lifecycle;
        }
        if event_type == "turn_start" || event_type == "turn_end" {
            return Self::Turn;
        }
        if event_type.starts_with("model_") {
            return Self::Model;
        }
        if event_type.starts_with("tool_") {
            return Self::Tool;
        }
        if event_type == "permission_denied" || event_type.starts_with("confirmation_") {
            return Self::Permission;
        }
        if event_type.starts_with("context_") || event_type == "cognitive_context_bound" {
            return Self::Context;
        }
        if event_type.starts_with("subagent_") {
            return Self::Child;
        }
        if event_type.starts_with("memory") || event_type == "memories_searched" {
            return Self::Memory;
        }
        if event_type == "run_control_applied"
            || event_type == "external_task_pending"
            || event_type == "external_task_completed"
            || event_type == "persistence_failed"
        {
            return Self::Control;
        }
        Self::Other
    }
}

/// Input accepted by the journal.  Payload bytes are counted and represented
/// by a digest; raw model/tool content belongs in an explicitly authorized
/// artifact store or evidence reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionFactInputV1 {
    pub frame: ExecutionFrameV1,
    pub sequence: u64,
    pub observed_at_ms: u64,
    pub event_type: String,
    pub payload_digest: String,
    pub payload_bytes: u64,
    pub artifact_refs: Vec<String>,
}

impl ExecutionFactInputV1 {
    pub fn from_event(
        frame: ExecutionFrameV1,
        sequence: u64,
        observed_at_ms: u64,
        event: &AgentEvent,
    ) -> Result<Self, JournalError> {
        let encoded = serde_json::to_vec(event)
            .map_err(|error| JournalError::Serialization(error.to_string()))?;
        let event_type = event.event_type_v1().to_string();
        let value = serde_json::to_value(event)
            .map_err(|error| JournalError::Serialization(error.to_string()))?;
        Ok(Self {
            frame,
            sequence,
            observed_at_ms,
            event_type,
            payload_digest: digest_bytes("a3s.code.execution-fact.payload.v1", &encoded),
            payload_bytes: u64::try_from(encoded.len())
                .map_err(|_| JournalError::InvalidField("payload_bytes"))?,
            artifact_refs: collect_artifact_refs(&value),
        })
    }

    pub fn from_run_event(
        frame: ExecutionFrameV1,
        event: &RunEventRecord,
    ) -> Result<Self, JournalError> {
        let sequence =
            u64::try_from(event.sequence).map_err(|_| JournalError::InvalidField("sequence"))?;
        Self::from_event(frame, sequence, event.timestamp_ms, &event.event)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFactV1 {
    pub schema: String,
    pub frame: ExecutionFrameV1,
    pub sequence: u64,
    pub observed_at_ms: u64,
    pub event_type: String,
    pub kind: ExecutionFactKindV1,
    pub payload_digest: String,
    pub payload_bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<String>,
    pub fact_digest: String,
}

impl ExecutionFactV1 {
    pub fn from_run_event(
        frame: ExecutionFrameV1,
        record: &RunEventRecord,
    ) -> Result<Self, JournalError> {
        let input = ExecutionFactInputV1::from_run_event(frame, record)?;
        Self::from_input(input)
    }

    pub fn from_input(input: ExecutionFactInputV1) -> Result<Self, JournalError> {
        let mut fact = Self {
            schema: EXECUTION_FACT_SCHEMA_V1.to_string(),
            frame: input.frame,
            sequence: input.sequence,
            observed_at_ms: input.observed_at_ms,
            kind: ExecutionFactKindV1::from_event_type(&input.event_type),
            event_type: input.event_type,
            payload_digest: input.payload_digest,
            payload_bytes: input.payload_bytes,
            artifact_refs: input.artifact_refs,
            fact_digest: String::new(),
        };
        fact.validate_without_digest()?;
        fact.fact_digest = fact.expected_digest()?;
        Ok(fact)
    }

    pub fn validate(&self) -> Result<(), JournalError> {
        self.validate_without_digest()?;
        validate_digest(&self.fact_digest)
            .map_err(|_| JournalError::InvalidField("fact_digest"))?;
        if self.fact_digest != self.expected_digest()? {
            return Err(JournalError::DigestMismatch("fact_digest"));
        }
        Ok(())
    }

    pub fn expected_digest(&self) -> Result<String, JournalError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            frame: &'a ExecutionFrameV1,
            sequence: u64,
            observed_at_ms: u64,
            event_type: &'a str,
            kind: ExecutionFactKindV1,
            payload_digest: &'a str,
            payload_bytes: u64,
            artifact_refs: &'a [String],
        }
        digest_json(
            "a3s.code.execution-fact.identity.v1",
            &Identity {
                schema: &self.schema,
                frame: &self.frame,
                sequence: self.sequence,
                observed_at_ms: self.observed_at_ms,
                event_type: &self.event_type,
                kind: self.kind,
                payload_digest: &self.payload_digest,
                payload_bytes: self.payload_bytes,
                artifact_refs: &self.artifact_refs,
            },
        )
        .map_err(|error| JournalError::Serialization(error.to_string()))
    }

    fn validate_without_digest(&self) -> Result<(), JournalError> {
        if self.schema != EXECUTION_FACT_SCHEMA_V1 {
            return Err(JournalError::UnsupportedSchema);
        }
        self.frame
            .validate()
            .map_err(|_| JournalError::InvalidField("frame"))?;
        if self.event_type.is_empty()
            || self.event_type.len() > MAX_EVENT_TYPE_BYTES
            || self.event_type.contains('\0')
            || self.event_type.lines().count() != 1
        {
            return Err(JournalError::InvalidField("event_type"));
        }
        if self.kind != ExecutionFactKindV1::from_event_type(&self.event_type) {
            return Err(JournalError::InvalidField("kind"));
        }
        validate_digest(&self.payload_digest)
            .map_err(|_| JournalError::InvalidField("payload_digest"))?;
        if self.payload_bytes == 0
            || self.payload_bytes > u64::try_from(MAX_PAYLOAD_BYTES).unwrap_or(u64::MAX)
        {
            return Err(JournalError::InvalidField("payload_bytes"));
        }
        if self.artifact_refs.len() > MAX_ARTIFACT_REFS
            || self.artifact_refs.iter().any(|uri| {
                uri.is_empty()
                    || uri.len() > MAX_ARTIFACT_URI_BYTES
                    || uri.contains('\0')
                    || uri.lines().count() != 1
            })
        {
            return Err(JournalError::InvalidField("artifact_refs"));
        }
        if self
            .artifact_refs
            .windows(2)
            .any(|window| window[0] >= window[1])
        {
            return Err(JournalError::InvalidField("artifact_refs"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFactPageV1 {
    pub facts: Vec<ExecutionFactV1>,
    pub first_available_sequence: Option<u64>,
    pub latest_sequence_exclusive: u64,
    pub next_cursor: Option<u64>,
    pub retention_gap: bool,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFactSnapshotV1 {
    pub target: ExecutionTargetV1,
    pub page: ExecutionFactPageV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactAppendOutcomeV1 {
    pub appended: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JournalError {
    #[error("execution fact schema is unsupported")]
    UnsupportedSchema,
    #[error("execution fact field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("execution fact `{0}` does not match its contents")]
    DigestMismatch(&'static str),
    #[error("execution fact target does not match the journal key")]
    TargetMismatch,
    #[error("execution fact frame conflicts with existing target history")]
    FrameConflict,
    #[error("execution fact sequence is not contiguous")]
    SequenceGap,
    #[error("execution fact sequence conflicts with an existing fact")]
    SequenceConflict,
    #[error("execution fact limit is invalid")]
    InvalidLimit,
    #[error("execution fact serialization failed: {0}")]
    Serialization(String),
}

pub trait ExecutionFactJournal: Send + Sync {
    fn append(&self, fact: ExecutionFactV1) -> Result<FactAppendOutcomeV1, JournalError>;

    /// Adapt an existing Code run event into the digest-only fact contract.
    /// Implementations may override this when their durable journal has a
    /// native event encoder; the default keeps the conversion identical for
    /// every host.
    fn append_run_event(
        &self,
        frame: ExecutionFrameV1,
        record: &RunEventRecord,
    ) -> Result<FactAppendOutcomeV1, JournalError> {
        self.append(ExecutionFactV1::from_run_event(frame, record)?)
    }

    fn page(
        &self,
        target: &ExecutionTargetV1,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Option<ExecutionFactPageV1>;
    fn snapshot(&self, target: &ExecutionTargetV1) -> Option<ExecutionFactSnapshotV1>;
}

/// Small adapter for runtimes that already receive `RunEventRecord` values.
/// It makes the frame binding explicit at construction and keeps event
/// ingestion out of product-specific evaluator code.
#[derive(Clone)]
pub struct ExecutionFactRecorder {
    journal: Arc<dyn ExecutionFactJournal>,
    frame: ExecutionFrameV1,
}

impl std::fmt::Debug for ExecutionFactRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionFactRecorder")
            .field("target", &self.frame.target)
            .finish()
    }
}

impl ExecutionFactRecorder {
    pub fn new(journal: Arc<dyn ExecutionFactJournal>, frame: ExecutionFrameV1) -> Self {
        Self { journal, frame }
    }

    pub fn record(&self, event: &RunEventRecord) -> Result<FactAppendOutcomeV1, JournalError> {
        self.journal.append_run_event(self.frame.clone(), event)
    }

    pub fn journal(&self) -> Arc<dyn ExecutionFactJournal> {
        Arc::clone(&self.journal)
    }
}

#[derive(Debug, Default)]
struct FactBuffer {
    facts: VecDeque<ExecutionFactV1>,
    serialized_bytes: usize,
    latest_sequence_exclusive: u64,
    /// The frame is part of the target's immutable identity plane. Keep it
    /// separately from the retained FIFO window so a fully trimmed journal
    /// cannot silently accept a different parent or generation later.
    frame: Option<ExecutionFrameV1>,
}

/// Bounded in-memory implementation used by the native harness and tests.
/// Hosts may implement [`ExecutionFactJournal`] over a durable append log.
#[derive(Debug, Clone)]
pub struct InMemoryExecutionFactJournal {
    inner: Arc<RwLock<HashMap<ExecutionTargetV1, FactBuffer>>>,
    max_facts_per_target: Option<usize>,
    max_bytes_per_target: Option<usize>,
}

impl InMemoryExecutionFactJournal {
    pub fn new() -> Self {
        Self::with_limits(None, None)
    }

    pub fn with_limits(
        max_facts_per_target: Option<usize>,
        max_bytes_per_target: Option<usize>,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            max_facts_per_target,
            max_bytes_per_target,
        }
    }

    pub fn append_event(
        &self,
        frame: ExecutionFrameV1,
        record: &RunEventRecord,
    ) -> Result<FactAppendOutcomeV1, JournalError> {
        <Self as ExecutionFactJournal>::append_run_event(self, frame, record)
    }

    fn trim(buffer: &mut FactBuffer, max_facts: Option<usize>, max_bytes: Option<usize>) {
        while max_facts.is_some_and(|limit| buffer.facts.len() > limit)
            || max_bytes.is_some_and(|limit| buffer.serialized_bytes > limit)
        {
            let Some(fact) = buffer.facts.pop_front() else {
                break;
            };
            buffer.serialized_bytes = buffer
                .serialized_bytes
                .saturating_sub(serialized_fact_len(&fact));
        }
    }
}

impl Default for InMemoryExecutionFactJournal {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionFactJournal for InMemoryExecutionFactJournal {
    fn append(&self, fact: ExecutionFactV1) -> Result<FactAppendOutcomeV1, JournalError> {
        fact.validate()?;
        let target = fact.frame.target.clone();
        let mut state = self
            .inner
            .write()
            .map_err(|_| JournalError::InvalidField("lock"))?;
        let buffer = state.entry(target).or_default();
        if let Some(frame) = &buffer.frame {
            if frame != &fact.frame {
                return Err(JournalError::FrameConflict);
            }
        } else {
            buffer.frame = Some(fact.frame.clone());
        }
        if let Some(existing) = buffer
            .facts
            .iter()
            .find(|entry| entry.sequence == fact.sequence)
        {
            if existing == &fact {
                return Ok(FactAppendOutcomeV1 {
                    appended: false,
                    replayed: true,
                });
            }
            return Err(JournalError::SequenceConflict);
        }
        // `latest_sequence_exclusive` is cumulative and survives FIFO
        // retention.  Basing admission on the last retained fact would allow
        // a stream whose whole window was trimmed to restart at an arbitrary
        // sequence (or to accept a replayed old event).
        let expected = buffer.latest_sequence_exclusive;
        if fact.sequence != expected {
            return Err(JournalError::SequenceGap);
        }
        let next_sequence = fact
            .sequence
            .checked_add(1)
            .ok_or(JournalError::InvalidField("sequence"))?;
        buffer.serialized_bytes = buffer
            .serialized_bytes
            .saturating_add(serialized_fact_len(&fact));
        buffer.latest_sequence_exclusive = next_sequence;
        buffer.facts.push_back(fact);
        Self::trim(buffer, self.max_facts_per_target, self.max_bytes_per_target);
        Ok(FactAppendOutcomeV1 {
            appended: true,
            replayed: false,
        })
    }

    fn page(
        &self,
        target: &ExecutionTargetV1,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Option<ExecutionFactPageV1> {
        if limit == 0 {
            return None;
        }
        let state = self.inner.read().ok()?;
        let buffer = state.get(target)?;
        let first_available_sequence = buffer.facts.front().map(|fact| fact.sequence);
        let latest_sequence_exclusive = buffer.latest_sequence_exclusive;
        let requested_start = after_sequence
            .map(|value| value.saturating_add(1))
            .unwrap_or(0);
        let retention_gap = if requested_start >= latest_sequence_exclusive {
            false
        } else {
            first_available_sequence
                .map(|first| requested_start < first)
                .unwrap_or(true)
        };
        let mut matching = buffer
            .facts
            .iter()
            .filter(|fact| after_sequence.is_none_or(|cursor| fact.sequence > cursor));
        let facts = matching.by_ref().take(limit).cloned().collect::<Vec<_>>();
        let has_more = matching.next().is_some();
        let next_cursor = facts.last().map(|fact| fact.sequence).or(after_sequence);
        Some(ExecutionFactPageV1 {
            facts,
            first_available_sequence,
            latest_sequence_exclusive,
            next_cursor,
            retention_gap,
            has_more,
        })
    }

    fn snapshot(&self, target: &ExecutionTargetV1) -> Option<ExecutionFactSnapshotV1> {
        self.page(target, None, usize::MAX)
            .map(|page| ExecutionFactSnapshotV1 {
                target: target.clone(),
                page,
            })
    }
}

fn serialized_fact_len(fact: &ExecutionFactV1) -> usize {
    serde_json::to_vec(fact)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn collect_artifact_refs(value: &serde_json::Value) -> Vec<String> {
    let mut refs = Vec::new();
    collect_artifact_refs_inner(value, &mut refs);
    refs.sort();
    refs.dedup();
    refs.truncate(MAX_ARTIFACT_REFS);
    refs
}

fn collect_artifact_refs_inner(value: &serde_json::Value, refs: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            for key in ["artifact_uri", "content_ref", "content_uri"] {
                if let Some(uri) = object.get(key).and_then(serde_json::Value::as_str) {
                    if uri.len() <= MAX_ARTIFACT_URI_BYTES {
                        refs.push(uri.to_string());
                    }
                }
            }
            for child in object.values() {
                collect_artifact_refs_inner(child, refs);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_artifact_refs_inner(child, refs);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;

    fn frame() -> ExecutionFrameV1 {
        ExecutionFrameV1::root(ExecutionTargetV1::new("session-1", "run-1"))
    }

    fn fact(sequence: u64) -> ExecutionFactV1 {
        ExecutionFactV1::from_input(ExecutionFactInputV1 {
            frame: frame(),
            sequence,
            observed_at_ms: sequence + 1,
            event_type: "tool_end".to_string(),
            payload_digest: digest_bytes("test", &[sequence as u8]),
            payload_bytes: 1,
            artifact_refs: Vec::new(),
        })
        .unwrap()
    }

    #[test]
    fn event_input_is_digest_only_and_extracts_artifact_refs() {
        let event = AgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "read".to_string(),
            args: None,
            output: "secret output".to_string(),
            exit_code: 0,
            metadata: Some(serde_json::json!({
                "artifact": {"artifact_uri": "a3s://artifact/1"}
            })),
            error_kind: None,
        };
        let input = ExecutionFactInputV1::from_event(frame(), 0, 10, &event).unwrap();
        assert_eq!(input.event_type, "tool_end");
        assert!(input.payload_digest.starts_with("sha256:"));
        assert_eq!(input.artifact_refs, vec!["a3s://artifact/1"]);
        let fact = ExecutionFactV1::from_input(input).unwrap();
        let encoded = serde_json::to_string(&fact).unwrap();
        assert!(!encoded.contains("secret output"));
        assert!(fact.validate().is_ok());
    }

    #[test]
    fn journal_is_contiguous_idempotent_and_conflict_safe() {
        let journal = InMemoryExecutionFactJournal::new();
        assert!(journal.append(fact(0)).unwrap().appended);
        assert!(journal.append(fact(0)).unwrap().replayed);
        assert!(matches!(
            journal.append(fact(2)),
            Err(JournalError::SequenceGap)
        ));
        let mut conflicting = fact(0);
        conflicting.payload_bytes = 99;
        conflicting.fact_digest = conflicting.expected_digest().unwrap();
        assert!(matches!(
            journal.append(conflicting),
            Err(JournalError::SequenceConflict)
        ));
    }

    #[test]
    fn fact_kind_is_derived_from_the_event_type() {
        let mut forged = fact(0);
        forged.kind = ExecutionFactKindV1::Lifecycle;
        forged.fact_digest = forged.expected_digest().unwrap();
        assert!(matches!(
            forged.validate(),
            Err(JournalError::InvalidField("kind"))
        ));
    }

    #[test]
    fn journal_keeps_frame_identity_after_fifo_retention() {
        let journal = InMemoryExecutionFactJournal::with_limits(Some(1), None);
        journal.append(fact(0)).unwrap();
        journal.append(fact(1)).unwrap();
        let target = ExecutionTargetV1::new("session-1", "run-1");
        let forged = ExecutionFactV1::from_input(ExecutionFactInputV1 {
            frame: ExecutionFrameV1::child(
                target,
                ExecutionTargetV1::new("session-parent", "parent-run"),
            ),
            sequence: 2,
            observed_at_ms: 3,
            event_type: "tool_end".to_string(),
            payload_digest: digest_bytes("test", &[2]),
            payload_bytes: 1,
            artifact_refs: Vec::new(),
        })
        .unwrap();
        assert!(matches!(
            journal.append(forged),
            Err(JournalError::FrameConflict)
        ));
    }

    #[test]
    fn journal_reports_retention_gap() {
        let journal = InMemoryExecutionFactJournal::with_limits(Some(2), None);
        for sequence in 0..3 {
            journal.append(fact(sequence)).unwrap();
        }
        let target = ExecutionTargetV1::new("session-1", "run-1");
        let page = journal.page(&target, None, 10).unwrap();
        assert_eq!(page.first_available_sequence, Some(1));
        assert_eq!(page.latest_sequence_exclusive, 3);
        assert!(page.retention_gap);
        assert_eq!(page.facts.len(), 2);
    }

    #[test]
    fn journal_requires_zero_for_a_new_stream_and_keeps_cursor_after_full_trim() {
        let journal = InMemoryExecutionFactJournal::new();
        assert!(matches!(
            journal.append(fact(1)),
            Err(JournalError::SequenceGap)
        ));

        let trimmed = InMemoryExecutionFactJournal::with_limits(Some(1), Some(1));
        trimmed.append(fact(0)).unwrap();
        let target = ExecutionTargetV1::new("session-1", "run-1");
        let page = trimmed.page(&target, None, 10).unwrap();
        assert!(page.facts.is_empty());
        assert_eq!(page.latest_sequence_exclusive, 1);
        assert!(page.retention_gap);
        assert!(matches!(
            trimmed.append(fact(0)),
            Err(JournalError::SequenceGap)
        ));
        trimmed.append(fact(1)).unwrap();

        let no_future = trimmed.page(&target, Some(u64::MAX), 10).unwrap();
        assert!(!no_future.retention_gap);
        assert!(no_future.facts.is_empty());
    }
}
