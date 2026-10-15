//! Host data carried by generic Evaluation payloads; no verdict/store authority.

use super::manifest::{validate_id, ContractError, DigestV1, CASE_COUNT};
use a3s_code_core::content_digest::digest_json;
use a3s_code_core::evaluation::ExecutionTargetV1;
use a3s_code_core::harness_loop::CompletionTerminal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PAYLOAD_SCHEMA: &str = "a3s.code.behavior-acceptance.v1";
pub const REQUEST_SCHEMA: &str = "a3s.code.behavior-acceptance-request.v1";
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

macro_rules! closed_set {
    ($name:ident { $($variant:ident => $token:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $token)] $variant),+ }
        impl $name {
            pub fn parse(value: &str) -> Result<Self, ContractError> {
                match value { $($token => Ok(Self::$variant),)+ _ => Err(ContractError::ClosedSet {
                    set: stringify!($name), token: value.to_string(),
                }) }
            }
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $token),+ } }
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}

closed_set!(ArtifactBehaviorV1 { Accepted => "accepted", Rejected => "rejected", Incomplete => "incomplete" });
closed_set!(AcceptanceReasonV1 {
    AllCasesPassed => "all_cases_passed", BehaviorMismatch => "behavior_mismatch",
    EvidenceIncomplete => "evidence_incomplete", MakerUnresolved => "maker_unresolved", MakerNotAllowed => "maker_not_allowed",
    FreezeInvalid => "freeze_invalid", FreezeIncomplete => "freeze_incomplete", FreezeRace => "freeze_race", StaleCandidate => "stale_candidate",
    IsolationUnqualified => "isolation_unqualified", IsolationUnavailable => "isolation_unavailable",
    IdentityMismatch => "identity_mismatch", ArtifactMissing => "artifact_missing", RecordConflict => "record_conflict",
    ZeroCases => "zero_cases", CaseCoverageMismatch => "case_coverage_mismatch",
    CandidateTimeout => "candidate_timeout", CandidateCancelled => "candidate_cancelled", CandidateCrash => "candidate_crash",
    OutputLimit => "output_limit", InvalidWire => "invalid_wire", ResourcesUnsettled => "resources_unsettled",
    ProviderFailure => "provider_failure", BudgetNotAdmitted => "budget_not_admitted", CostUnknown => "cost_unknown",
    HostStorageFailure => "host_storage_failure",
});
closed_set!(ResourceSettlementV1 { Settled => "settled", Unsettled => "unsettled", Unknown => "unknown" });
closed_set!(NativeQualificationStatusV1 { Qualified => "qualified", Unqualified => "unqualified", Unavailable => "unavailable" });
closed_set!(ResourceLimitStatusV1 { Enforced => "enforced", Measured => "measured", Unavailable => "unavailable" });
closed_set!(AttemptRoleV1 { First => "first", Remediation => "remediation" });
closed_set!(FailureOriginV1 { None => "none", HostInjectedCancel => "host_injected_cancel", Provider => "provider", Candidate => "candidate", Infrastructure => "infrastructure" });
closed_set!(BillingStatusV1 { Reported => "reported", Unknown => "unknown", Partial => "partial" });
closed_set!(UsageUncertaintyPolicyV1 { RetainUnknown => "retain_unknown" });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptIdentityV1 {
    pub suite_attempt_id: String,
    pub logical_attempt_id: String,
    pub role: AttemptRoleV1,
    pub predecessor_id: Option<String>,
}
impl AttemptIdentityV1 {
    pub fn validate(&self) -> Result<(), ContractError> {
        validate_id(&self.suite_attempt_id)?;
        validate_id(&self.logical_attempt_id)?;
        match (self.role, &self.predecessor_id) {
            (AttemptRoleV1::First, None) => Ok(()),
            (AttemptRoleV1::Remediation, Some(previous))
                if previous != &self.logical_attempt_id =>
            {
                validate_id(previous)
            }
            _ => Err(report_error("attempt role and predecessor disagree")),
        }
    }
}

/// Every input that changes replay identity. Observed result data is separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceRequestV1 {
    pub schema: String,
    pub manifest_digest: DigestV1,
    pub attempt: AttemptIdentityV1,
    pub maker_target: ExecutionTargetV1,
    #[serde(deserialize_with = "deserialize_completion")]
    pub maker_completion: CompletionTerminal,
    pub effect_digest: DigestV1,
    pub candidate_byte_digest: DigestV1,
    pub verifier_digest: DigestV1,
    pub environment_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub original_receipt_capture_digest: DigestV1,
    pub settlement_evidence_digest: DigestV1,
}
impl AcceptanceRequestV1 {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema != REQUEST_SCHEMA {
            return Err(report_error("unsupported request schema"));
        }
        self.attempt.validate()?;
        self.maker_target
            .validate()
            .map_err(|error| report_error(error.to_string()))?;
        if let CompletionTerminal::Verified { effect_digest }
        | CompletionTerminal::Waived { effect_digest } = &self.maker_completion
        {
            if effect_digest != self.effect_digest.as_str() {
                return Err(report_error(
                    "immutable maker completion has another effect binding",
                ));
            }
        }
        Ok(())
    }
    pub fn identity(&self) -> Result<DigestV1, ContractError> {
        self.validate()?;
        DigestV1::new(
            digest_json("a3s.code.behavior-acceptance-request.identity.v1", self)
                .map_err(|error| report_error(error.to_string()))?,
        )
    }
    pub fn auxiliary_id(&self) -> Result<String, ContractError> {
        Ok(format!("behavior-acceptance:{}", self.identity()?.as_str()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCaseReceiptV1 {
    pub case_id: String,
    pub ordinal: u64,
    pub input_digest: DigestV1,
    pub maker_target: ExecutionTargetV1,
    pub request_identity: DigestV1,
    pub candidate_byte_digest: DigestV1,
    pub verifier_digest: DigestV1,
    pub environment_digest: DigestV1,
    pub response_digest: DigestV1,
    pub response_byte_count: u64,
    pub stderr_digest: DigestV1,
    pub stderr_byte_count: u64,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_complete: bool,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub resource_settlement: ResourceSettlementV1,
    pub outcome: ArtifactBehaviorV1,
    pub reason: AcceptanceReasonV1,
}
impl AcceptanceCaseReceiptV1 {
    pub fn validate(&self) -> Result<(), ContractError> {
        validate_id(&self.case_id)?;
        self.maker_target
            .validate()
            .map_err(|error| report_error(error.to_string()))?;
        if self.ordinal >= CASE_COUNT as u64 || self.finished_at_ms < self.started_at_ms {
            return Err(report_error("invalid case ordinal/timestamps"));
        }
        if self.outcome == ArtifactBehaviorV1::Accepted
            && (self.timed_out
                || !self.output_complete
                || self.exit_code.is_none()
                || self.resource_settlement != ResourceSettlementV1::Settled
                || self.reason != AcceptanceReasonV1::AllCasesPassed)
        {
            return Err(report_error(
                "accepted case lacks a complete settled observation",
            ));
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<DigestV1, ContractError> {
        self.validate()?;
        DigestV1::new(
            digest_json("a3s.code.behavior-acceptance-case-receipt.v1", self)
                .map_err(|error| report_error(error.to_string()))?,
        )
    }
}

/// Structural validation alone never authorizes delivery. T007 must prove the
/// full original-evidence/native/check conjunction before constructing a pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptancePayloadV1 {
    pub schema: String,
    pub request_identity: DigestV1,
    pub task_id: String,
    pub cluster_id: String,
    pub variant: String,
    pub manifest_digest: DigestV1,
    pub maker_target: ExecutionTargetV1,
    #[serde(deserialize_with = "deserialize_completion")]
    pub maker_completion: CompletionTerminal,
    pub effect_digest: DigestV1,
    pub candidate_byte_digest: DigestV1,
    pub verifier_digest: DigestV1,
    pub environment_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub original_receipt_capture_digest: DigestV1,
    pub settlement_evidence_digest: DigestV1,
    pub artifact_behavior: ArtifactBehaviorV1,
    pub delivery_accepted: bool,
    pub reasons: Vec<AcceptanceReasonV1>,
    pub expected_case_count: u64,
    pub executed_case_count: u64,
    pub case_receipt_digests: Vec<DigestV1>,
    pub verification_report_digest: DigestV1,
    pub resource_settlement: ResourceSettlementV1,
    pub waiver_present: bool,
    pub attempt: AttemptIdentityV1,
    pub observed_at_ms: u64,
}
impl AcceptancePayloadV1 {
    pub fn request(&self) -> AcceptanceRequestV1 {
        AcceptanceRequestV1 {
            schema: REQUEST_SCHEMA.into(),
            manifest_digest: self.manifest_digest.clone(),
            attempt: self.attempt.clone(),
            maker_target: self.maker_target.clone(),
            maker_completion: self.maker_completion.clone(),
            effect_digest: self.effect_digest.clone(),
            candidate_byte_digest: self.candidate_byte_digest.clone(),
            verifier_digest: self.verifier_digest.clone(),
            environment_digest: self.environment_digest.clone(),
            evidence_digest: self.evidence_digest.clone(),
            original_receipt_capture_digest: self.original_receipt_capture_digest.clone(),
            settlement_evidence_digest: self.settlement_evidence_digest.clone(),
        }
    }
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema != PAYLOAD_SCHEMA {
            return Err(report_error("unsupported payload schema"));
        }
        for value in [&self.task_id, &self.cluster_id, &self.variant] {
            validate_id(value)?;
        }
        if self.request().identity()? != self.request_identity {
            return Err(report_error("request identity mismatch"));
        }
        require_reasons(&self.reasons)?;
        let receipts: BTreeSet<_> = self.case_receipt_digests.iter().collect();
        if self.expected_case_count != CASE_COUNT as u64
            || self.executed_case_count > self.expected_case_count
            || self.case_receipt_digests.len() as u64 != self.executed_case_count
            || receipts.len() != self.case_receipt_digests.len()
        {
            return Err(report_error(
                "case counts/digests disagree or contain duplicates",
            ));
        }
        let positive = self.executed_case_count == self.expected_case_count
            && self.resource_settlement == ResourceSettlementV1::Settled;
        if (self.artifact_behavior == ArtifactBehaviorV1::Accepted
            || self.reasons.contains(&AcceptanceReasonV1::AllCasesPassed))
            && !positive
        {
            return Err(report_error(
                "positive behavior lacks exact nonzero settled case coverage",
            ));
        }
        if self.delivery_accepted
            && (self.artifact_behavior != ArtifactBehaviorV1::Accepted
                || !positive
                || self.waiver_present
                || matches!(
                    self.maker_completion,
                    CompletionTerminal::Waived { .. } | CompletionTerminal::Distinct
                ))
        {
            return Err(report_error(
                "delivery conflicts with artifact, coverage, waiver or maker completion",
            ));
        }
        let encoded = serde_json::to_vec(self).map_err(|error| report_error(error.to_string()))?;
        if encoded.len() > MAX_PAYLOAD_BYTES {
            return Err(report_error("payload exceeds 256 KiB"));
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<DigestV1, ContractError> {
        self.validate()?;
        DigestV1::new(
            digest_json("a3s.code.behavior-acceptance-payload.v1", self)
                .map_err(|error| report_error(error.to_string()))?,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptAccountingV1 {
    pub attempt: AttemptIdentityV1,
    pub failure_origin: FailureOriginV1,
    pub provider_model_route: String,
    pub provider_revision: Option<String>,
    pub model_revision: Option<String>,
    pub provider_invocation_ids: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_usage")]
    pub reported_usage: Option<a3s_code_core::llm::TokenUsage>,
    pub estimated_tokens: Option<u64>,
    pub estimated_money_microusd: Option<u64>,
    pub reserved_tokens: Option<u64>,
    pub reserved_money_microusd: Option<u64>,
    pub known_retry_count: Option<u64>,
    pub known_fallback_count: Option<u64>,
    pub billing_status: BillingStatusV1,
    pub cost_complete: bool,
    pub uncertainty_reasons: Vec<AcceptanceReasonV1>,
}
impl AttemptAccountingV1 {
    pub fn validate(&self) -> Result<(), ContractError> {
        self.attempt.validate()?;
        validate_id(&self.provider_model_route)?;
        for value in self
            .provider_invocation_ids
            .iter()
            .chain(self.provider_revision.iter())
            .chain(self.model_revision.iter())
        {
            validate_id(value)?;
        }
        if self
            .uncertainty_reasons
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != self.uncertainty_reasons.len()
        {
            return Err(report_error("duplicate accounting reasons"));
        }
        let reported_usage_observed = self.reported_usage.as_ref().is_some_and(|usage| {
            usage.prompt_tokens > 0 || usage.completion_tokens > 0 || usage.total_tokens > 0
        });
        if self.cost_complete
            && (self.billing_status != BillingStatusV1::Reported
                || !reported_usage_observed
                || self.known_retry_count.is_none()
                || self.known_fallback_count.is_none()
                || !self.uncertainty_reasons.is_empty())
        {
            return Err(report_error(
                "unknown usage, retry or billing cannot be cost complete",
            ));
        }
        Ok(())
    }
}

fn require_reasons(reasons: &[AcceptanceReasonV1]) -> Result<(), ContractError> {
    if reasons.is_empty() || reasons.iter().collect::<BTreeSet<_>>().len() != reasons.len() {
        return Err(report_error("reasons must be nonempty, ordered and unique"));
    }
    Ok(())
}
fn report_error(value: impl Into<String>) -> ContractError {
    ContractError::Report(value.into())
}

// Existing Core types retain their own wire contract. These private decoders
// only make the frozen host projection strict; they add no terminal or usage
// state and leave the existing public types and serializer unchanged.
fn deserialize_completion<'de, D>(deserializer: D) -> Result<CompletionTerminal, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        kind: String,
        #[serde(
            default,
            deserialize_with = "present_value",
            skip_serializing_if = "Option::is_none"
        )]
        effect_digest: Option<serde_json::Value>,
    }
    fn present_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde_json::Value::deserialize(deserializer).map(Some)
    }
    let wire = Wire::deserialize(deserializer)?;
    let value = serde_json::to_value(wire).map_err(serde::de::Error::custom)?;
    let terminal: CompletionTerminal =
        serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
    if serde_json::to_value(&terminal).map_err(serde::de::Error::custom)? != value {
        return Err(serde::de::Error::custom(
            "maker_completion fields disagree with the existing terminal wire",
        ));
    }
    Ok(terminal)
}

fn deserialize_usage<'de, D>(
    deserializer: D,
) -> Result<Option<a3s_code_core::llm::TokenUsage>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        prompt_tokens: usize,
        completion_tokens: usize,
        total_tokens: usize,
        cache_read_tokens: Option<usize>,
        cache_write_tokens: Option<usize>,
    }
    Ok(
        Option::<Wire>::deserialize(deserializer)?.map(|wire| a3s_code_core::llm::TokenUsage {
            prompt_tokens: wire.prompt_tokens,
            completion_tokens: wire.completion_tokens,
            total_tokens: wire.total_tokens,
            cache_read_tokens: wire.cache_read_tokens,
            cache_write_tokens: wire.cache_write_tokens,
        }),
    )
}

#[cfg(test)]
mod tests;
