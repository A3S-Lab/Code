//! Bounded evidence-graph projection with publication completeness checks.

use super::{
    digest, validate_digest_field, validate_id, ResearchCitationV1, ResearchClaimStatusV1,
    ResearchClaimV1, ResearchContractError,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const RESEARCH_EVIDENCE_GRAPH_SCHEMA_V1: &str = "a3s.code.evidence-graph.v1";
pub const RESEARCH_MAX_EVIDENCE_GRAPH_CLAIMS: usize = 512;
pub const RESEARCH_MAX_EVIDENCE_GRAPH_CITATIONS: usize = 2048;
const RESEARCH_EVIDENCE_GRAPH_DIGEST_DOMAIN: &str = "a3s.code.evidence-graph.identity.v1";

/// One immutable projection of claims and citations for a research Run.
///
/// The graph does not decide scientific truth. It only fences mixed project or
/// Run identities and measures whether every claim carries explicit support,
/// conflict, or gap evidence before a host may treat the set as publishable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchEvidenceGraphV1 {
    pub schema: String,
    pub graph_id: String,
    pub project_id: String,
    pub run_id: String,
    pub claims: Vec<ResearchClaimV1>,
    pub citations: Vec<ResearchCitationV1>,
    pub graph_digest: String,
}

/// Digest-only completeness counters for one evidence graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchEvidenceCompletenessV1 {
    pub claim_count: u32,
    pub citation_count: u32,
    pub supported_count: u32,
    pub conflicted_count: u32,
    pub unsupported_count: u32,
    pub proposed_count: u32,
    pub publication_ready: bool,
}

impl ResearchEvidenceGraphV1 {
    /// Construct and validate a graph against the admitted research Run.
    pub fn new_for_run(
        graph_id: impl Into<String>,
        run: &crate::research::ResearchRunV1,
        claims: Vec<ResearchClaimV1>,
        citations: Vec<ResearchCitationV1>,
    ) -> Result<Self, ResearchContractError> {
        let graph = Self::new(
            graph_id,
            run.project_id.clone(),
            run.run_id.clone(),
            claims,
            citations,
        )?;
        graph.validate_for_run(run)?;
        Ok(graph)
    }

    pub fn new(
        graph_id: impl Into<String>,
        project_id: impl Into<String>,
        run_id: impl Into<String>,
        mut claims: Vec<ResearchClaimV1>,
        mut citations: Vec<ResearchCitationV1>,
    ) -> Result<Self, ResearchContractError> {
        claims.sort_unstable_by(|left, right| left.claim_id.cmp(&right.claim_id));
        citations.sort_unstable_by(|left, right| left.citation_id.cmp(&right.citation_id));
        let mut graph = Self {
            schema: RESEARCH_EVIDENCE_GRAPH_SCHEMA_V1.to_owned(),
            graph_id: graph_id.into(),
            project_id: project_id.into(),
            run_id: run_id.into(),
            claims,
            citations,
            graph_digest: String::new(),
        };
        graph.validate_without_digest()?;
        graph.graph_digest = graph.expected_digest()?;
        Ok(graph)
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
        for claim in &self.claims {
            claim.validate_for_run(run)?;
        }
        for citation in &self.citations {
            citation.validate_for_run(run)?;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("graphDigest", &self.graph_digest)?;
        if self.graph_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("graphDigest"));
        }
        Ok(())
    }

    /// Measure whether every claim has explicit support, conflict, or gap
    /// evidence and whether citations close those support links.
    pub fn completeness(&self) -> Result<ResearchEvidenceCompletenessV1, ResearchContractError> {
        self.validate()?;
        let mut supported_count = 0u32;
        let mut conflicted_count = 0u32;
        let mut unsupported_count = 0u32;
        let mut proposed_count = 0u32;
        for claim in &self.claims {
            match claim.status {
                ResearchClaimStatusV1::Proposed => proposed_count += 1,
                ResearchClaimStatusV1::Supported => supported_count += 1,
                ResearchClaimStatusV1::Conflicted => conflicted_count += 1,
                ResearchClaimStatusV1::Unsupported => unsupported_count += 1,
            }
        }
        let publication_ready = proposed_count == 0
            && self
                .claims
                .iter()
                .all(|claim| self.claim_is_publication_complete(claim).is_ok());
        Ok(ResearchEvidenceCompletenessV1 {
            claim_count: u32::try_from(self.claims.len())
                .map_err(|_| ResearchContractError::InvalidField("claims"))?,
            citation_count: u32::try_from(self.citations.len())
                .map_err(|_| ResearchContractError::InvalidField("citations"))?,
            supported_count,
            conflicted_count,
            unsupported_count,
            proposed_count,
            publication_ready,
        })
    }

    /// Fail closed unless every claim is publication-ready and support links
    /// resolve to citations retained in this graph.
    pub fn validate_publication_completeness(&self) -> Result<(), ResearchContractError> {
        let completeness = self.completeness()?;
        if !completeness.publication_ready {
            return Err(ResearchContractError::InvalidField("publicationReady"));
        }
        Ok(())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let graph: Self = super::decode_json_slice(bytes)?;
        graph.validate()?;
        Ok(graph)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    fn claim_is_publication_complete(
        &self,
        claim: &ResearchClaimV1,
    ) -> Result<(), ResearchContractError> {
        if !claim.status.is_publication_ready() {
            return Err(ResearchContractError::InvalidField("claim.status"));
        }
        if matches!(claim.status, ResearchClaimStatusV1::Supported) {
            let citation_digests: BTreeSet<&str> = self
                .citations
                .iter()
                .filter(|citation| citation.claim_id == claim.claim_id)
                .map(|citation| citation.citation_digest.as_str())
                .collect();
            if citation_digests.is_empty() {
                return Err(ResearchContractError::InvalidField("citations"));
            }
            for support in &claim.support_digests {
                if !citation_digests.contains(support.as_str()) {
                    return Err(ResearchContractError::InvalidField("supportDigests"));
                }
            }
        }
        Ok(())
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_EVIDENCE_GRAPH_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("graphId", &self.graph_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        if self.claims.len() > RESEARCH_MAX_EVIDENCE_GRAPH_CLAIMS {
            return Err(ResearchContractError::InvalidField("claims"));
        }
        if self.citations.len() > RESEARCH_MAX_EVIDENCE_GRAPH_CITATIONS {
            return Err(ResearchContractError::InvalidField("citations"));
        }
        for pair in self.claims.windows(2) {
            if pair[0].claim_id >= pair[1].claim_id {
                return Err(ResearchContractError::InvalidField("claims"));
            }
        }
        for pair in self.citations.windows(2) {
            if pair[0].citation_id >= pair[1].citation_id {
                return Err(ResearchContractError::InvalidField("citations"));
            }
        }
        let mut claim_ids = BTreeMap::new();
        for claim in &self.claims {
            claim.validate()?;
            if claim.project_id != self.project_id {
                return Err(ResearchContractError::InvalidField("claim.projectId"));
            }
            if claim.run_id != self.run_id {
                return Err(ResearchContractError::InvalidField("claim.runId"));
            }
            if claim_ids.insert(claim.claim_id.as_str(), ()).is_some() {
                return Err(ResearchContractError::InvalidField("claimId"));
            }
        }
        let mut citation_ids = BTreeSet::new();
        for citation in &self.citations {
            citation.validate()?;
            if citation.project_id != self.project_id {
                return Err(ResearchContractError::InvalidField("citation.projectId"));
            }
            if citation.run_id != self.run_id {
                return Err(ResearchContractError::InvalidField("citation.runId"));
            }
            if !claim_ids.contains_key(citation.claim_id.as_str()) {
                return Err(ResearchContractError::InvalidField("citation.claimId"));
            }
            if !citation_ids.insert(citation.citation_id.as_str()) {
                return Err(ResearchContractError::InvalidField("citationId"));
            }
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            graph_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            claim_digests: Vec<&'a str>,
            citation_digests: Vec<&'a str>,
        }
        digest(
            RESEARCH_EVIDENCE_GRAPH_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                graph_id: &self.graph_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                claim_digests: self
                    .claims
                    .iter()
                    .map(|claim| claim.claim_digest.as_str())
                    .collect(),
                citation_digests: self
                    .citations
                    .iter()
                    .map(|citation| citation.citation_digest.as_str())
                    .collect(),
            },
        )
    }
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
    use crate::research::{ResearchReproducibilityV1, ResearchRunStatusV1, ResearchRunV1};

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
            ResearchReproducibilityV1::Reproducible,
            Some(7),
        )
        .unwrap();
        run.transition_to(ResearchRunStatusV1::Admitted).unwrap();
        run
    }

    #[test]
    fn publication_completeness_requires_support_or_explicit_gap() {
        let run = admitted_run();
        let citation = ResearchCitationV1::new(
            "cite-1",
            "project-1",
            "run-1",
            "claim-1",
            digest('3'),
            digest('4'),
            Some("p.1".to_owned()),
            1,
        )
        .unwrap();
        let supported = ResearchClaimV1::new("claim-1", "project-1", "run-1", digest('a'), 1)
            .unwrap()
            .mark_supported(vec![citation.citation_digest.clone()])
            .unwrap();
        let gap = ResearchClaimV1::new("claim-2", "project-1", "run-1", digest('b'), 2)
            .unwrap()
            .mark_unsupported(digest('5'))
            .unwrap();
        let graph = ResearchEvidenceGraphV1::new_for_run(
            "graph-1",
            &run,
            vec![supported, gap],
            vec![citation],
        )
        .unwrap();
        let completeness = graph.completeness().unwrap();
        assert!(completeness.publication_ready);
        assert_eq!(completeness.supported_count, 1);
        assert_eq!(completeness.unsupported_count, 1);
        assert_eq!(completeness.proposed_count, 0);
        graph.validate_publication_completeness().unwrap();
        let encoded = graph.to_vec().unwrap();
        assert_eq!(
            ResearchEvidenceGraphV1::from_slice(&encoded).unwrap(),
            graph
        );
    }

    #[test]
    fn proposed_or_unlinked_support_blocks_publication() {
        let run = admitted_run();
        let proposed =
            ResearchClaimV1::new("claim-1", "project-1", "run-1", digest('a'), 1).unwrap();
        let graph =
            ResearchEvidenceGraphV1::new_for_run("graph-1", &run, vec![proposed], Vec::new())
                .unwrap();
        assert!(!graph.completeness().unwrap().publication_ready);
        assert!(matches!(
            graph.validate_publication_completeness(),
            Err(ResearchContractError::InvalidField("publicationReady"))
        ));

        let citation = ResearchCitationV1::new(
            "cite-1",
            "project-1",
            "run-1",
            "claim-2",
            digest('3'),
            digest('4'),
            None,
            1,
        )
        .unwrap();
        let supported = ResearchClaimV1::new("claim-2", "project-1", "run-1", digest('b'), 1)
            .unwrap()
            .mark_supported(vec![digest('6')])
            .unwrap();
        let graph =
            ResearchEvidenceGraphV1::new_for_run("graph-2", &run, vec![supported], vec![citation])
                .unwrap();
        assert!(!graph.completeness().unwrap().publication_ready);
    }

    #[test]
    fn mixed_run_or_orphan_citation_fail_closed() {
        let run = admitted_run();
        let claim = ResearchClaimV1::new("claim-1", "project-1", "run-1", digest('a'), 1).unwrap();
        let orphan = ResearchCitationV1::new(
            "cite-1",
            "project-1",
            "run-1",
            "missing-claim",
            digest('3'),
            digest('4'),
            None,
            1,
        )
        .unwrap();
        assert!(matches!(
            ResearchEvidenceGraphV1::new_for_run("graph-1", &run, vec![claim], vec![orphan]),
            Err(ResearchContractError::InvalidField("citation.claimId"))
        ));
    }
}
