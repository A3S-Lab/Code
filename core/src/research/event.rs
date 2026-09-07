use super::{digest, validate_digest_field, validate_id, ResearchContractError};
use crate::core_identity::CoreEventIdentity;
use serde::{Deserialize, Serialize};

pub const RESEARCH_EVENT_SCHEMA_V1: &str = "a3s.code.science-event.v1";
const RESEARCH_EVENT_DIGEST_DOMAIN: &str = "a3s.code.science-event.identity.v1";
pub const RESEARCH_MAX_EVENT_TYPE_BYTES: usize = 128;

/// Digest-only research event projection for Desktop and other hosts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchEventV1 {
    pub schema: String,
    pub project_id: String,
    pub project_revision: u64,
    pub run_id: Option<String>,
    pub sequence: u64,
    pub event_type: String,
    pub payload_digest: String,
    pub observed_at_ms: u64,
    pub event_digest: String,
}

impl ResearchEventV1 {
    pub fn new(
        project_id: impl Into<String>,
        project_revision: u64,
        run_id: Option<String>,
        sequence: u64,
        event_type: impl Into<String>,
        payload_digest: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, ResearchContractError> {
        let mut event = Self {
            schema: RESEARCH_EVENT_SCHEMA_V1.to_owned(),
            project_id: project_id.into(),
            project_revision,
            run_id,
            sequence,
            event_type: event_type.into(),
            payload_digest: payload_digest.into(),
            observed_at_ms,
            event_digest: String::new(),
        };
        event.validate_without_digest()?;
        event.event_digest = event.expected_digest()?;
        Ok(event)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("eventDigest", &self.event_digest)?;
        if self.event_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("eventDigest"));
        }
        Ok(())
    }

    /// Decode a bounded JSON research event and validate its identity before
    /// returning it to a caller at a process boundary.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let event: Self = super::decode_json_slice(bytes)?;
        event.validate()?;
        Ok(event)
    }

    /// Encode a validated research event for a process boundary.
    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    /// Project one Core event into the research event view.
    ///
    /// Core evidence cursors are zero-based because they align with retained
    /// `RunEventRecord` sequences. The research wire contract is intentionally
    /// one-based, so this adapter performs the only explicit representation
    /// conversion while preserving the operation id, payload digest, and
    /// observation time. Research event names are dotted by contract, so the
    /// runtime name is placed under the explicit `code` namespace and runtime
    /// underscores are normalized to the research contract's hyphens.
    pub fn from_core_event(
        project_id: impl Into<String>,
        project_revision: u64,
        event: &CoreEventIdentity,
    ) -> Result<Self, ResearchContractError> {
        Self::from_core_event_for_run(
            project_id,
            project_revision,
            event.identity.operation_id.as_str(),
            event,
        )
    }

    /// Project a Core event while supplying the actual Code Run identity.
    ///
    /// `CoreEventIdentity::operation_id` is intentionally opaque and may
    /// represent a session-scoped operation rather than the bare Run id.
    /// Research projections must therefore use this explicit adapter whenever
    /// a host has the owning `ExecutionTargetV1` available.
    pub fn from_core_event_for_run(
        project_id: impl Into<String>,
        project_revision: u64,
        run_id: impl Into<String>,
        event: &CoreEventIdentity,
    ) -> Result<Self, ResearchContractError> {
        event
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("coreEvent"))?;
        let sequence = event
            .identity
            .evidence_cursor
            .sequence()
            .checked_add(1)
            .ok_or(ResearchContractError::InvalidField("sequence"))?;
        Self::new(
            project_id,
            project_revision,
            Some(run_id.into()),
            sequence,
            format!("code.{}", event.event_type.replace('_', "-")),
            event.payload_digest.clone(),
            event.observed_at_ms,
        )
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_EVENT_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("projectId", &self.project_id)?;
        if self.project_revision == 0 {
            return Err(ResearchContractError::InvalidField("projectRevision"));
        }
        if let Some(run_id) = &self.run_id {
            validate_id("runId", run_id)?;
        }
        if self.sequence == 0 {
            return Err(ResearchContractError::InvalidField("sequence"));
        }
        validate_event_type(&self.event_type)?;
        validate_digest_field("payloadDigest", &self.payload_digest)?;
        if self.observed_at_ms == 0 {
            return Err(ResearchContractError::InvalidField("observedAtMs"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            project_id: &'a str,
            project_revision: u64,
            run_id: Option<&'a str>,
            sequence: u64,
            event_type: &'a str,
            payload_digest: &'a str,
            observed_at_ms: u64,
        }
        digest(
            RESEARCH_EVENT_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                project_id: &self.project_id,
                project_revision: self.project_revision,
                run_id: self.run_id.as_deref(),
                sequence: self.sequence,
                event_type: &self.event_type,
                payload_digest: &self.payload_digest,
                observed_at_ms: self.observed_at_ms,
            },
        )
    }
}

fn validate_event_type(value: &str) -> Result<(), ResearchContractError> {
    if value.is_empty()
        || value.len() > RESEARCH_MAX_EVENT_TYPE_BYTES
        || value.starts_with('.')
        || value.ends_with('.')
        || value.split('.').any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(ResearchContractError::InvalidField("eventType"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_identity::{
        CapabilityStamp, CoreIdentity, EvidenceCursor, OperationId, SourceRevision,
    };
    use crate::AgentEvent;

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    #[test]
    fn event_rejects_noncanonical_event_types_and_binds_payload() {
        assert!(matches!(
            ResearchEventV1::new("project-1", 1, None, 1, "Research.Started", digest('a'), 1),
            Err(ResearchContractError::InvalidField("eventType"))
        ));
        let event = ResearchEventV1::new(
            "project-1",
            1,
            Some("run-1".to_owned()),
            1,
            "research.run.admitted",
            digest('a'),
            1,
        )
        .unwrap();
        assert!(event.validate().is_ok());
        let encoded = event.to_vec().unwrap();
        assert_eq!(ResearchEventV1::from_slice(&encoded).unwrap(), event);

        let mut encoded = serde_json::to_value(&event).unwrap();
        encoded["unexpected"] = serde_json::Value::Bool(true);
        assert!(ResearchEventV1::from_slice(&serde_json::to_vec(&encoded).unwrap()).is_err());
    }

    #[test]
    fn core_event_projection_preserves_identity_and_uses_research_sequence() {
        let core = CoreEventIdentity::from_agent_event(
            CoreIdentity::new(
                OperationId::new("session-1/run-1").unwrap(),
                SourceRevision::new(4),
                Some(CapabilityStamp::new(2, digest('b')).unwrap()),
                EvidenceCursor::new(6),
            ),
            42,
            &AgentEvent::TextDelta {
                text: "finding".to_owned(),
            },
        )
        .unwrap();
        let projected = ResearchEventV1::from_core_event("project-1", 3, &core).unwrap();

        assert_eq!(projected.run_id.as_deref(), Some("session-1/run-1"));
        assert_eq!(projected.sequence, 7);
        assert_eq!(projected.event_type, "code.text-delta");
        assert_eq!(projected.payload_digest, core.payload_digest);
        assert_eq!(projected.observed_at_ms, 42);
        assert!(projected.validate().is_ok());
    }

    #[test]
    fn explicit_run_projection_does_not_confuse_operation_and_run_identity() {
        let core = CoreEventIdentity::from_agent_event(
            CoreIdentity::new(
                OperationId::new("session-1/run-1/turn-2").unwrap(),
                SourceRevision::new(4),
                None,
                EvidenceCursor::new(6),
            ),
            42,
            &AgentEvent::TextDelta {
                text: "finding".to_owned(),
            },
        )
        .unwrap();
        let projected =
            ResearchEventV1::from_core_event_for_run("project-1", 3, "run-1", &core).unwrap();

        assert_eq!(projected.run_id.as_deref(), Some("run-1"));
        assert_ne!(projected.run_id.as_deref(), Some("session-1/run-1/turn-2"));
        assert!(projected.validate().is_ok());
    }
}
