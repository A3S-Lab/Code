//! Append-only core event log shared by runtime projections.
//!
//! This is the KRN-1 event fabric: one bounded, hash-chained log of
//! [`CoreEventIdentity`] values per
//! operation. The existing run journal, evaluation fact journal, and research
//! projections remain the compatibility authorities; this log lets a host
//! append and replay one canonical event stream where duplicate, reordered,
//! stale-generation, and cursor-skipping writes fail closed.
//!
//! The log deliberately stores digest-only identities plus bounded artifact
//! URI references. Raw payloads stay in their existing stores; nothing here
//! becomes a second audit database.

use crate::agent::AgentEvent;
use crate::core_identity::{
    CoreEventIdentity, CoreIdentity, CoreIdentityError, EvidenceCursor, LogicalClock, OperationId,
    SourceRevision,
};
use crate::run::RunEventRecord;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use thiserror::Error;

pub const CORE_LOG_ENTRY_SCHEMA_V1: &str = "a3s.code.core-event-log-entry.v1";
pub const CORE_LOG_ENTRY_DIGEST_DOMAIN_V1: &str = "a3s.code.core-event-log.entry.v1";
/// Chain anchor used by the first entry of every operation stream.
pub const CORE_LOG_GENESIS_PREVIOUS_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";
const MAX_LOG_ENTRIES_PER_OPERATION: usize = 65_536;
const MAX_LOG_BYTES_PER_OPERATION: usize = 64 * 1024 * 1024;
const MAX_ARTIFACT_REFS: usize = 128;
const MAX_ARTIFACT_URI_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreEventLogError {
    #[error("core event log entry schema is unsupported")]
    UnsupportedSchema,
    #[error("core event log field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("core event log `{0}` does not match its contents")]
    DigestMismatch(&'static str),
    #[error("core event log cursor is not contiguous with the stream tail")]
    CursorGap,
    #[error("core event log cursor conflicts with a different retained entry")]
    CursorConflict,
    #[error("core event log entry does not chain to the current stream tail")]
    CausalityConflict,
    #[error("core event log rejects an older capability generation than the stream pin")]
    StaleGeneration,
    #[error("core event log rejects a regressing source revision")]
    StaleSourceRevision,
    #[error("core event log serialization failed: {0}")]
    Serialization(String),
    #[error("core event log lock is poisoned")]
    LockPoisoned,
}

impl From<CoreIdentityError> for CoreEventLogError {
    fn from(error: CoreIdentityError) -> Self {
        match error {
            CoreIdentityError::UnsupportedSchema => Self::UnsupportedSchema,
            CoreIdentityError::InvalidDigest(field) => Self::DigestMismatch(field),
            CoreIdentityError::Serialization(message) => Self::Serialization(message),
            other => Self::InvalidField(match other {
                CoreIdentityError::InvalidField(field) => field,
                _ => "identity",
            }),
        }
    }
}

/// One hash-chained log entry: a validated core event identity plus the
/// bounded artifact references that locate its payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreLogEntryV1 {
    pub schema: String,
    pub event: CoreEventIdentity,
    pub previous_digest: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<String>,
    pub entry_digest: String,
}

impl CoreLogEntryV1 {
    pub fn new(
        event: CoreEventIdentity,
        previous_digest: impl Into<String>,
        artifact_refs: Vec<String>,
    ) -> Result<Self, CoreEventLogError> {
        let mut entry = Self {
            schema: CORE_LOG_ENTRY_SCHEMA_V1.to_owned(),
            event,
            previous_digest: previous_digest.into(),
            artifact_refs,
            entry_digest: String::new(),
        };
        entry.validate_without_digest()?;
        entry.entry_digest = entry.expected_digest()?;
        Ok(entry)
    }

    pub fn validate(&self) -> Result<(), CoreEventLogError> {
        self.validate_without_digest()?;
        validate_digest("entry_digest", &self.entry_digest)?;
        if self.entry_digest != self.expected_digest()? {
            return Err(CoreEventLogError::DigestMismatch("entry_digest"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, CoreEventLogError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            event_digest: &'a str,
            previous_digest: &'a str,
            artifact_refs: &'a [String],
        }
        let bytes = serde_json::to_vec(&Identity {
            schema: &self.schema,
            event_digest: &self.event.event_digest,
            previous_digest: &self.previous_digest,
            artifact_refs: &self.artifact_refs,
        })
        .map_err(|error| CoreEventLogError::Serialization(error.to_string()))?;
        Ok(digest_bytes(CORE_LOG_ENTRY_DIGEST_DOMAIN_V1, &bytes))
    }

    fn validate_without_digest(&self) -> Result<(), CoreEventLogError> {
        if self.schema != CORE_LOG_ENTRY_SCHEMA_V1 {
            return Err(CoreEventLogError::UnsupportedSchema);
        }
        self.event.validate()?;
        let is_genesis = self.previous_digest == CORE_LOG_GENESIS_PREVIOUS_DIGEST;
        if !is_genesis {
            validate_digest("previous_digest", &self.previous_digest)?;
        }
        if self.artifact_refs.len() > MAX_ARTIFACT_REFS
            || self.artifact_refs.iter().any(|uri| {
                uri.is_empty()
                    || uri.len() > MAX_ARTIFACT_URI_BYTES
                    || uri.contains('\0')
                    || uri.lines().count() != 1
            })
        {
            return Err(CoreEventLogError::InvalidField("artifact_refs"));
        }
        if self
            .artifact_refs
            .windows(2)
            .any(|window| window[0] >= window[1])
        {
            return Err(CoreEventLogError::InvalidField("artifact_refs"));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CoreLogEntryV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema: String,
            event: CoreEventIdentity,
            previous_digest: String,
            #[serde(default)]
            artifact_refs: Vec<String>,
            entry_digest: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema: wire.schema,
            event: wire.event,
            previous_digest: wire.previous_digest,
            artifact_refs: wire.artifact_refs,
            entry_digest: wire.entry_digest,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CoreLogAppendOutcomeV1 {
    pub appended: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CoreLogPageV1 {
    pub entries: Vec<CoreLogEntryV1>,
    pub first_available_cursor: Option<u64>,
    pub latest_cursor_exclusive: u64,
    pub next_cursor: Option<u64>,
    pub retention_gap: bool,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CoreLogVerificationV1 {
    pub verified_entries: usize,
    pub first_available_cursor: Option<u64>,
    pub latest_cursor_exclusive: u64,
    pub chain_head_digest: String,
}

#[derive(Debug, Default)]
struct LogBuffer {
    entries: VecDeque<CoreLogEntryV1>,
    serialized_bytes: usize,
    /// Cumulative evidence cursor bound; survives FIFO retention so a fully
    /// trimmed stream cannot restart at an arbitrary cursor.
    latest_cursor_exclusive: u64,
    /// Current chain head; survives FIFO retention so causality stays exact
    /// even when the newest retained entry has been evicted.
    tail_entry_digest: Option<String>,
    /// Highest capability generation ever admitted; stale generations fail.
    generation_max: Option<u64>,
    /// Highest source revision ever admitted; regressions fail.
    source_revision_max: u64,
}

/// Bounded in-memory append-only log over core event identities.
///
/// Hosts may adapt durable stores behind the same admission rules; the
/// in-memory implementation is the reference semantics and test oracle.
#[derive(Debug, Clone)]
pub struct CoreEventLog {
    inner: Arc<RwLock<HashMap<OperationId, LogBuffer>>>,
    max_entries_per_operation: Option<usize>,
    max_bytes_per_operation: Option<usize>,
}

impl CoreEventLog {
    pub fn new() -> Self {
        Self::with_limits(None, None)
    }

    pub fn with_limits(
        max_entries_per_operation: Option<usize>,
        max_bytes_per_operation: Option<usize>,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            max_entries_per_operation: max_entries_per_operation
                .map(|limit| limit.min(MAX_LOG_ENTRIES_PER_OPERATION)),
            max_bytes_per_operation: max_bytes_per_operation
                .map(|limit| limit.min(MAX_LOG_BYTES_PER_OPERATION)),
        }
    }

    /// Append one already-chained entry under the fail-closed admission rules.
    pub fn append(
        &self,
        entry: CoreLogEntryV1,
    ) -> Result<CoreLogAppendOutcomeV1, CoreEventLogError> {
        entry.validate()?;
        let operation = entry.event.identity.operation_id.clone();
        let mut state = self
            .inner
            .write()
            .map_err(|_| CoreEventLogError::LockPoisoned)?;
        let buffer = state.entry(operation).or_default();
        let appended = admit(buffer, &entry)?;
        if appended {
            buffer.serialized_bytes = buffer
                .serialized_bytes
                .saturating_add(serialized_entry_len(&entry));
            buffer.entries.push_back(entry);
            trim(
                buffer,
                self.max_entries_per_operation,
                self.max_bytes_per_operation,
            );
        }
        Ok(CoreLogAppendOutcomeV1 {
            appended,
            replayed: !appended,
        })
    }

    /// Adapt, chain, and append one agent event through the shared identity
    /// plane. The chain anchor is resolved under the same lock as admission,
    /// so concurrent appenders cannot fork the chain.
    pub fn append_agent_event(
        &self,
        identity: CoreIdentity,
        clock: &dyn LogicalClock,
        event: &AgentEvent,
        artifact_refs: Vec<String>,
    ) -> Result<CoreLogAppendOutcomeV1, CoreEventLogError> {
        let operation = identity.operation_id.clone();
        let core_event = CoreEventIdentity::from_agent_event_at(identity, clock, event)?;
        {
            let state = self
                .inner
                .read()
                .map_err(|_| CoreEventLogError::LockPoisoned)?;
            if let Some(buffer) = state.get(&operation) {
                if let Some(existing) = buffer.entries.iter().find(|entry| {
                    entry.event.identity.evidence_cursor == core_event.identity.evidence_cursor
                }) {
                    if existing.event == core_event {
                        return Ok(CoreLogAppendOutcomeV1 {
                            appended: false,
                            replayed: true,
                        });
                    }
                    return Err(CoreEventLogError::CursorConflict);
                }
            }
        }
        let previous_digest = {
            let state = self
                .inner
                .read()
                .map_err(|_| CoreEventLogError::LockPoisoned)?;
            state
                .get(&operation)
                .and_then(|buffer| buffer.tail_entry_digest.clone())
                .unwrap_or_else(|| CORE_LOG_GENESIS_PREVIOUS_DIGEST.to_owned())
        };
        let entry = CoreLogEntryV1::new(core_event, previous_digest, artifact_refs)?;
        self.append(entry)
    }

    /// Adapt the event representation already retained by a Code run.
    pub fn append_run_event(
        &self,
        operation_id: OperationId,
        source_revision: SourceRevision,
        capability_stamp: Option<crate::core_identity::CapabilityStamp>,
        record: &RunEventRecord,
    ) -> Result<CoreLogAppendOutcomeV1, CoreEventLogError> {
        let identity = CoreIdentity::new(
            operation_id,
            source_revision,
            capability_stamp,
            EvidenceCursor::new(0),
        );
        let core_event = CoreEventIdentity::from_run_event(
            identity.operation_id.clone(),
            identity.source_revision,
            identity.capability_stamp.clone(),
            record,
        )?;
        let artifact_refs = collect_artifact_refs(
            &serde_json::to_value(&record.event)
                .map_err(|error| CoreEventLogError::Serialization(error.to_string()))?,
        );
        self.append_core_event(core_event, artifact_refs)
    }

    /// Chain and append an already adapted core event identity.
    pub fn append_core_event(
        &self,
        core_event: CoreEventIdentity,
        artifact_refs: Vec<String>,
    ) -> Result<CoreLogAppendOutcomeV1, CoreEventLogError> {
        let operation = core_event.identity.operation_id.clone();
        let previous_digest = {
            let state = self
                .inner
                .read()
                .map_err(|_| CoreEventLogError::LockPoisoned)?;
            state
                .get(&operation)
                .and_then(|buffer| buffer.tail_entry_digest.clone())
                .unwrap_or_else(|| CORE_LOG_GENESIS_PREVIOUS_DIGEST.to_owned())
        };
        let entry = CoreLogEntryV1::new(core_event, previous_digest, artifact_refs)?;
        self.append(entry)
    }

    /// Replay retained entries after a cursor. Every paged entry is validated
    /// (identity digest plus chain position) so tampering fails closed.
    pub fn page(
        &self,
        operation: &OperationId,
        after_cursor: Option<EvidenceCursor>,
        limit: usize,
    ) -> Result<Option<CoreLogPageV1>, CoreEventLogError> {
        if limit == 0 {
            return Err(CoreEventLogError::InvalidField("limit"));
        }
        let state = self
            .inner
            .read()
            .map_err(|_| CoreEventLogError::LockPoisoned)?;
        let Some(buffer) = state.get(operation) else {
            return Ok(None);
        };
        let after = after_cursor.map(|cursor| cursor.sequence());
        let first_available_cursor = buffer
            .entries
            .front()
            .map(|entry| entry.event.identity.evidence_cursor.sequence());
        let latest_cursor_exclusive = buffer.latest_cursor_exclusive;
        let requested_start = after.map(|value| value + 1).unwrap_or(0);
        let retention_gap = if requested_start >= latest_cursor_exclusive {
            false
        } else {
            first_available_cursor
                .map(|first| requested_start < first)
                .unwrap_or(true)
        };
        let mut expected_previous = buffer
            .entries
            .front()
            .map(|entry| entry.previous_digest.clone());
        let mut matching = buffer.entries.iter().filter(|entry| {
            after.is_none_or(|cursor| entry.event.identity.evidence_cursor.sequence() > cursor)
        });
        let mut entries = Vec::new();
        let mut has_more = false;
        for entry in matching.by_ref() {
            entry.validate()?;
            if let Some(previous) = &expected_previous {
                if entry.previous_digest != *previous {
                    return Err(CoreEventLogError::CausalityConflict);
                }
            }
            expected_previous = Some(entry.entry_digest.clone());
            if entries.len() < limit {
                entries.push(entry.clone());
            } else {
                has_more = true;
                break;
            }
        }
        if entries.len() == limit {
            has_more = matching.next().is_some();
        }
        let next_cursor = entries
            .last()
            .map(|entry| entry.event.identity.evidence_cursor.sequence())
            .or(after);
        Ok(Some(CoreLogPageV1 {
            entries,
            first_available_cursor,
            latest_cursor_exclusive,
            next_cursor,
            retention_gap,
            has_more,
        }))
    }

    /// Verify the complete retained window of one operation stream.
    pub fn verify(
        &self,
        operation: &OperationId,
    ) -> Result<Option<CoreLogVerificationV1>, CoreEventLogError> {
        let page = self.page(operation, None, usize::MAX)?;
        Ok(page.map(|page| CoreLogVerificationV1 {
            verified_entries: page.entries.len(),
            first_available_cursor: page.first_available_cursor,
            latest_cursor_exclusive: page.latest_cursor_exclusive,
            chain_head_digest: page
                .entries
                .last()
                .map(|entry| entry.entry_digest.clone())
                .unwrap_or_else(|| CORE_LOG_GENESIS_PREVIOUS_DIGEST.to_owned()),
        }))
    }
}

impl Default for CoreEventLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Admission rules shared by every append path. Mutates the buffer's
/// cumulative identity pins on success; pushes nothing (the caller owns the
/// queue so it can account retention first).
fn admit(buffer: &mut LogBuffer, entry: &CoreLogEntryV1) -> Result<bool, CoreEventLogError> {
    let cursor = entry.event.identity.evidence_cursor.sequence();
    if let Some(generation) = entry.event.identity.capability_stamp.as_ref() {
        if buffer
            .generation_max
            .is_some_and(|current| generation.generation() < current)
        {
            return Err(CoreEventLogError::StaleGeneration);
        }
    }
    if entry.event.identity.source_revision.value() < buffer.source_revision_max {
        return Err(CoreEventLogError::StaleSourceRevision);
    }
    if let Some(existing) = buffer
        .entries
        .iter()
        .find(|retained| retained.event.identity.evidence_cursor.sequence() == cursor)
    {
        if existing == entry {
            return Ok(false);
        }
        return Err(CoreEventLogError::CursorConflict);
    }
    if cursor != buffer.latest_cursor_exclusive {
        return Err(CoreEventLogError::CursorGap);
    }
    let expected_previous = buffer
        .tail_entry_digest
        .clone()
        .unwrap_or_else(|| CORE_LOG_GENESIS_PREVIOUS_DIGEST.to_owned());
    if entry.previous_digest != expected_previous {
        return Err(CoreEventLogError::CausalityConflict);
    }
    let next_cursor = cursor
        .checked_add(1)
        .ok_or(CoreEventLogError::InvalidField("evidence_cursor"))?;
    buffer.latest_cursor_exclusive = next_cursor;
    buffer.tail_entry_digest = Some(entry.entry_digest.clone());
    if let Some(stamp) = entry.event.identity.capability_stamp.as_ref() {
        buffer.generation_max = Some(buffer.generation_max.map_or_else(
            || stamp.generation(),
            |current| current.max(stamp.generation()),
        ));
    }
    buffer.source_revision_max = buffer
        .source_revision_max
        .max(entry.event.identity.source_revision.value());
    Ok(true)
}

fn trim(buffer: &mut LogBuffer, max_entries: Option<usize>, max_bytes: Option<usize>) {
    while max_entries.is_some_and(|limit| buffer.entries.len() > limit)
        || max_bytes.is_some_and(|limit| buffer.serialized_bytes > limit)
    {
        let Some(entry) = buffer.entries.pop_front() else {
            break;
        };
        buffer.serialized_bytes = buffer
            .serialized_bytes
            .saturating_sub(serialized_entry_len(&entry));
    }
}

fn serialized_entry_len(entry: &CoreLogEntryV1) -> usize {
    serde_json::to_vec(entry)
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

fn validate_digest(field: &'static str, value: &str) -> Result<(), CoreEventLogError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(CoreEventLogError::DigestMismatch(field));
    }
    Ok(())
}

fn digest_bytes(domain: &str, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_identity::{CapabilityStamp, ManualLogicalClock, OperationId, SourceRevision};

    fn operation() -> OperationId {
        OperationId::new("session-1/run-1").unwrap()
    }

    fn identity(cursor: u64) -> CoreIdentity {
        CoreIdentity::new(
            operation(),
            SourceRevision::new(7),
            Some(
                CapabilityStamp::new(
                    3,
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .unwrap(),
            ),
            EvidenceCursor::new(cursor),
        )
    }

    fn entry(cursor: u64) -> CoreLogEntryV1 {
        let clock = ManualLogicalClock::new(1_000 + cursor);
        let event = AgentEvent::TurnStart {
            turn: cursor as usize,
        };
        let core_event =
            CoreEventIdentity::from_agent_event_at(identity(cursor), &clock, &event).unwrap();
        let previous = if cursor == 0 {
            CORE_LOG_GENESIS_PREVIOUS_DIGEST.to_owned()
        } else {
            entry(cursor - 1).entry_digest
        };
        CoreLogEntryV1::new(core_event, previous, Vec::new()).unwrap()
    }

    #[test]
    fn appending_and_replaying_twice_is_idempotent() {
        let log = CoreEventLog::new();
        for cursor in 0..4 {
            let outcome = log.append(entry(cursor)).unwrap();
            assert!(outcome.appended && !outcome.replayed);
        }
        // Re-appending the identical batch replays without duplicating.
        for cursor in 0..4 {
            let outcome = log.append(entry(cursor)).unwrap();
            assert!(!outcome.appended && outcome.replayed);
        }
        let first = log.page(&operation(), None, usize::MAX).unwrap().unwrap();
        let second = log.page(&operation(), None, usize::MAX).unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.entries.len(), 4);
        assert_eq!(first.latest_cursor_exclusive, 4);
        assert!(!first.retention_gap);
        let verification = log.verify(&operation()).unwrap().unwrap();
        assert_eq!(verification.verified_entries, 4);
    }

    #[test]
    fn duplicate_cursor_with_different_content_fails_closed() {
        let log = CoreEventLog::new();
        log.append(entry(0)).unwrap();
        let mut forged = entry(0);
        let clock = ManualLogicalClock::new(9_999);
        let event = AgentEvent::Error {
            message: "forged".to_owned(),
        };
        forged.event = CoreEventIdentity::from_agent_event_at(identity(0), &clock, &event).unwrap();
        forged.entry_digest = forged.expected_digest().unwrap();
        assert!(matches!(
            log.append(forged),
            Err(CoreEventLogError::CursorConflict)
        ));
    }

    #[test]
    fn reordered_and_cursor_skipping_writes_fail_closed() {
        let log = CoreEventLog::new();
        assert!(matches!(
            log.append(entry(1)),
            Err(CoreEventLogError::CursorGap)
        ));
        log.append(entry(0)).unwrap();
        assert!(matches!(
            log.append(entry(2)),
            Err(CoreEventLogError::CursorGap)
        ));
        // A next-cursor entry that bypasses the current tail cannot fork the
        // stream: the causality check fails it closed even though its cursor
        // is exactly the next expected one.
        let mut fork = entry(1);
        fork.previous_digest = CORE_LOG_GENESIS_PREVIOUS_DIGEST.to_owned();
        fork.entry_digest = fork.expected_digest().unwrap();
        assert!(matches!(
            log.append(fork),
            Err(CoreEventLogError::CausalityConflict)
        ));
    }

    #[test]
    fn causality_conflicts_fail_closed() {
        let log = CoreEventLog::new();
        log.append(entry(0)).unwrap();
        let mut orphan = entry(1);
        orphan.previous_digest = CORE_LOG_GENESIS_PREVIOUS_DIGEST.to_owned();
        orphan.entry_digest = orphan.expected_digest().unwrap();
        assert!(matches!(
            log.append(orphan),
            Err(CoreEventLogError::CausalityConflict)
        ));
    }

    #[test]
    fn stale_generation_and_source_regression_fail_closed() {
        let log = CoreEventLog::new();
        log.append(entry(0)).unwrap();
        let clock = ManualLogicalClock::new(2_000);
        let event = AgentEvent::TurnStart { turn: 1 };
        let stale_generation = CoreIdentity::new(
            operation(),
            SourceRevision::new(7),
            Some(
                CapabilityStamp::new(
                    2,
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .unwrap(),
            ),
            EvidenceCursor::new(1),
        );
        let stale_event =
            CoreEventIdentity::from_agent_event_at(stale_generation, &clock, &event).unwrap();
        let stale_entry =
            CoreLogEntryV1::new(stale_event, entry(0).entry_digest, Vec::new()).unwrap();
        assert!(matches!(
            log.append(stale_entry),
            Err(CoreEventLogError::StaleGeneration)
        ));

        let stale_source = CoreIdentity::new(
            operation(),
            SourceRevision::new(6),
            Some(
                CapabilityStamp::new(
                    3,
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .unwrap(),
            ),
            EvidenceCursor::new(1),
        );
        let stale_source_event =
            CoreEventIdentity::from_agent_event_at(stale_source, &clock, &event).unwrap();
        let stale_source_entry =
            CoreLogEntryV1::new(stale_source_event, entry(0).entry_digest, Vec::new()).unwrap();
        assert!(matches!(
            log.append(stale_source_entry),
            Err(CoreEventLogError::StaleSourceRevision)
        ));
    }

    #[test]
    fn retention_reports_gaps_and_keeps_identity_pins() {
        let log = CoreEventLog::with_limits(Some(2), None);
        for cursor in 0..4 {
            log.append(entry(cursor)).unwrap();
        }
        let page = log.page(&operation(), None, usize::MAX).unwrap().unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.first_available_cursor, Some(2));
        assert_eq!(page.latest_cursor_exclusive, 4);
        assert!(page.retention_gap);
        // A fully trimmed replay of an old cursor fails closed instead of
        // restarting the stream.
        assert!(matches!(
            log.append(entry(0)),
            Err(CoreEventLogError::CursorGap)
        ));
        // The generation pin survives retention.
        let clock = ManualLogicalClock::new(9_000);
        let event = AgentEvent::TurnStart { turn: 99 };
        let stale = CoreIdentity::new(
            operation(),
            SourceRevision::new(7),
            Some(
                CapabilityStamp::new(
                    2,
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .unwrap(),
            ),
            EvidenceCursor::new(4),
        );
        let stale_event = CoreEventIdentity::from_agent_event_at(stale, &clock, &event).unwrap();
        let stale_entry = CoreLogEntryV1::new(
            stale_event,
            page.entries[1].entry_digest.clone(),
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            log.append(stale_entry),
            Err(CoreEventLogError::StaleGeneration)
        ));
    }

    #[test]
    fn replay_detects_a_tampered_chain() {
        let log = CoreEventLog::new();
        for cursor in 0..3 {
            log.append(entry(cursor)).unwrap();
        }
        {
            let mut state = log.inner.write().unwrap();
            let buffer = state.get_mut(&operation()).unwrap();
            buffer.entries[1].artifact_refs = vec!["a3s://artifact/1".to_owned()];
        }
        assert!(matches!(
            log.verify(&operation()),
            Err(CoreEventLogError::DigestMismatch(_))
        ));
    }

    #[test]
    fn agent_event_adapter_chains_and_replays() {
        let log = CoreEventLog::new();
        let clock = ManualLogicalClock::new(500);
        let event = AgentEvent::TurnStart { turn: 0 };
        let outcome = log
            .append_agent_event(identity(0), &clock, &event, Vec::new())
            .unwrap();
        assert!(outcome.appended);
        let replay = log
            .append_agent_event(identity(0), &clock, &event, Vec::new())
            .unwrap();
        assert!(replay.replayed && !replay.appended);
        let page = log.page(&operation(), None, usize::MAX).unwrap().unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(
            page.entries[0].event.identity.operation_id.as_str(),
            "session-1/run-1"
        );
    }

    #[test]
    fn projections_share_one_identity_through_the_log() {
        use crate::research::ResearchEventV1;
        let log = CoreEventLog::new();
        let clock = ManualLogicalClock::new(1_000);
        let event = AgentEvent::TurnStart { turn: 0 };
        log.append_agent_event(identity(0), &clock, &event, Vec::new())
            .unwrap();
        let page = log.page(&operation(), None, usize::MAX).unwrap().unwrap();
        let core = page.entries[0].event.clone();
        let projected = ResearchEventV1::from_core_event("project-1", 3, &core).unwrap();
        let replayed = ResearchEventV1::from_core_event("project-1", 3, &core).unwrap();
        assert_eq!(projected, replayed);
        assert_eq!(projected.payload_digest, core.payload_digest);
        assert_eq!(projected.observed_at_ms, core.observed_at_ms);
    }
}
