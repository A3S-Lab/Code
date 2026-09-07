package code

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"
	"time"
)

type sdkCheckpointExportFixture struct {
	SchemaVersion            int                            `json:"schema_version"`
	FixtureID                string                         `json:"fixture_id"`
	RequiredExportFields     []string                       `json:"required_export_fields"`
	RequiredDescriptorFields []string                       `json:"required_descriptor_fields"`
	SampleExport             SdkSessionCheckpointExportV1   `json:"sample_export"`
	ForbiddenFields          []string                       `json:"forbidden_fields"`
}

func loadSdkCheckpointExportFixture(t *testing.T) sdkCheckpointExportFixture {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve fixture path")
	}
	path := filepath.Join(filepath.Dir(file), "..", "evaluation", "sdk-checkpoint-export-v1.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read fixture: %v", err)
	}
	var fixture sdkCheckpointExportFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatalf("decode fixture: %v", err)
	}
	if fixture.SchemaVersion != 1 || fixture.FixtureID != "sdk-checkpoint-export-v1" {
		t.Fatalf("unexpected fixture metadata: %#v", fixture)
	}
	return fixture
}

func TestSdkCheckpointExportFixtureContract(t *testing.T) {
	fixture := loadSdkCheckpointExportFixture(t)
	if fixture.SampleExport.ContentBase64 == "" {
		t.Fatal("sample export must include contentBase64")
	}
	var descriptor map[string]any
	if err := json.Unmarshal(fixture.SampleExport.Descriptor, &descriptor); err != nil {
		t.Fatalf("decode descriptor: %v", err)
	}
	for _, field := range fixture.RequiredDescriptorFields {
		if _, ok := descriptor[field]; !ok {
			t.Fatalf("missing descriptor field %q", field)
		}
	}

	var seenOp string
	var seenHandler string
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (any, error) {
			seenOp = operation
			if id, ok := params["handler_id"].(string); ok {
				seenHandler = id
			}
			return map[string]any{"configured": true}, nil
		},
	}
	session := &Session{runtime: runtime, handle: "session-cp-export-fixture"}
	received := make(chan SdkSessionCheckpointExportV1, 1)
	if err := session.SetSessionCheckpointExportSink(context.Background(), &SessionCheckpointExportHandler{
		ExportCheckpoint: func(_ context.Context, export SdkSessionCheckpointExportV1) error {
			received <- export
			return nil
		},
		Timeout: time.Second,
	}); err != nil {
		t.Fatalf("SetSessionCheckpointExportSink: %v", err)
	}
	if seenOp != "session_set_session_checkpoint_export_sink" {
		t.Fatalf("unexpected operation %q", seenOp)
	}
	if seenHandler == "" || session.checkpointCallback != seenHandler {
		t.Fatalf("handler id not retained: seen=%q session=%q", seenHandler, session.checkpointCallback)
	}

	runtime.mu.Lock()
	callback := runtime.callbacks[seenHandler]
	runtime.mu.Unlock()
	if callback == nil {
		t.Fatal("checkpoint callback was not registered")
	}
	payload, err := json.Marshal(fixture.SampleExport)
	if err != nil {
		t.Fatalf("marshal sample: %v", err)
	}
	reply, err := callback(context.Background(), "export_checkpoint", payload)
	if err != nil {
		t.Fatalf("callback invoke: %v", err)
	}
	okMap, ok := reply.(map[string]any)
	if !ok || okMap["ok"] != true {
		t.Fatalf("unexpected callback reply %#v", reply)
	}
	select {
	case got := <-received:
		if got.ContentBase64 != fixture.SampleExport.ContentBase64 {
			t.Fatalf("contentBase64=%q", got.ContentBase64)
		}
		encoded, err := json.Marshal(got)
		if err != nil {
			t.Fatalf("marshal received: %v", err)
		}
		var object map[string]any
		if err := json.Unmarshal(encoded, &object); err != nil {
			t.Fatalf("decode received: %v", err)
		}
		for _, field := range fixture.RequiredExportFields {
			if _, ok := object[field]; !ok {
				t.Fatalf("missing export field %q", field)
			}
		}
		assertNoForbiddenMiddlewareFields(t, object, fixture.ForbiddenFields)
	default:
		t.Fatal("handler did not receive export")
	}

	if err := session.SetSessionCheckpointExportSink(context.Background(), nil); err != nil {
		t.Fatalf("clear sink: %v", err)
	}
	if session.checkpointCallback != "" {
		t.Fatalf("checkpoint callback leaked after clear: %q", session.checkpointCallback)
	}
}
