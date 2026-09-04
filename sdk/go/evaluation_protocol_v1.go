// Code generated from core/src/evaluation/protocol.rs; DO NOT EDIT.
//
// Run: node scripts/generate_evaluation_protocol_artifacts.mjs

package code

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
)

const EvaluationProtocolVersionV1 = 1
const EvaluationProtocolSchemaV1 = "a3s.code.evaluation-wire.v1"
const EvaluationProtocolMaxMessageBytes = 33554432

// EvaluationWireKindV1 is the closed top-level payload catalog accepted by
// Core. Payload bytes remain opaque until a host chooses a typed adapter.
type EvaluationWireKindV1 string

const (
	EvaluationWireEvidenceReadRequest  EvaluationWireKindV1 = "evidence_read_request"
	EvaluationWireEvidenceSnapshot     EvaluationWireKindV1 = "evidence_snapshot"
	EvaluationWireAuxiliaryRunSpec     EvaluationWireKindV1 = "auxiliary_run_spec"
	EvaluationWireAuxiliaryRunSnapshot EvaluationWireKindV1 = "auxiliary_run_snapshot"
	EvaluationWireAuxiliaryRunOutput   EvaluationWireKindV1 = "auxiliary_run_output"
	EvaluationWireEvaluationResult     EvaluationWireKindV1 = "evaluation_result"
	EvaluationWireEvaluationRecord     EvaluationWireKindV1 = "evaluation_record"
)

var evaluationWireKindsV1 = [...]EvaluationWireKindV1{
	EvaluationWireEvidenceReadRequest,
	EvaluationWireEvidenceSnapshot,
	EvaluationWireAuxiliaryRunSpec,
	EvaluationWireAuxiliaryRunSnapshot,
	EvaluationWireAuxiliaryRunOutput,
	EvaluationWireEvaluationResult,
	EvaluationWireEvaluationRecord,
}

// EvaluationWireKindsV1 returns the ordered version-1 catalog.
func EvaluationWireKindsV1() []EvaluationWireKindV1 {
	return append([]EvaluationWireKindV1(nil), evaluationWireKindsV1[:]...)
}

// EvaluationWireEnvelopeV1 is the strict JSON transport shape shared by Core
// and the SDKs. Core validates payload fields before admission.
type EvaluationWireEnvelopeV1 struct {
	Schema  string               `json:"schema"`
	Version uint16               `json:"version"`
	Kind    EvaluationWireKindV1 `json:"kind"`
	Payload json.RawMessage      `json:"payload"`
}

// Validate checks the envelope identity and the closed kind catalog. Core
// remains responsible for validating the concrete payload fields.
func (envelope EvaluationWireEnvelopeV1) Validate() error {
	if envelope.Schema != EvaluationProtocolSchemaV1 {
		return fmt.Errorf("unsupported evaluation wire schema %q", envelope.Schema)
	}
	if envelope.Version != EvaluationProtocolVersionV1 {
		return fmt.Errorf("unsupported evaluation wire version %d", envelope.Version)
	}
	trimmed := bytes.TrimSpace(envelope.Payload)
	if len(trimmed) == 0 || bytes.Equal(trimmed, []byte("null")) {
		return fmt.Errorf("evaluation wire payload is required")
	}
	for _, known := range evaluationWireKindsV1 {
		if envelope.Kind == known {
			return nil
		}
	}
	return fmt.Errorf("unknown evaluation wire kind %q", envelope.Kind)
}

// DecodeEvaluationWireEnvelopeV1 rejects unknown top-level fields, unsupported
// versions, unknown kinds, trailing JSON, and oversized messages.
func DecodeEvaluationWireEnvelopeV1(data []byte) (EvaluationWireEnvelopeV1, error) {
	var envelope EvaluationWireEnvelopeV1
	if len(data) > EvaluationProtocolMaxMessageBytes {
		return envelope, fmt.Errorf("evaluation wire message exceeds %d bytes", EvaluationProtocolMaxMessageBytes)
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&envelope); err != nil {
		return envelope, err
	}
	var trailing any
	if err := decoder.Decode(&trailing); err != io.EOF {
		if err == nil {
			return envelope, fmt.Errorf("evaluation wire message has trailing JSON")
		}
		return envelope, err
	}
	if err := envelope.Validate(); err != nil {
		return envelope, err
	}
	return envelope, nil
}

// EvidenceReadRequestV1 is preserved as JSON so hosts can apply their own typed adapter.
type EvidenceReadRequestPayloadV1 = json.RawMessage

// EvidenceSnapshotV1 is preserved as JSON so hosts can apply their own typed adapter.
type EvidenceSnapshotPayloadV1 = json.RawMessage

// AuxiliaryRunSpecV1 is preserved as JSON so hosts can apply their own typed adapter.
type AuxiliaryRunSpecPayloadV1 = json.RawMessage

// AuxiliaryRunSnapshotV1 is preserved as JSON so hosts can apply their own typed adapter.
type AuxiliaryRunSnapshotPayloadV1 = json.RawMessage

// AuxiliaryRunOutputV1 is preserved as JSON so hosts can apply their own typed adapter.
type AuxiliaryRunOutputPayloadV1 = json.RawMessage

// EvaluationResultV1 is preserved as JSON so hosts can apply their own typed adapter.
type EvaluationResultPayloadV1 = json.RawMessage

// EvaluationRecordV1 is preserved as JSON so hosts can apply their own typed adapter.
type EvaluationRecordPayloadV1 = json.RawMessage
