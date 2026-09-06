//! Native research-run contracts for the A3S Code scientific workflow.
//!
//! This module contains bounded, digest-bound transport values. It does not
//! resolve packages, choose scientific methods, or decide whether a finding
//! is acceptable. A3S Use owns package and environment authority; hosts own
//! scientific policy and human decisions.

mod error;
mod event;
mod evidence;
mod provenance;
mod review;
mod review_batch;
mod run;

pub use error::ResearchContractError;
pub use event::{ResearchEventV1, RESEARCH_EVENT_SCHEMA_V1, RESEARCH_MAX_EVENT_TYPE_BYTES};
pub use evidence::{
    ResearchEvidenceFactKindV1, ResearchEvidenceFactV1, RESEARCH_EVIDENCE_FACT_SCHEMA_V1,
    RESEARCH_MAX_FACT_METADATA, RESEARCH_MAX_METADATA_VALUE_BYTES,
};
pub use provenance::{
    ResearchArtifactKindV1, ResearchProvenanceReceiptV1, RESEARCH_ARTIFACT_KINDS,
    RESEARCH_PROVENANCE_RECEIPT_SCHEMA_V1,
};
pub use review::{
    ResearchReviewCategoryV1, ResearchReviewFindingV1, ResearchReviewLocationV1,
    ResearchReviewSeverityV1, ResearchReviewStatusV1, RESEARCH_REVIEW_FINDING_SCHEMA_V1,
};
pub use review_batch::{
    ResearchReviewBatchV1, RESEARCH_MAX_REVIEW_FINDINGS, RESEARCH_REVIEW_BATCH_SCHEMA_V1,
};
pub use run::{
    ResearchReproducibilityV1, ResearchRunStatusV1, ResearchRunV1, RESEARCH_RUN_SCHEMA_V1,
};

pub(crate) const RESEARCH_MAX_ID_BYTES: usize = 256;
pub(crate) const RESEARCH_MAX_TEXT_BYTES: usize = 16 * 1024;
pub(crate) const RESEARCH_MAX_DIGESTS: usize = 512;

pub(crate) fn validate_id(field: &'static str, value: &str) -> Result<(), ResearchContractError> {
    if value.is_empty()
        || value.len() > RESEARCH_MAX_ID_BYTES
        || value.contains('\0')
        || value.lines().count() != 1
    {
        return Err(ResearchContractError::InvalidField(field));
    }
    Ok(())
}

pub(crate) fn validate_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ResearchContractError> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.contains('\0')
        || value.lines().count() > 1
    {
        return Err(ResearchContractError::InvalidField(field));
    }
    Ok(())
}

pub(crate) fn validate_digest_field(
    field: &'static str,
    value: &str,
) -> Result<(), ResearchContractError> {
    crate::evaluation::validate_digest(value)
        .map_err(|_| ResearchContractError::InvalidDigest(field))
}

pub(crate) fn digest<T: serde::Serialize>(
    domain: &'static str,
    value: &T,
) -> Result<String, ResearchContractError> {
    crate::evaluation::digest_json(domain, value)
        .map_err(|error| ResearchContractError::Serialization(error.to_string()))
}
