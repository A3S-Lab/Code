use super::{
    SessionCheckpointDescriptorV1, SessionCheckpointError, SessionCheckpointPayloadV1,
    SessionCheckpointResult, SessionLogicalResumeEvidenceV1, SessionSnapshotEvidenceV1,
    LOOP_CHECKPOINT_SCHEMA_VERSION, MAX_ID_BYTES, SESSION_CHECKPOINT_DESCRIPTOR_SCHEMA_V1,
    SESSION_CHECKPOINT_ENCODING_V1, SESSION_CHECKPOINT_FORMAT_V1,
    SESSION_CHECKPOINT_LOGICAL_RESUME_SEMANTICS_V1, SESSION_CHECKPOINT_MAX_CONTENT_BYTES,
    SESSION_CHECKPOINT_MEDIA_TYPE_V1, SESSION_CHECKPOINT_PAYLOAD_SCHEMA_V1,
    SESSION_LOGICAL_RESUME_EVIDENCE_SCHEMA_V1, SESSION_SNAPSHOT_EVIDENCE_SCHEMA_V1,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(super) fn validate_payload(
    payload: &SessionCheckpointPayloadV1,
) -> SessionCheckpointResult<()> {
    if payload.schema != SESSION_CHECKPOINT_PAYLOAD_SCHEMA_V1 {
        return Err(SessionCheckpointError::InvalidPayload(
            "payload schema is unsupported".into(),
        ));
    }
    let session_id = &payload.snapshot.session.id;
    payload
        .snapshot
        .validate_for_session(session_id)
        .map_err(|error| SessionCheckpointError::InvalidPayload(error.to_string()))?;
    validate_id("payload session_id", session_id)?;

    if let Some(logical_resume) = &payload.logical_resume {
        if logical_resume.schema_version != LOOP_CHECKPOINT_SCHEMA_VERSION {
            return Err(SessionCheckpointError::InvalidPayload(
                "loop checkpoint is not the exact portable v1 schema".into(),
            ));
        }
        logical_resume
            .ensure_loadable()
            .and_then(|()| logical_resume.ensure_owned_by(&logical_resume.run_id, session_id))
            .map_err(|error| SessionCheckpointError::InvalidPayload(error.to_string()))?;
        if logical_resume.turn == 0 {
            return Err(SessionCheckpointError::InvalidPayload(
                "logical resume has no completed tool-round boundary".into(),
            ));
        }
        let source = payload
            .snapshot
            .run_records
            .iter()
            .find(|record| record.snapshot.id == logical_resume.run_id)
            .ok_or_else(|| {
                SessionCheckpointError::InvalidPayload(format!(
                    "logical resume source run {:?} is absent from the same Session snapshot",
                    logical_resume.run_id
                ))
            })?;
        if source.snapshot.status.is_terminal() {
            return Err(SessionCheckpointError::InvalidPayload(format!(
                "logical resume source run {:?} is already terminal",
                logical_resume.run_id
            )));
        }
        if payload.snapshot.session.cognitive_package_binding
            != source.snapshot.cognitive_package_binding
        {
            return Err(SessionCheckpointError::InvalidPayload(format!(
                "logical resume source run {:?} and its portable Session snapshot carry different cognitive authorities",
                logical_resume.run_id
            )));
        }
        if source.snapshot.capability_binding != logical_resume.capability_binding {
            return Err(SessionCheckpointError::InvalidPayload(format!(
                "logical resume source run {:?} and its checkpoint carry different capability generations",
                logical_resume.run_id
            )));
        }
    }
    Ok(())
}

pub(super) fn build_descriptor(
    payload: &SessionCheckpointPayloadV1,
    content: &[u8],
) -> SessionCheckpointResult<SessionCheckpointDescriptorV1> {
    let snapshot = snapshot_evidence(&payload.snapshot)?;
    let logical_resume = payload
        .logical_resume
        .as_ref()
        .map(logical_resume_evidence)
        .transpose()?;

    let mut descriptor = SessionCheckpointDescriptorV1 {
        schema: SESSION_CHECKPOINT_DESCRIPTOR_SCHEMA_V1.to_string(),
        format: SESSION_CHECKPOINT_FORMAT_V1.to_string(),
        media_type: SESSION_CHECKPOINT_MEDIA_TYPE_V1.to_string(),
        snapshot,
        logical_resume,
        size_bytes: content_size(content)?,
        content_digest: content_digest(content),
        descriptor_digest: String::new(),
    };
    descriptor.descriptor_digest = descriptor.expected_digest()?;
    descriptor.validate()?;
    Ok(descriptor)
}

pub(super) fn snapshot_evidence(
    snapshot: &crate::store::SessionSnapshotV1,
) -> SessionCheckpointResult<SessionSnapshotEvidenceV1> {
    snapshot
        .validate_for_session(&snapshot.session.id)
        .map_err(|error| SessionCheckpointError::InvalidPayload(error.to_string()))?;
    let snapshot_bytes = canonical_json_bytes(snapshot)?;
    ensure_bounded_content(&snapshot_bytes)?;
    let mut evidence = SessionSnapshotEvidenceV1 {
        schema: SESSION_SNAPSHOT_EVIDENCE_SCHEMA_V1.to_string(),
        encoding: SESSION_CHECKPOINT_ENCODING_V1.to_string(),
        session_id: snapshot.session.id.clone(),
        snapshot_schema_version: snapshot.schema_version,
        size_bytes: content_size(&snapshot_bytes)?,
        content_digest: content_digest(&snapshot_bytes),
        evidence_digest: String::new(),
    };
    evidence.evidence_digest = evidence.expected_digest()?;
    evidence.validate()?;
    Ok(evidence)
}

pub(super) fn logical_resume_evidence(
    checkpoint: &crate::loop_checkpoint::LoopCheckpoint,
) -> SessionCheckpointResult<SessionLogicalResumeEvidenceV1> {
    if checkpoint.schema_version != LOOP_CHECKPOINT_SCHEMA_VERSION {
        return Err(SessionCheckpointError::InvalidPayload(
            "loop checkpoint is not the exact portable v1 schema".into(),
        ));
    }
    checkpoint
        .ensure_loadable()
        .map_err(|error| SessionCheckpointError::InvalidPayload(error.to_string()))?;
    validate_id("logical-resume session_id", &checkpoint.session_id)?;
    validate_id("logical-resume source_run_id", &checkpoint.run_id)?;
    if checkpoint.turn == 0 {
        return Err(SessionCheckpointError::InvalidPayload(
            "logical resume has no completed tool-round boundary".into(),
        ));
    }
    let checkpoint_bytes = canonical_json_bytes(checkpoint)?;
    ensure_bounded_content(&checkpoint_bytes)?;
    let completed_tool_rounds = u64::try_from(checkpoint.turn).map_err(|_| {
        SessionCheckpointError::InvalidPayload(
            "tool-round counter cannot be represented by the v1 descriptor".into(),
        )
    })?;
    let mut evidence = SessionLogicalResumeEvidenceV1 {
        schema: SESSION_LOGICAL_RESUME_EVIDENCE_SCHEMA_V1.to_string(),
        resume_semantics: SESSION_CHECKPOINT_LOGICAL_RESUME_SEMANTICS_V1.to_string(),
        session_id: checkpoint.session_id.clone(),
        source_run_id: checkpoint.run_id.clone(),
        checkpoint_schema_version: checkpoint.schema_version,
        completed_tool_rounds,
        checkpoint_ms: checkpoint.checkpoint_ms,
        size_bytes: content_size(&checkpoint_bytes)?,
        content_digest: content_digest(&checkpoint_bytes),
        evidence_digest: String::new(),
    };
    evidence.evidence_digest = evidence.expected_digest()?;
    evidence.validate()?;
    Ok(evidence)
}

pub(super) fn canonical_json_bytes<T: Serialize>(value: &T) -> SessionCheckpointResult<Vec<u8>> {
    let value = serde_json::to_value(value)
        .map_err(|error| SessionCheckpointError::Encoding(error.to_string()))?;
    serde_json::to_vec(&canonicalize_json(value))
        .map_err(|error| SessionCheckpointError::Encoding(error.to_string()))
}

fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let values = entries
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect();
            serde_json::Value::Object(values)
        }
        scalar => scalar,
    }
}

pub(super) fn domain_digest<T: Serialize>(
    domain: &str,
    value: &T,
) -> SessionCheckpointResult<String> {
    let encoded = canonical_json_bytes(value)?;
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub(super) fn content_digest(content: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(content))
}

pub(super) fn content_size(content: &[u8]) -> SessionCheckpointResult<u64> {
    u64::try_from(content.len()).map_err(|_| {
        SessionCheckpointError::InvalidPayload(
            "checkpoint byte length cannot be represented by the v1 descriptor".into(),
        )
    })
}

pub(super) fn ensure_bounded_content(content: &[u8]) -> SessionCheckpointResult<()> {
    if content_size(content)? > SESSION_CHECKPOINT_MAX_CONTENT_BYTES {
        return Err(SessionCheckpointError::InvalidPayload(format!(
            "checkpoint content exceeds the {} byte v1 ceiling",
            SESSION_CHECKPOINT_MAX_CONTENT_BYTES
        )));
    }
    Ok(())
}

pub(super) fn validate_content_identity(
    size_bytes: u64,
    digest: &str,
) -> SessionCheckpointResult<()> {
    if size_bytes == 0 || size_bytes > SESSION_CHECKPOINT_MAX_CONTENT_BYTES {
        return Err(invalid_descriptor(
            "content size is zero or exceeds the v1 ceiling",
        ));
    }
    validate_sha256("content_digest", digest)
}

pub(super) fn validate_sha256(field: &str, value: &str) -> SessionCheckpointResult<()> {
    let valid = value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if valid {
        Ok(())
    } else {
        Err(invalid_descriptor(format!(
            "{field} must be canonical lowercase SHA-256"
        )))
    }
}

pub(super) fn validate_id(field: &str, value: &str) -> SessionCheckpointResult<()> {
    if value.trim().is_empty() || value.len() > MAX_ID_BYTES || value.chars().any(char::is_control)
    {
        return Err(SessionCheckpointError::InvalidPayload(format!(
            "{field} is empty, unbounded, or contains control characters"
        )));
    }
    Ok(())
}

pub(super) fn invalid_descriptor(message: impl Into<String>) -> SessionCheckpointError {
    SessionCheckpointError::InvalidDescriptor(message.into())
}
