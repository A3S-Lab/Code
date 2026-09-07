package code

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

type sdkImmutableContentFixture struct {
	SchemaVersion              int                               `json:"schema_version"`
	FixtureID                  string                            `json:"fixture_id"`
	RequiredWriteRequestFields []string                          `json:"required_write_request_fields"`
	RequiredBindingFields      []string                          `json:"required_binding_fields"`
	RequiredDescriptorFields   []string                          `json:"required_descriptor_fields"`
	RequiredReferenceFields    []string                          `json:"required_reference_fields"`
	SampleWriteRequest         SdkImmutableContentWriteRequestV1 `json:"sample_write_request"`
	SampleReference            ImmutableContentReferenceV1       `json:"sample_reference"`
	ForbiddenFields            []string                          `json:"forbidden_fields"`
}

func loadSdkImmutableContentFixture(t *testing.T) sdkImmutableContentFixture {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve fixture path")
	}
	path := filepath.Join(filepath.Dir(file), "..", "evaluation", "sdk-immutable-content-v1.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read fixture: %v", err)
	}
	var fixture sdkImmutableContentFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatalf("decode fixture: %v", err)
	}
	if fixture.SchemaVersion != 1 || fixture.FixtureID != "sdk-immutable-content-v1" {
		t.Fatalf("unexpected fixture metadata: %#v", fixture)
	}
	return fixture
}

func TestSdkImmutableContentFixtureContract(t *testing.T) {
	fixture := loadSdkImmutableContentFixture(t)
	if fixture.SampleWriteRequest.ContentBase64 == "" {
		t.Fatal("sample write request must include contentBase64")
	}
	var binding map[string]any
	if err := json.Unmarshal(fixture.SampleWriteRequest.Binding, &binding); err != nil {
		t.Fatalf("decode binding: %v", err)
	}
	for _, field := range fixture.RequiredBindingFields {
		if _, ok := binding[field]; !ok {
			t.Fatalf("missing binding field %q", field)
		}
	}
	var descriptor map[string]any
	if err := json.Unmarshal(fixture.SampleWriteRequest.Descriptor, &descriptor); err != nil {
		t.Fatalf("decode descriptor: %v", err)
	}
	for _, field := range fixture.RequiredDescriptorFields {
		if _, ok := descriptor[field]; !ok {
			t.Fatalf("missing descriptor field %q", field)
		}
	}
	digestHex, _ := strings.CutPrefix(fixture.SampleReference.ContentDigest, "sha256:")
	if digestHex == "" || !strings.Contains(fixture.SampleReference.URI, "/"+digestHex) {
		t.Fatalf("reference URI must contain content digest path segment: %q", fixture.SampleReference.URI)
	}

	runtime := &fakeRuntime{}
	runtime.request = func(
		ctx context.Context,
		operation string,
		params map[string]any,
	) (any, error) {
		switch operation {
		case "agent_create":
			return map[string]any{"agent_id": "agent-imm1"}, nil
		case "session_create":
			wire, ok := params["options"].(map[string]any)
			if !ok {
				t.Fatalf("prepared options = %#v", params["options"])
			}
			adapter, ok := wire["immutable_content_adapter"].(immutableContentAdapterWireOptions)
			if !ok || adapter.HandlerID == "" || adapter.AdapterName != "go-fixture" ||
				adapter.MaximumBytes != 4096 || adapter.AuthorityDigest == "" {
				t.Fatalf("immutable content wire options = %#v", wire["immutable_content_adapter"])
			}
			runtime.mu.Lock()
			callback := runtime.callbacks[adapter.HandlerID]
			runtime.mu.Unlock()
			if callback == nil {
				t.Fatal("put callback was not registered before session creation")
			}
			payload, err := json.Marshal(fixture.SampleWriteRequest)
			if err != nil {
				t.Fatal(err)
			}
			value, err := callback(ctx, "put", payload)
			if err != nil {
				t.Fatal(err)
			}
			reference, ok := value.(*ImmutableContentReferenceV1)
			if !ok || reference == nil {
				t.Fatalf("put response = %#v", value)
			}
			if reference.URI != fixture.SampleReference.URI {
				t.Fatalf("put URI = %q, want %q", reference.URI, fixture.SampleReference.URI)
			}
			if !strings.Contains(reference.URI, digestHex) {
				t.Fatalf("put URI missing digest segment: %q", reference.URI)
			}
			return map[string]any{
				"session_handle": "session-handle-imm1",
				"session_id":     "session-id-imm1",
				"workspace":      "/tmp/imm1",
			}, nil
		case "session_close", "agent_close":
			return map[string]any{}, nil
		default:
			t.Fatalf("unexpected operation %q", operation)
			return nil, nil
		}
	}

	agent, err := Create(context.Background(), "inline acl", WithRuntime(runtime))
	if err != nil {
		t.Fatal(err)
	}
	session, err := agent.Session(context.Background(), "/tmp/imm1", &SessionOptions{
		ImmutableContentAdapter: &ImmutableContentAdapterOptions{
			AuthorityDigest: "sha256:" + strings.Repeat("a", 64),
			MaximumBytes:    4096,
			AdapterName:     "go-fixture",
			Timeout:         time.Second,
			Put: func(
				_ context.Context,
				req SdkImmutableContentWriteRequestV1,
			) (*ImmutableContentReferenceV1, error) {
				if req.ContentBase64 != fixture.SampleWriteRequest.ContentBase64 {
					t.Fatalf("contentBase64 = %q", req.ContentBase64)
				}
				ref := fixture.SampleReference
				return &ref, nil
			},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	runtime.mu.Lock()
	callbackCount := len(runtime.callbacks)
	runtime.mu.Unlock()
	if callbackCount != 1 {
		t.Fatalf("callbacks after create = %d, want 1", callbackCount)
	}
	if err := session.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	runtime.mu.Lock()
	callbackCount = len(runtime.callbacks)
	runtime.mu.Unlock()
	if callbackCount != 0 {
		t.Fatalf("callbacks leaked after close: %d", callbackCount)
	}
	if err := agent.Close(context.Background()); err != nil {
		t.Fatal(err)
	}

	encoded, err := json.Marshal(fixture.SampleReference)
	if err != nil {
		t.Fatal(err)
	}
	var object map[string]any
	if err := json.Unmarshal(encoded, &object); err != nil {
		t.Fatal(err)
	}
	for _, field := range fixture.RequiredReferenceFields {
		if _, ok := object[field]; !ok {
			t.Fatalf("missing reference field %q", field)
		}
	}
	rawText := string(encoded)
	for _, forbidden := range fixture.ForbiddenFields {
		if strings.Contains(strings.ToLower(rawText), strings.ToLower(forbidden)) {
			t.Fatalf("forbidden field %q present in sample reference", forbidden)
		}
	}
}

func TestPrepareImmutableContentAdapterOptionsValidation(t *testing.T) {
	runtime := &fakeRuntime{}
	_, callbackID, err := prepareImmutableContentAdapterOptions(runtime, nil, &SessionOptions{
		ImmutableContentAdapter: &ImmutableContentAdapterOptions{
			AuthorityDigest: "sha256:" + strings.Repeat("a", 64),
			MaximumBytes:    1024,
			AdapterName:     "go-fixture",
			Put:             nil,
		},
	})
	if err == nil || callbackID != "" {
		t.Fatalf("nil Put = callback %q, error %v", callbackID, err)
	}
}
