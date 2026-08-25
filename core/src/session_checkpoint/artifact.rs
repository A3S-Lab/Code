use super::{
    codec, SessionCheckpointDescriptorV1, SessionCheckpointError, SessionCheckpointPayloadV1,
    SessionCheckpointResult, SessionSnapshotV1, SESSION_CHECKPOINT_PAYLOAD_SCHEMA_V1,
};
use crate::loop_checkpoint::LoopCheckpoint;

/// Exact checkpoint bytes paired with their secret-free descriptor.
#[derive(Clone)]
pub struct SessionCheckpointExportV1 {
    descriptor: SessionCheckpointDescriptorV1,
    content: Vec<u8>,
}

impl std::fmt::Debug for SessionCheckpointExportV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionCheckpointExportV1")
            .field("descriptor", &self.descriptor)
            .field("content", &"<redacted>")
            .finish()
    }
}

impl SessionCheckpointExportV1 {
    pub fn new(
        snapshot: SessionSnapshotV1,
        logical_resume: Option<LoopCheckpoint>,
    ) -> SessionCheckpointResult<Self> {
        let payload = SessionCheckpointPayloadV1 {
            schema: SESSION_CHECKPOINT_PAYLOAD_SCHEMA_V1.to_string(),
            snapshot,
            logical_resume,
        };
        codec::validate_payload(&payload)?;
        let content = codec::canonical_json_bytes(&payload)?;
        codec::ensure_bounded_content(&content)?;
        let descriptor = codec::build_descriptor(&payload, &content)?;
        Self::from_parts(descriptor, content)
    }

    pub fn from_parts(
        descriptor: SessionCheckpointDescriptorV1,
        content: Vec<u8>,
    ) -> SessionCheckpointResult<Self> {
        validate_and_decode(&descriptor, &content)?;
        Ok(Self {
            descriptor,
            content,
        })
    }

    pub fn descriptor(&self) -> &SessionCheckpointDescriptorV1 {
        &self.descriptor
    }

    pub fn content(&self) -> &[u8] {
        &self.content
    }

    pub fn open(&self) -> SessionCheckpointResult<SessionCheckpointPayloadV1> {
        validate_and_decode(&self.descriptor, &self.content)
    }

    /// Consume and decode this export after revalidating its complete identity.
    ///
    /// This avoids retaining a second potentially large payload allocation in
    /// restore-admission paths that already own the export.
    pub fn into_open(self) -> SessionCheckpointResult<SessionCheckpointPayloadV1> {
        validate_and_decode(&self.descriptor, &self.content)
    }

    pub fn into_content(self) -> Vec<u8> {
        self.content
    }

    pub fn into_parts(self) -> (SessionCheckpointDescriptorV1, Vec<u8>) {
        (self.descriptor, self.content)
    }
}

fn validate_and_decode(
    descriptor: &SessionCheckpointDescriptorV1,
    content: &[u8],
) -> SessionCheckpointResult<SessionCheckpointPayloadV1> {
    descriptor.validate()?;
    codec::ensure_bounded_content(content)?;
    let actual_size = codec::content_size(content)?;
    if descriptor.size_bytes != actual_size
        || descriptor.content_digest != codec::content_digest(content)
    {
        return Err(SessionCheckpointError::ContentDrift(
            "descriptor does not match the exact supplied bytes".into(),
        ));
    }

    let payload: SessionCheckpointPayloadV1 = serde_json::from_slice(content)
        .map_err(|error| SessionCheckpointError::InvalidPayload(error.to_string()))?;
    codec::validate_payload(&payload)?;
    let canonical = codec::canonical_json_bytes(&payload)?;
    if canonical != content {
        return Err(SessionCheckpointError::ContentDrift(
            "payload is not the exact canonical JSON encoding".into(),
        ));
    }
    let expected = codec::build_descriptor(&payload, content)?;
    if descriptor != &expected {
        return Err(SessionCheckpointError::ContentDrift(
            "descriptor does not match the decoded checkpoint components".into(),
        ));
    }
    Ok(payload)
}
