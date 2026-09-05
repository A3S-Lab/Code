use super::{digest, validate_digest_field, validate_id, ResearchContractError};
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

        let mut encoded = serde_json::to_value(&event).unwrap();
        encoded["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ResearchEventV1>(encoded).is_err());
    }
}
