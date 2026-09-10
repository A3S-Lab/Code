//! Durable session-scoped review findings for sticky reply / multi-scenario review.
//!
//! # First principles
//!
//! Review exists to independently verify claim↔record, persist findings beside
//! the session transcript, and drive real correction — not to paint a local
//! "resolved" badge.
//!
//! Lifecycle (host-enforced):
//! `pending` → `addressed` (main agent) → `accepted` (reviewer) |
//! `pending` ← `reopen` from `addressed` | `waived` from `pending`.
//!
//! Only `pending` findings whose registered [`ReviewScenario`] opts into
//! `injects_into_main` are prefixed into the next main-agent prompt.
//!
//! # Core vs extension
//!
//! - **Core:** lifecycle, [`SessionReviewStoreV1`] snapshot field, inject fence,
//!   [`ReviewSubjectV1`], scenario registry APIs.
//! - **Extensions:** [`ReviewScenario`] implementations (hosts / Use packages)
//!   own rubrics and evidence collection. Core never matches on scenario
//!   business rules beyond registry lookup.
//!
//! Distinct from [`crate::research`] review contracts: those bind research
//! artifacts and digests.

mod error;
mod finding;
mod scenario;
mod store;
mod subject;

pub use error::SessionReviewError;
pub use finding::{
    SessionReviewAnchorV1, SessionReviewFindingV1, SessionReviewSeverityV1, SessionReviewStatusV1,
    SESSION_REVIEW_FINDING_SCHEMA_V1,
};
pub use scenario::{
    admit_finding_drafts, register_default_scenarios, ReplyTranscriptScenario, ReviewFindingDraft,
    ReviewScenario, ReviewScenarioRegistry, ReviewTriggerPolicy, SCENARIO_REPLY_TRANSCRIPT,
};
pub use store::{SessionReviewStoreV1, SESSION_REVIEW_STORE_SCHEMA_V1};
pub use subject::ReviewSubjectV1;

/// Maximum JSON payload for one finding or store document.
pub const SESSION_REVIEW_MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
pub const SESSION_REVIEW_MAX_ID_BYTES: usize = 256;
pub const SESSION_REVIEW_MAX_TEXT_BYTES: usize = 16 * 1024;
pub const SESSION_REVIEW_MAX_FINDINGS: usize = 256;

pub(crate) fn decode_json_slice<T>(bytes: &[u8]) -> Result<T, SessionReviewError>
where
    T: serde::de::DeserializeOwned,
{
    if bytes.len() > SESSION_REVIEW_MAX_MESSAGE_BYTES {
        return Err(SessionReviewError::Encoding);
    }
    serde_json::from_slice(bytes)
        .map_err(|error| SessionReviewError::Serialization(error.to_string()))
}

pub(crate) fn encode_json<T: serde::Serialize + ?Sized>(
    value: &T,
) -> Result<Vec<u8>, SessionReviewError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| SessionReviewError::Serialization(error.to_string()))?;
    if bytes.len() > SESSION_REVIEW_MAX_MESSAGE_BYTES {
        return Err(SessionReviewError::Encoding);
    }
    Ok(bytes)
}

pub(crate) fn validate_id(field: &'static str, value: &str) -> Result<(), SessionReviewError> {
    if value.is_empty()
        || value.len() > SESSION_REVIEW_MAX_ID_BYTES
        || value.contains('\0')
        || value.contains(['\r', '\n'])
    {
        return Err(SessionReviewError::InvalidField(field));
    }
    Ok(())
}

/// Claim / evidence / suggestion may contain newlines; reject NUL and emptiness.
pub(crate) fn validate_multiline_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), SessionReviewError> {
    if value.is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(SessionReviewError::InvalidField(field));
    }
    Ok(())
}
