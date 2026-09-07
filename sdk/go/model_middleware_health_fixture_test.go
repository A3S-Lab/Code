package code

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

type modelMiddlewareHealthFixture struct {
	SchemaVersion          int      `json:"schema_version"`
	ReportSchemaVersion    int      `json:"report_schema_version"`
	FixtureID              string   `json:"fixture_id"`
	RequiredSnapshotFields []string `json:"required_snapshot_fields"`
	ForbiddenFields        []string `json:"forbidden_fields"`
}

func loadModelMiddlewareHealthFixture(t *testing.T) modelMiddlewareHealthFixture {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve fixture path")
	}
	path := filepath.Join(filepath.Dir(file), "..", "evaluation", "model-middleware-health-v1.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read fixture: %v", err)
	}
	var fixture modelMiddlewareHealthFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatalf("decode fixture: %v", err)
	}
	if fixture.SchemaVersion != 1 || fixture.FixtureID != "model-middleware-health-v1" {
		t.Fatalf("unexpected fixture metadata: %#v", fixture)
	}
	return fixture
}

func TestModelMiddlewareHealthFixtureWithRuntime(t *testing.T) {
	fixture := loadModelMiddlewareHealthFixture(t)
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (any, error) {
			if operation != "session_model_middleware_health" {
				t.Fatalf("unexpected operation %q", operation)
			}
			if params["session_handle"] != "session-middleware-fixture" {
				t.Fatalf("unexpected session params: %#v", params)
			}
			return ModelMiddlewareHealthSnapshot{}, nil
		},
	}
	session := &Session{runtime: runtime, handle: "session-middleware-fixture"}
	health, err := session.ModelMiddlewareHealth(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	validateModelMiddlewareHealthFixture(t, health, fixture)
}

func validateModelMiddlewareHealthFixture(
	t *testing.T,
	health ModelMiddlewareHealthSnapshot,
	fixture modelMiddlewareHealthFixture,
) {
	t.Helper()
	encoded, err := json.Marshal(health)
	if err != nil {
		t.Fatalf("marshal health: %v", err)
	}
	var object map[string]any
	if err := json.Unmarshal(encoded, &object); err != nil {
		t.Fatalf("decode health object: %v", err)
	}
	for _, field := range fixture.RequiredSnapshotFields {
		if _, ok := object[field]; !ok {
			t.Fatalf("missing required field %q", field)
		}
	}
	assertNoForbiddenMiddlewareFields(t, object, fixture.ForbiddenFields)
	if health.TrustAdmitted != 0 ||
		health.TrustRejected != 0 ||
		health.ProviderCalls != 0 ||
		health.UsageRecorded != 0 {
		t.Fatalf("fresh session counters must be zero: %#v", health)
	}
}

func assertNoForbiddenMiddlewareFields(t *testing.T, value any, forbidden []string) {
	t.Helper()
	deny := make(map[string]struct{}, len(forbidden))
	for _, field := range forbidden {
		deny[field] = struct{}{}
	}
	switch typed := value.(type) {
	case map[string]any:
		for key, child := range typed {
			if _, blocked := deny[key]; blocked {
				t.Fatalf("forbidden diagnostic field %q", key)
			}
			assertNoForbiddenMiddlewareFields(t, child, forbidden)
		}
	case []any:
		for _, child := range typed {
			assertNoForbiddenMiddlewareFields(t, child, forbidden)
		}
	}
}
