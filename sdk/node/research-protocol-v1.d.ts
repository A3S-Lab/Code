/**
 * Generated from core/src/research/protocol.rs.
 * Run `node scripts/generate_research_protocol_artifacts.mjs` to update.
 *
 * Payloads remain opaque JSON objects at the SDK boundary. Rust Core owns the
 * closed payload schemas and validation; hosts own business/reviewer meaning.
 */

export const RESEARCH_PROTOCOL_VERSION_V1 = 1 as const
export const RESEARCH_PROTOCOL_SCHEMA_V1 = 'a3s.code.research-wire.v1' as const
export const RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES = 33554432 as const

/** Closed top-level kinds accepted by research wire version 1. */
export type KnownResearchWireKindV1 =
  | 'research_run'
  | 'research_evidence_fact'
  | 'research_claim'
  | 'research_citation'
  | 'research_evidence_graph'
  | 'research_workflow_step'
  | 'research_workflow_plan'
  | 'research_rerun_lineage'
  | 'research_provenance_receipt'
  | 'research_reproducibility_manifest'
  | 'research_review_finding'
  | 'research_review_batch'
  | 'research_event'

export type ResearchWireKindV1 = KnownResearchWireKindV1

export const ResearchWireTypeV1 = {
  RESEARCH_RUN: 'research_run',
  RESEARCH_EVIDENCE_FACT: 'research_evidence_fact',
  RESEARCH_CLAIM: 'research_claim',
  RESEARCH_CITATION: 'research_citation',
  RESEARCH_EVIDENCE_GRAPH: 'research_evidence_graph',
  RESEARCH_WORKFLOW_STEP: 'research_workflow_step',
  RESEARCH_WORKFLOW_PLAN: 'research_workflow_plan',
  RESEARCH_RERUN_LINEAGE: 'research_rerun_lineage',
  RESEARCH_PROVENANCE_RECEIPT: 'research_provenance_receipt',
  RESEARCH_REPRODUCIBILITY_MANIFEST: 'research_reproducibility_manifest',
  RESEARCH_REVIEW_FINDING: 'research_review_finding',
  RESEARCH_REVIEW_BATCH: 'research_review_batch',
  RESEARCH_EVENT: 'research_event',
} as const

export const RESEARCH_WIRE_KINDS_V1 = [
  'research_run',
  'research_evidence_fact',
  'research_claim',
  'research_citation',
  'research_evidence_graph',
  'research_workflow_step',
  'research_workflow_plan',
  'research_rerun_lineage',
  'research_provenance_receipt',
  'research_reproducibility_manifest',
  'research_review_finding',
  'research_review_batch',
  'research_event',
] as const satisfies readonly KnownResearchWireKindV1[]

/** Strict envelope shared by Core and all SDKs. */
export interface ResearchWireEnvelopeV1<TPayload = ResearchWirePayloadV1> {
  readonly schema: typeof RESEARCH_PROTOCOL_SCHEMA_V1
  readonly version: typeof RESEARCH_PROTOCOL_VERSION_V1
  readonly kind: ResearchWireKindV1
  readonly payload: TPayload
}

export type ResearchRunPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchEvidenceFactPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchClaimPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchCitationPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchEvidenceGraphPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchWorkflowStepPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchWorkflowPlanPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchRerunLineagePayloadV1 = Readonly<Record<string, unknown>>
export type ResearchProvenanceReceiptPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchReproducibilityManifestPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchReviewFindingPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchReviewBatchPayloadV1 = Readonly<Record<string, unknown>>
export type ResearchEventPayloadV1 = Readonly<Record<string, unknown>>

/** Opaque JSON payload preserved for host-owned transport adapters. */
export type ResearchWirePayloadV1 = Readonly<Record<string, unknown>>

/** Union of all payload shapes known to Core at wire version 1. */
export type KnownResearchWirePayloadV1 =
  | ResearchRunPayloadV1
  | ResearchEvidenceFactPayloadV1
  | ResearchClaimPayloadV1
  | ResearchCitationPayloadV1
  | ResearchEvidenceGraphPayloadV1
  | ResearchWorkflowStepPayloadV1
  | ResearchWorkflowPlanPayloadV1
  | ResearchRerunLineagePayloadV1
  | ResearchProvenanceReceiptPayloadV1
  | ResearchReproducibilityManifestPayloadV1
  | ResearchReviewFindingPayloadV1
  | ResearchReviewBatchPayloadV1
  | ResearchEventPayloadV1

/** Discriminated message union for exhaustive SDK dispatch. */
export type ResearchWireMessageV1 =
  | (ResearchWireEnvelopeV1<ResearchRunPayloadV1> & { readonly kind: 'research_run' })
  | (ResearchWireEnvelopeV1<ResearchEvidenceFactPayloadV1> & { readonly kind: 'research_evidence_fact' })
  | (ResearchWireEnvelopeV1<ResearchClaimPayloadV1> & { readonly kind: 'research_claim' })
  | (ResearchWireEnvelopeV1<ResearchCitationPayloadV1> & { readonly kind: 'research_citation' })
  | (ResearchWireEnvelopeV1<ResearchEvidenceGraphPayloadV1> & { readonly kind: 'research_evidence_graph' })
  | (ResearchWireEnvelopeV1<ResearchWorkflowStepPayloadV1> & { readonly kind: 'research_workflow_step' })
  | (ResearchWireEnvelopeV1<ResearchWorkflowPlanPayloadV1> & { readonly kind: 'research_workflow_plan' })
  | (ResearchWireEnvelopeV1<ResearchRerunLineagePayloadV1> & { readonly kind: 'research_rerun_lineage' })
  | (ResearchWireEnvelopeV1<ResearchProvenanceReceiptPayloadV1> & { readonly kind: 'research_provenance_receipt' })
  | (ResearchWireEnvelopeV1<ResearchReproducibilityManifestPayloadV1> & { readonly kind: 'research_reproducibility_manifest' })
  | (ResearchWireEnvelopeV1<ResearchReviewFindingPayloadV1> & { readonly kind: 'research_review_finding' })
  | (ResearchWireEnvelopeV1<ResearchReviewBatchPayloadV1> & { readonly kind: 'research_review_batch' })
  | (ResearchWireEnvelopeV1<ResearchEventPayloadV1> & { readonly kind: 'research_event' })
