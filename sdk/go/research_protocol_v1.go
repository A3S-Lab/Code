// Code generated from core/src/research/protocol.rs; DO NOT EDIT.
//
// Run: node scripts/generate_research_protocol_artifacts.mjs

package code

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
)

const ResearchProtocolVersionV1 = 1
const ResearchProtocolSchemaV1 = "a3s.code.research-wire.v1"
const ResearchProtocolMaxMessageBytes = 33554432

// ResearchWireKindV1 is the closed top-level payload catalog accepted by
// Core. Payload bytes remain opaque until a host chooses a typed adapter.
type ResearchWireKindV1 string

const (
	ResearchWireResearchRun                     ResearchWireKindV1 = "research_run"
	ResearchWireResearchEvidenceFact            ResearchWireKindV1 = "research_evidence_fact"
	ResearchWireResearchClaim                   ResearchWireKindV1 = "research_claim"
	ResearchWireResearchCitation                ResearchWireKindV1 = "research_citation"
	ResearchWireResearchEvidenceGraph           ResearchWireKindV1 = "research_evidence_graph"
	ResearchWireResearchWorkflowStep            ResearchWireKindV1 = "research_workflow_step"
	ResearchWireResearchWorkflowPlan            ResearchWireKindV1 = "research_workflow_plan"
	ResearchWireResearchRerunLineage            ResearchWireKindV1 = "research_rerun_lineage"
	ResearchWireResearchProvenanceReceipt       ResearchWireKindV1 = "research_provenance_receipt"
	ResearchWireResearchReproducibilityManifest ResearchWireKindV1 = "research_reproducibility_manifest"
	ResearchWireResearchReviewFinding           ResearchWireKindV1 = "research_review_finding"
	ResearchWireResearchReviewBatch             ResearchWireKindV1 = "research_review_batch"
	ResearchWireResearchEvent                   ResearchWireKindV1 = "research_event"
)

var researchWireKindsV1 = [...]ResearchWireKindV1{
	ResearchWireResearchRun,
	ResearchWireResearchEvidenceFact,
	ResearchWireResearchClaim,
	ResearchWireResearchCitation,
	ResearchWireResearchEvidenceGraph,
	ResearchWireResearchWorkflowStep,
	ResearchWireResearchWorkflowPlan,
	ResearchWireResearchRerunLineage,
	ResearchWireResearchProvenanceReceipt,
	ResearchWireResearchReproducibilityManifest,
	ResearchWireResearchReviewFinding,
	ResearchWireResearchReviewBatch,
	ResearchWireResearchEvent,
}

// ResearchWireKindsV1 returns the ordered version-1 catalog.
func ResearchWireKindsV1() []ResearchWireKindV1 {
	return append([]ResearchWireKindV1(nil), researchWireKindsV1[:]...)
}

// ResearchWireEnvelopeV1 is the strict JSON transport shape shared by Core
// and the SDKs. Core validates payload fields before admission.
type ResearchWireEnvelopeV1 struct {
	Schema  string             `json:"schema"`
	Version uint16             `json:"version"`
	Kind    ResearchWireKindV1 `json:"kind"`
	Payload json.RawMessage    `json:"payload"`
}

// Validate checks the envelope identity and the closed kind catalog. Core
// remains responsible for validating the concrete payload fields.
func (envelope ResearchWireEnvelopeV1) Validate() error {
	if envelope.Schema != ResearchProtocolSchemaV1 {
		return fmt.Errorf("unsupported research wire schema %q", envelope.Schema)
	}
	if envelope.Version != ResearchProtocolVersionV1 {
		return fmt.Errorf("unsupported research wire version %d", envelope.Version)
	}
	trimmed := bytes.TrimSpace(envelope.Payload)
	if len(trimmed) == 0 || bytes.Equal(trimmed, []byte("null")) {
		return fmt.Errorf("research wire payload is required")
	}
	for _, known := range researchWireKindsV1 {
		if envelope.Kind == known {
			return nil
		}
	}
	return fmt.Errorf("unknown research wire kind %q", envelope.Kind)
}

// DecodeResearchWireEnvelopeV1 rejects unknown top-level fields, unsupported
// versions, unknown kinds, trailing JSON, and oversized messages.
func DecodeResearchWireEnvelopeV1(data []byte) (ResearchWireEnvelopeV1, error) {
	var envelope ResearchWireEnvelopeV1
	if len(data) > ResearchProtocolMaxMessageBytes {
		return envelope, fmt.Errorf("research wire message exceeds %d bytes", ResearchProtocolMaxMessageBytes)
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&envelope); err != nil {
		return envelope, err
	}
	var trailing any
	if err := decoder.Decode(&trailing); err != io.EOF {
		if err == nil {
			return envelope, fmt.Errorf("research wire message has trailing JSON")
		}
		return envelope, err
	}
	if err := envelope.Validate(); err != nil {
		return envelope, err
	}
	return envelope, nil
}

// ResearchRunV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchRunPayloadV1 = json.RawMessage

// ResearchEvidenceFactV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchEvidenceFactPayloadV1 = json.RawMessage

// ResearchClaimV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchClaimPayloadV1 = json.RawMessage

// ResearchCitationV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchCitationPayloadV1 = json.RawMessage

// ResearchEvidenceGraphV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchEvidenceGraphPayloadV1 = json.RawMessage

// ResearchWorkflowStepV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchWorkflowStepPayloadV1 = json.RawMessage

// ResearchWorkflowPlanV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchWorkflowPlanPayloadV1 = json.RawMessage

// ResearchRerunLineageV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchRerunLineagePayloadV1 = json.RawMessage

// ResearchProvenanceReceiptV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchProvenanceReceiptPayloadV1 = json.RawMessage

// ResearchReproducibilityManifestV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchReproducibilityManifestPayloadV1 = json.RawMessage

// ResearchReviewFindingV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchReviewFindingPayloadV1 = json.RawMessage

// ResearchReviewBatchV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchReviewBatchPayloadV1 = json.RawMessage

// ResearchEventV1 is preserved as JSON so hosts can apply their own typed adapter.
type ResearchEventPayloadV1 = json.RawMessage
