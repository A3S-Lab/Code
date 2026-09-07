"""Generated research wire protocol declarations.

Generated from core/src/research/protocol.rs. Run
node scripts/generate_research_protocol_artifacts.mjs to update.

Payload values intentionally remain mappings: Core is the single authority for
closed payload validation, while hosts own transport and business semantics.
"""

from typing import Final, Literal, Mapping, Tuple, TypedDict

RESEARCH_PROTOCOL_VERSION_V1: Final[int] = 1
RESEARCH_PROTOCOL_SCHEMA_V1: Final[str] = "a3s.code.research-wire.v1"
RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES: Final[int] = 33554432

KnownResearchWireKindV1 = Literal[
    "research_run",
    "research_evidence_fact",
    "research_claim",
    "research_citation",
    "research_evidence_graph",
    "research_workflow_step",
    "research_workflow_plan",
    "research_rerun_lineage",
    "research_provenance_receipt",
    "research_reproducibility_manifest",
    "research_review_finding",
    "research_review_batch",
    "research_event",
]
ResearchWireKindV1 = KnownResearchWireKindV1

RESEARCH_WIRE_KINDS_V1: Final[Tuple[KnownResearchWireKindV1, ...]] = (
    "research_run",
    "research_evidence_fact",
    "research_claim",
    "research_citation",
    "research_evidence_graph",
    "research_workflow_step",
    "research_workflow_plan",
    "research_rerun_lineage",
    "research_provenance_receipt",
    "research_reproducibility_manifest",
    "research_review_finding",
    "research_review_batch",
    "research_event",
)


class ResearchWireTypeV1:
    """Canonical string constants for research wire version 1."""

    RESEARCH_RUN: Final[str] = "research_run"
    RESEARCH_EVIDENCE_FACT: Final[str] = "research_evidence_fact"
    RESEARCH_CLAIM: Final[str] = "research_claim"
    RESEARCH_CITATION: Final[str] = "research_citation"
    RESEARCH_EVIDENCE_GRAPH: Final[str] = "research_evidence_graph"
    RESEARCH_WORKFLOW_STEP: Final[str] = "research_workflow_step"
    RESEARCH_WORKFLOW_PLAN: Final[str] = "research_workflow_plan"
    RESEARCH_RERUN_LINEAGE: Final[str] = "research_rerun_lineage"
    RESEARCH_PROVENANCE_RECEIPT: Final[str] = "research_provenance_receipt"
    RESEARCH_REPRODUCIBILITY_MANIFEST: Final[str] = "research_reproducibility_manifest"
    RESEARCH_REVIEW_FINDING: Final[str] = "research_review_finding"
    RESEARCH_REVIEW_BATCH: Final[str] = "research_review_batch"
    RESEARCH_EVENT: Final[str] = "research_event"


ResearchWirePayloadV1 = Mapping[str, object]
ResearchRunPayloadV1 = ResearchWirePayloadV1
ResearchEvidenceFactPayloadV1 = ResearchWirePayloadV1
ResearchClaimPayloadV1 = ResearchWirePayloadV1
ResearchCitationPayloadV1 = ResearchWirePayloadV1
ResearchEvidenceGraphPayloadV1 = ResearchWirePayloadV1
ResearchWorkflowStepPayloadV1 = ResearchWirePayloadV1
ResearchWorkflowPlanPayloadV1 = ResearchWirePayloadV1
ResearchRerunLineagePayloadV1 = ResearchWirePayloadV1
ResearchProvenanceReceiptPayloadV1 = ResearchWirePayloadV1
ResearchReproducibilityManifestPayloadV1 = ResearchWirePayloadV1
ResearchReviewFindingPayloadV1 = ResearchWirePayloadV1
ResearchReviewBatchPayloadV1 = ResearchWirePayloadV1
ResearchEventPayloadV1 = ResearchWirePayloadV1


class ResearchWireEnvelopeV1(TypedDict):
    """Strict top-level envelope shape emitted by Code Core."""

    schema: str
    version: int
    kind: KnownResearchWireKindV1
    payload: ResearchWirePayloadV1


__all__ = [
    "RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES",
    "RESEARCH_PROTOCOL_SCHEMA_V1",
    "RESEARCH_PROTOCOL_VERSION_V1",
    "RESEARCH_WIRE_KINDS_V1",
    "ResearchWireEnvelopeV1",
    "ResearchWireKindV1",
    "ResearchWirePayloadV1",
    "ResearchWireTypeV1",
    "KnownResearchWireKindV1",
    "ResearchRunPayloadV1",
    "ResearchEvidenceFactPayloadV1",
    "ResearchClaimPayloadV1",
    "ResearchCitationPayloadV1",
    "ResearchEvidenceGraphPayloadV1",
    "ResearchWorkflowStepPayloadV1",
    "ResearchWorkflowPlanPayloadV1",
    "ResearchRerunLineagePayloadV1",
    "ResearchProvenanceReceiptPayloadV1",
    "ResearchReproducibilityManifestPayloadV1",
    "ResearchReviewFindingPayloadV1",
    "ResearchReviewBatchPayloadV1",
    "ResearchEventPayloadV1",
]
