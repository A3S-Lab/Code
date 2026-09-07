//! Research workflow-plan bridge over existing execution result receipts.

use super::{
    digest, validate_digest_field, validate_id, ResearchContractError, RESEARCH_MAX_DIGESTS,
};
use crate::execution_identity::ExecutionResultReceiptV1;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const RESEARCH_WORKFLOW_STEP_SCHEMA_V1: &str = "a3s.code.research-workflow-step.v1";
pub const RESEARCH_WORKFLOW_PLAN_SCHEMA_V1: &str = "a3s.code.research-workflow-plan.v1";
pub const RESEARCH_RERUN_LINEAGE_SCHEMA_V1: &str = "a3s.code.research-rerun-lineage.v1";
pub const RESEARCH_MAX_WORKFLOW_STEPS: usize = 512;
const RESEARCH_WORKFLOW_STEP_DIGEST_DOMAIN: &str = "a3s.code.research-workflow-step.identity.v1";
const RESEARCH_WORKFLOW_PLAN_DIGEST_DOMAIN: &str = "a3s.code.research-workflow-plan.identity.v1";
const RESEARCH_WORKFLOW_RECEIPT_DIGEST_DOMAIN: &str =
    "a3s.code.research-workflow-receipt.identity.v1";
const RESEARCH_RERUN_LINEAGE_DIGEST_DOMAIN: &str = "a3s.code.research-rerun-lineage.identity.v1";

/// One research-visible workflow step bound to digests and optional receipts.
///
/// Code does not schedule the step. It only records the exact identity and
/// artifact/input digests so a later review finding can select a minimal
/// re-run set without rewriting the parent evidence ledger.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchWorkflowStepV1 {
    pub schema: String,
    pub step_id: String,
    pub project_id: String,
    pub run_id: String,
    pub workflow_digest: String,
    pub step_identity_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_receipt_digest: Option<String>,
    pub input_digests: Vec<String>,
    pub output_artifact_digests: Vec<String>,
    pub depends_on: Vec<String>,
    pub step_digest: String,
}

impl ResearchWorkflowStepV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        step_id: impl Into<String>,
        project_id: impl Into<String>,
        run_id: impl Into<String>,
        workflow_digest: impl Into<String>,
        step_identity_digest: impl Into<String>,
        mut input_digests: Vec<String>,
        mut output_artifact_digests: Vec<String>,
        mut depends_on: Vec<String>,
    ) -> Result<Self, ResearchContractError> {
        input_digests.sort();
        input_digests.dedup();
        output_artifact_digests.sort();
        output_artifact_digests.dedup();
        depends_on.sort();
        depends_on.dedup();
        let mut step = Self {
            schema: RESEARCH_WORKFLOW_STEP_SCHEMA_V1.to_owned(),
            step_id: step_id.into(),
            project_id: project_id.into(),
            run_id: run_id.into(),
            workflow_digest: workflow_digest.into(),
            step_identity_digest: step_identity_digest.into(),
            result_receipt_digest: None,
            input_digests,
            output_artifact_digests,
            depends_on,
            step_digest: String::new(),
        };
        step.validate_without_digest()?;
        step.step_digest = step.expected_digest()?;
        Ok(step)
    }

    /// Bind the exact execution result receipt produced for this step.
    ///
    /// The receipt identity digest must match the admitted step identity. The
    /// receipt evidence digest must appear in the step inputs so a reviewer
    /// cannot attach an unrelated terminal outcome.
    pub fn bind_result_receipt(
        mut self,
        receipt: &ExecutionResultReceiptV1,
    ) -> Result<Self, ResearchContractError> {
        self.validate()?;
        receipt
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("resultReceipt"))?;
        if receipt.identity.digest != self.step_identity_digest {
            return Err(ResearchContractError::InvalidField("stepIdentityDigest"));
        }
        if !self
            .input_digests
            .iter()
            .any(|digest| digest == &receipt.evidence_digest)
        {
            return Err(ResearchContractError::InvalidField("evidenceDigest"));
        }
        let receipt_digest = digest(RESEARCH_WORKFLOW_RECEIPT_DIGEST_DOMAIN, receipt)
            .map_err(|error| ResearchContractError::Serialization(error.to_string()))?;
        if let Some(existing) = &self.result_receipt_digest {
            if existing != &receipt_digest {
                return Err(ResearchContractError::InvalidField("resultReceiptDigest"));
            }
            return Ok(self);
        }
        self.result_receipt_digest = Some(receipt_digest);
        self.step_digest = self.expected_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn validate_for_run(
        &self,
        run: &crate::research::ResearchRunV1,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        if self.project_id != run.project_id {
            return Err(ResearchContractError::InvalidField("projectId"));
        }
        if self.run_id != run.run_id {
            return Err(ResearchContractError::InvalidField("runId"));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("stepDigest", &self.step_digest)?;
        if self.step_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("stepDigest"));
        }
        Ok(())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let step: Self = super::decode_json_slice(bytes)?;
        step.validate()?;
        Ok(step)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_WORKFLOW_STEP_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("stepId", &self.step_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_digest_field("workflowDigest", &self.workflow_digest)?;
        validate_digest_field("stepIdentityDigest", &self.step_identity_digest)?;
        if let Some(receipt_digest) = &self.result_receipt_digest {
            validate_digest_field("resultReceiptDigest", receipt_digest)?;
        }
        if self.input_digests.is_empty() || self.input_digests.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("inputDigests"));
        }
        if self.output_artifact_digests.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("outputArtifactDigests"));
        }
        if self.depends_on.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("dependsOn"));
        }
        validate_sorted_unique_digests("inputDigests", &self.input_digests)?;
        validate_sorted_unique_digests("outputArtifactDigests", &self.output_artifact_digests)?;
        for pair in self.depends_on.windows(2) {
            if pair[0] >= pair[1] {
                return Err(ResearchContractError::InvalidField("dependsOn"));
            }
        }
        for dependency in &self.depends_on {
            validate_id("dependsOn", dependency)?;
            if dependency == &self.step_id {
                return Err(ResearchContractError::InvalidField("dependsOn"));
            }
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            step_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            workflow_digest: &'a str,
            step_identity_digest: &'a str,
            result_receipt_digest: Option<&'a str>,
            input_digests: &'a [String],
            output_artifact_digests: &'a [String],
            depends_on: &'a [String],
        }
        digest(
            RESEARCH_WORKFLOW_STEP_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                step_id: &self.step_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                workflow_digest: &self.workflow_digest,
                step_identity_digest: &self.step_identity_digest,
                result_receipt_digest: self.result_receipt_digest.as_deref(),
                input_digests: &self.input_digests,
                output_artifact_digests: &self.output_artifact_digests,
                depends_on: &self.depends_on,
            },
        )
    }
}

/// Bounded workflow projection for one research Run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchWorkflowPlanV1 {
    pub schema: String,
    pub plan_id: String,
    pub project_id: String,
    pub run_id: String,
    pub workflow_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random_seed: Option<u64>,
    pub steps: Vec<ResearchWorkflowStepV1>,
    pub plan_digest: String,
}

impl ResearchWorkflowPlanV1 {
    pub fn new_for_run(
        plan_id: impl Into<String>,
        run: &crate::research::ResearchRunV1,
        workflow_digest: impl Into<String>,
        steps: Vec<ResearchWorkflowStepV1>,
    ) -> Result<Self, ResearchContractError> {
        let plan = Self::new(
            plan_id,
            run.project_id.clone(),
            run.run_id.clone(),
            workflow_digest,
            run.random_seed,
            steps,
        )?;
        plan.validate_for_run(run)?;
        Ok(plan)
    }

    pub fn new(
        plan_id: impl Into<String>,
        project_id: impl Into<String>,
        run_id: impl Into<String>,
        workflow_digest: impl Into<String>,
        random_seed: Option<u64>,
        mut steps: Vec<ResearchWorkflowStepV1>,
    ) -> Result<Self, ResearchContractError> {
        steps.sort_unstable_by(|left, right| left.step_id.cmp(&right.step_id));
        let mut plan = Self {
            schema: RESEARCH_WORKFLOW_PLAN_SCHEMA_V1.to_owned(),
            plan_id: plan_id.into(),
            project_id: project_id.into(),
            run_id: run_id.into(),
            workflow_digest: workflow_digest.into(),
            random_seed,
            steps,
            plan_digest: String::new(),
        };
        plan.validate_without_digest()?;
        plan.plan_digest = plan.expected_digest()?;
        Ok(plan)
    }

    pub fn validate_for_run(
        &self,
        run: &crate::research::ResearchRunV1,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        if self.project_id != run.project_id {
            return Err(ResearchContractError::InvalidField("projectId"));
        }
        if self.run_id != run.run_id {
            return Err(ResearchContractError::InvalidField("runId"));
        }
        if self.random_seed != run.random_seed {
            return Err(ResearchContractError::InvalidField("randomSeed"));
        }
        for step in &self.steps {
            step.validate_for_run(run)?;
            if step.workflow_digest != self.workflow_digest {
                return Err(ResearchContractError::InvalidField("workflowDigest"));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("planDigest", &self.plan_digest)?;
        if self.plan_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("planDigest"));
        }
        Ok(())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let plan: Self = super::decode_json_slice(bytes)?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    /// Return affected step ids in dependency-first order for a re-run.
    pub fn affected_step_ids_for_findings(
        &self,
        findings: &[crate::research::ResearchReviewFindingV1],
    ) -> Result<Vec<String>, ResearchContractError> {
        self.validate()?;
        let mut directly_affected = BTreeSet::new();
        for finding in findings {
            finding.validate()?;
            if finding.project_id != self.project_id {
                return Err(ResearchContractError::InvalidField("finding.projectId"));
            }
            if finding.run_id != self.run_id {
                return Err(ResearchContractError::InvalidField("finding.runId"));
            }
            for step in &self.steps {
                if step
                    .output_artifact_digests
                    .iter()
                    .any(|digest| digest == &finding.artifact_digest)
                    || finding
                        .evidence_digests
                        .iter()
                        .any(|digest| step.input_digests.contains(digest))
                    || finding.evidence_digests.iter().any(|digest| {
                        step.output_artifact_digests
                            .iter()
                            .any(|output| output == digest)
                    })
                {
                    directly_affected.insert(step.step_id.clone());
                }
            }
        }
        close_dependents(&self.steps, directly_affected)
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_WORKFLOW_PLAN_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("planId", &self.plan_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_digest_field("workflowDigest", &self.workflow_digest)?;
        if self.steps.len() > RESEARCH_MAX_WORKFLOW_STEPS {
            return Err(ResearchContractError::InvalidField("steps"));
        }
        for pair in self.steps.windows(2) {
            if pair[0].step_id >= pair[1].step_id {
                return Err(ResearchContractError::InvalidField("steps"));
            }
        }
        let mut step_ids = BTreeMap::new();
        for step in &self.steps {
            step.validate()?;
            if step.project_id != self.project_id {
                return Err(ResearchContractError::InvalidField("step.projectId"));
            }
            if step.run_id != self.run_id {
                return Err(ResearchContractError::InvalidField("step.runId"));
            }
            if step.workflow_digest != self.workflow_digest {
                return Err(ResearchContractError::InvalidField("workflowDigest"));
            }
            if step_ids.insert(step.step_id.as_str(), ()).is_some() {
                return Err(ResearchContractError::InvalidField("stepId"));
            }
        }
        for step in &self.steps {
            for dependency in &step.depends_on {
                if !step_ids.contains_key(dependency.as_str()) {
                    return Err(ResearchContractError::InvalidField("dependsOn"));
                }
            }
        }
        detect_dependency_cycles(&self.steps)?;
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            plan_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            workflow_digest: &'a str,
            random_seed: Option<u64>,
            step_digests: Vec<&'a str>,
        }
        digest(
            RESEARCH_WORKFLOW_PLAN_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                plan_id: &self.plan_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                workflow_digest: &self.workflow_digest,
                random_seed: self.random_seed,
                step_digests: self
                    .steps
                    .iter()
                    .map(|step| step.step_digest.as_str())
                    .collect(),
            },
        )
    }
}

/// Immutable projection of finding-triggered affected-step re-run lineage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchRerunLineageV1 {
    pub schema: String,
    pub lineage_id: String,
    pub project_id: String,
    pub run_id: String,
    pub plan_digest: String,
    pub finding_ids: Vec<String>,
    pub affected_step_ids: Vec<String>,
    pub lineage_digest: String,
}

impl ResearchRerunLineageV1 {
    pub fn from_findings_for_plan(
        lineage_id: impl Into<String>,
        plan: &ResearchWorkflowPlanV1,
        findings: &[crate::research::ResearchReviewFindingV1],
    ) -> Result<Self, ResearchContractError> {
        plan.validate()?;
        let mut finding_ids: Vec<String> = findings
            .iter()
            .map(|finding| finding.finding_id.clone())
            .collect();
        finding_ids.sort();
        finding_ids.dedup();
        if finding_ids.len() != findings.len() {
            return Err(ResearchContractError::InvalidField("findingId"));
        }
        let affected_step_ids = plan.affected_step_ids_for_findings(findings)?;
        let mut lineage = Self {
            schema: RESEARCH_RERUN_LINEAGE_SCHEMA_V1.to_owned(),
            lineage_id: lineage_id.into(),
            project_id: plan.project_id.clone(),
            run_id: plan.run_id.clone(),
            plan_digest: plan.plan_digest.clone(),
            finding_ids,
            affected_step_ids,
            lineage_digest: String::new(),
        };
        lineage.validate_without_digest()?;
        lineage.lineage_digest = lineage.expected_digest()?;
        Ok(lineage)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("lineageDigest", &self.lineage_digest)?;
        if self.lineage_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("lineageDigest"));
        }
        Ok(())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let lineage: Self = super::decode_json_slice(bytes)?;
        lineage.validate()?;
        Ok(lineage)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_RERUN_LINEAGE_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("lineageId", &self.lineage_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_digest_field("planDigest", &self.plan_digest)?;
        if self.finding_ids.is_empty() || self.finding_ids.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("findingIds"));
        }
        for pair in self.finding_ids.windows(2) {
            if pair[0] >= pair[1] {
                return Err(ResearchContractError::InvalidField("findingIds"));
            }
        }
        for finding_id in &self.finding_ids {
            validate_id("findingId", finding_id)?;
        }
        if self.affected_step_ids.len() > RESEARCH_MAX_WORKFLOW_STEPS {
            return Err(ResearchContractError::InvalidField("affectedStepIds"));
        }
        for step_id in &self.affected_step_ids {
            validate_id("affectedStepIds", step_id)?;
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            lineage_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            plan_digest: &'a str,
            finding_ids: &'a [String],
            affected_step_ids: &'a [String],
        }
        digest(
            RESEARCH_RERUN_LINEAGE_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                lineage_id: &self.lineage_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                plan_digest: &self.plan_digest,
                finding_ids: &self.finding_ids,
                affected_step_ids: &self.affected_step_ids,
            },
        )
    }
}

fn validate_sorted_unique_digests(
    field: &'static str,
    digests: &[String],
) -> Result<(), ResearchContractError> {
    for pair in digests.windows(2) {
        if pair[0] >= pair[1] {
            return Err(ResearchContractError::InvalidField(field));
        }
    }
    for value in digests {
        validate_digest_field(field, value)?;
    }
    Ok(())
}

fn detect_dependency_cycles(steps: &[ResearchWorkflowStepV1]) -> Result<(), ResearchContractError> {
    let by_id: BTreeMap<&str, &ResearchWorkflowStepV1> = steps
        .iter()
        .map(|step| (step.step_id.as_str(), step))
        .collect();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for step in steps {
        visit_cycle(step.step_id.as_str(), &by_id, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn visit_cycle(
    step_id: &str,
    by_id: &BTreeMap<&str, &ResearchWorkflowStepV1>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) -> Result<(), ResearchContractError> {
    if visited.contains(step_id) {
        return Ok(());
    }
    if !visiting.insert(step_id.to_owned()) {
        return Err(ResearchContractError::InvalidField("dependsOn"));
    }
    let Some(step) = by_id.get(step_id) else {
        return Err(ResearchContractError::InvalidField("dependsOn"));
    };
    for dependency in &step.depends_on {
        visit_cycle(dependency.as_str(), by_id, visiting, visited)?;
    }
    visiting.remove(step_id);
    visited.insert(step_id.to_owned());
    Ok(())
}

fn close_dependents(
    steps: &[ResearchWorkflowStepV1],
    mut affected: BTreeSet<String>,
) -> Result<Vec<String>, ResearchContractError> {
    let mut changed = true;
    while changed {
        changed = false;
        for step in steps {
            if affected.contains(&step.step_id) {
                continue;
            }
            if step
                .depends_on
                .iter()
                .any(|dependency| affected.contains(dependency))
            {
                affected.insert(step.step_id.clone());
                changed = true;
            }
        }
    }
    // Dependency-first order: emit a step only after every dependency that is
    // also affected has already been emitted.
    let mut remaining = affected;
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|step_id| {
                steps
                    .iter()
                    .find(|step| &step.step_id == *step_id)
                    .map(|step| {
                        step.depends_on
                            .iter()
                            .all(|dependency| !remaining.contains(dependency))
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if ready.is_empty() {
            return Err(ResearchContractError::InvalidField("dependsOn"));
        }
        for step_id in ready {
            remaining.remove(&step_id);
            ordered.push(step_id);
        }
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{
        CapabilityCeiling, CapabilityContribution, CapabilityDescriptor,
        CapabilityExecutionCeiling, CapabilityKind, CapabilitySet, CapabilitySource,
        CodeCatalogGeneration, GovernanceCapabilityCeiling, RunCapabilityBindingV1, Sha256Digest,
        WorkspaceCapabilityCeiling,
    };
    use crate::execution_identity::{ExecutionIdentityV1, ExecutionResultOutcomeV1};
    use crate::research::{
        ResearchReproducibilityV1, ResearchReviewCategoryV1, ResearchReviewFindingV1,
        ResearchReviewSeverityV1, ResearchRunStatusV1, ResearchRunV1,
    };

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    fn binding() -> RunCapabilityBindingV1 {
        let source =
            CapabilitySource::builtin("test", Sha256Digest::new(digest('c')).unwrap()).unwrap();
        let descriptor = CapabilityDescriptor::new(
            &source,
            CapabilityKind::Tool,
            "tool",
            "tool",
            Sha256Digest::new(digest('d')).unwrap(),
            [],
        )
        .unwrap();
        let contribution = CapabilityContribution::new(source, [descriptor]).unwrap();
        let set = CapabilitySet::from_contributions(CodeCatalogGeneration::new(1), [contribution])
            .unwrap();
        let ceiling = CapabilityCeiling::all(
            &set,
            WorkspaceCapabilityCeiling::default(),
            GovernanceCapabilityCeiling::default(),
            CapabilityExecutionCeiling::new(1, 1, None, None, None).unwrap(),
        )
        .unwrap();
        RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap()
    }

    fn admitted_run() -> ResearchRunV1 {
        let mut run = ResearchRunV1::new(
            "run-1",
            "project-1",
            1,
            digest('1'),
            digest('2'),
            binding(),
            "provider-1",
            "model-1",
            ResearchReproducibilityV1::Deterministic,
            Some(7),
        )
        .unwrap();
        run.transition_to(ResearchRunStatusV1::Admitted).unwrap();
        run
    }

    fn receipt(identity_digest: &str, evidence: &str, result: &str) -> ExecutionResultReceiptV1 {
        let identity = ExecutionIdentityV1 {
            schema: crate::execution_identity::EXECUTION_IDENTITY_SCHEMA_V1.to_owned(),
            domain: crate::execution_identity::WORKFLOW_STEP_IDENTITY_DOMAIN_V1.to_owned(),
            digest: identity_digest.to_owned(),
        };
        ExecutionResultReceiptV1::new(
            identity,
            evidence.to_owned(),
            ExecutionResultOutcomeV1::Succeeded,
            Some(result.to_owned()),
            16,
        )
        .unwrap()
    }

    #[test]
    fn plan_binds_receipts_and_selects_transitive_rerun_steps() {
        let run = admitted_run();
        let analyze = ResearchWorkflowStepV1::new(
            "analyze",
            "project-1",
            "run-1",
            digest('7'),
            digest('a'),
            vec![digest('2')],
            vec![digest('3')],
            Vec::new(),
        )
        .unwrap()
        .bind_result_receipt(&receipt(&digest('a'), &digest('2'), &digest('9')))
        .unwrap();
        let report = ResearchWorkflowStepV1::new(
            "report",
            "project-1",
            "run-1",
            digest('7'),
            digest('b'),
            vec![digest('3')],
            vec![digest('4')],
            vec!["analyze".to_owned()],
        )
        .unwrap();
        let plan = ResearchWorkflowPlanV1::new_for_run(
            "plan-1",
            &run,
            digest('7'),
            vec![report.clone(), analyze],
        )
        .unwrap();
        assert_eq!(plan.random_seed, Some(7));
        let finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('3'),
            ResearchReviewCategoryV1::Numeric,
            ResearchReviewSeverityV1::Error,
            "numeric mismatch",
            None,
            vec![digest('2')],
            "reviewer",
            1,
        )
        .unwrap();
        assert_eq!(
            plan.affected_step_ids_for_findings(&[finding.clone()])
                .unwrap(),
            vec!["analyze".to_owned(), "report".to_owned()]
        );
        let encoded = plan.to_vec().unwrap();
        assert_eq!(ResearchWorkflowPlanV1::from_slice(&encoded).unwrap(), plan);
        let lineage =
            ResearchRerunLineageV1::from_findings_for_plan("lineage-1", &plan, &[finding]).unwrap();
        assert_eq!(
            lineage.affected_step_ids,
            vec!["analyze".to_owned(), "report".to_owned()]
        );
        assert_eq!(lineage.plan_digest, plan.plan_digest);
        let lineage_bytes = lineage.to_vec().unwrap();
        assert_eq!(
            ResearchRerunLineageV1::from_slice(&lineage_bytes).unwrap(),
            lineage
        );
    }

    #[test]
    fn receipt_and_seed_drift_fail_closed() {
        let run = admitted_run();
        let step = ResearchWorkflowStepV1::new(
            "analyze",
            "project-1",
            "run-1",
            digest('7'),
            digest('a'),
            vec![digest('2')],
            vec![digest('3')],
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            step.clone()
                .bind_result_receipt(&receipt(&digest('b'), &digest('2'), &digest('9'))),
            Err(ResearchContractError::InvalidField("stepIdentityDigest"))
        ));
        assert!(matches!(
            step.bind_result_receipt(&receipt(&digest('a'), &digest('5'), &digest('9'))),
            Err(ResearchContractError::InvalidField("evidenceDigest"))
        ));
        let plan = ResearchWorkflowPlanV1::new(
            "plan-1",
            "project-1",
            "run-1",
            digest('7'),
            Some(99),
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            plan.validate_for_run(&run),
            Err(ResearchContractError::InvalidField("randomSeed"))
        ));
    }
}
