package code

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func evaluationFixturePath(t *testing.T) string {
	t.Helper()
	_, source, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	return filepath.Join(filepath.Dir(source), "..", "evaluation", "evaluation-wire-v1-fixtures.json")
}

func TestEvaluationWireCatalogAndFixtures(t *testing.T) {
	want := []EvaluationWireKindV1{
		EvaluationWireEvidenceReadRequest,
		EvaluationWireEvidenceSnapshot,
		EvaluationWireAuxiliaryRunSpec,
		EvaluationWireAuxiliaryRunSnapshot,
		EvaluationWireAuxiliaryRunOutput,
		EvaluationWireEvaluationResult,
		EvaluationWireEvaluationRecord,
	}
	got := EvaluationWireKindsV1()
	if len(got) != len(want) {
		t.Fatalf("catalog length = %d, want %d", len(got), len(want))
	}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("catalog[%d] = %q, want %q", index, got[index], want[index])
		}
	}

	data, err := os.ReadFile(evaluationFixturePath(t))
	if err != nil {
		t.Fatal(err)
	}
	var fixtures map[string]json.RawMessage
	if err := json.Unmarshal(data, &fixtures); err != nil {
		t.Fatal(err)
	}
	valid, err := DecodeEvaluationWireEnvelopeV1(fixtures["valid"])
	if err != nil {
		t.Fatalf("valid fixture rejected: %v", err)
	}
	if valid.Kind != EvaluationWireEvidenceReadRequest {
		t.Fatalf("valid kind = %q", valid.Kind)
	}
	for _, name := range []string{"unknown_top_level_field", "unsupported_version"} {
		if _, err := DecodeEvaluationWireEnvelopeV1(fixtures[name]); err == nil {
			t.Fatalf("negative fixture %q was accepted", name)
		}
	}
	// Payload field validation is intentionally delegated to Core's typed
	// decoder because this projection preserves payload bytes opaquely.
	if len(fixtures["unknown_payload_field"]) == 0 {
		t.Fatal("missing unknown payload fixture")
	}
}

func TestEvaluationWireEnvelopeRejectsTrailingJSON(t *testing.T) {
	valid := []byte(`{"schema":"a3s.code.evaluation-wire.v1","version":1,"kind":"evidence_read_request","payload":{"target":{"schema":"a3s.code.execution-target.v1","session_id":"s","run_id":"r"}}}`)
	if _, err := DecodeEvaluationWireEnvelopeV1(append(valid, []byte(` {}`)...)); err == nil {
		t.Fatal("trailing JSON was accepted")
	}
}
