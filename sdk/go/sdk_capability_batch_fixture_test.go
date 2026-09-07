package code

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

type sdkCapabilityBatchFixture struct {
	SchemaVersion         int                  `json:"schema_version"`
	FixtureID             string               `json:"fixture_id"`
	RequiredBatchFields   []string             `json:"required_batch_fields"`
	RequiredReceiptFields []string             `json:"required_receipt_fields"`
	SampleBatch           SdkCapabilityBatchV1 `json:"sample_batch"`
	ExpectedReceipt       map[string]uint64    `json:"expected_receipt"`
	ForbiddenFields       []string             `json:"forbidden_fields"`
}

func loadSdkCapabilityBatchFixture(t *testing.T) sdkCapabilityBatchFixture {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve fixture path")
	}
	path := filepath.Join(filepath.Dir(file), "..", "evaluation", "sdk-capability-batch-v1.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read fixture: %v", err)
	}
	var fixture sdkCapabilityBatchFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatalf("decode fixture: %v", err)
	}
	if fixture.SchemaVersion != 1 || fixture.FixtureID != "sdk-capability-batch-v1" {
		t.Fatalf("unexpected fixture metadata: %#v", fixture)
	}
	return fixture
}

func TestSdkCapabilityBatchFixtureContract(t *testing.T) {
	fixture := loadSdkCapabilityBatchFixture(t)
	if fixture.SampleBatch.SchemaVersion != 1 {
		t.Fatalf("sample schemaVersion: %d", fixture.SampleBatch.SchemaVersion)
	}
	if len(fixture.SampleBatch.Skills) == 0 {
		t.Fatal("sample batch must stage skills")
	}
	for _, field := range fixture.RequiredBatchFields {
		switch field {
		case "schemaVersion", "generation", "sourceId", "skills":
		default:
			t.Fatalf("unexpected required batch field %q", field)
		}
	}

	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (any, error) {
			if operation != "session_apply_capability_batch" {
				t.Fatalf("unexpected operation %q", operation)
			}
			if params["session_handle"] != "session-cap-batch-fixture" {
				t.Fatalf("unexpected session handle %#v", params["session_handle"])
			}
			encoded, err := json.Marshal(params["batch"])
			if err != nil {
				t.Fatalf("marshal batch: %v", err)
			}
			var batch SdkCapabilityBatchV1
			if err := json.Unmarshal(encoded, &batch); err != nil {
				t.Fatalf("decode batch: %v", err)
			}
			if batch.Generation != 1 || len(batch.Skills) != 1 {
				t.Fatalf("unexpected batch %#v", batch)
			}
			return SdkCapabilityCommitReceiptV1{
				PreviousGeneration:  0,
				CommittedGeneration: 1,
				PreviousDigest:      "sha256:previous",
				CommittedDigest:     "sha256:committed",
			}, nil
		},
	}
	session := &Session{runtime: runtime, handle: "session-cap-batch-fixture"}
	receipt, err := session.ApplyCapabilityBatch(context.Background(), fixture.SampleBatch)
	if err != nil {
		t.Fatalf("ApplyCapabilityBatch: %v", err)
	}
	if receipt.PreviousGeneration != fixture.ExpectedReceipt["previousGeneration"] {
		t.Fatalf("previousGeneration=%d", receipt.PreviousGeneration)
	}
	if receipt.CommittedGeneration != fixture.ExpectedReceipt["committedGeneration"] {
		t.Fatalf("committedGeneration=%d", receipt.CommittedGeneration)
	}
	encoded, err := json.Marshal(receipt)
	if err != nil {
		t.Fatalf("marshal receipt: %v", err)
	}
	var object map[string]any
	if err := json.Unmarshal(encoded, &object); err != nil {
		t.Fatalf("decode receipt: %v", err)
	}
	for _, field := range fixture.RequiredReceiptFields {
		if _, ok := object[field]; !ok {
			t.Fatalf("missing receipt field %q", field)
		}
	}
	assertNoForbiddenMiddlewareFields(t, object, fixture.ForbiddenFields)
}
