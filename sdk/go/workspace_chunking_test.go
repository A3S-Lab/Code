package code

import (
	"context"
	"encoding/json"
	"os"
	"strings"
	"testing"
)

type sdkChunkingFixture struct {
	Schema         string                     `json:"schema"`
	Cases          []sdkChunkingFixtureCase   `json:"cases"`
	InvalidWindows []sdkChunkingInvalidWindow `json:"invalid_windows"`
}

type sdkChunkingFixtureCase struct {
	Name         string   `json:"name"`
	TargetBytes  *uint    `json:"target_bytes"`
	OverlapBytes *uint    `json:"overlap_bytes"`
	Separators   []string `json:"separators"`
}

type sdkChunkingInvalidWindow struct {
	Name         string `json:"name"`
	TargetBytes  uint   `json:"target_bytes"`
	OverlapBytes uint   `json:"overlap_bytes"`
}

func loadSDKChunkingFixture(t *testing.T) sdkChunkingFixture {
	t.Helper()
	encoded, err := os.ReadFile("../../core/tests/fixtures/workspace-chunking-sdk-v1.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture sdkChunkingFixture
	if err := json.Unmarshal(encoded, &fixture); err != nil {
		t.Fatal(err)
	}
	return fixture
}

func TestTypedWorkspaceChunkingStrategiesMatchSharedFixture(t *testing.T) {
	fixture := loadSDKChunkingFixture(t)
	if fixture.Schema != "a3s.workspace-chunking-sdk.fixture.v1" {
		t.Fatalf("fixture schema = %q", fixture.Schema)
	}
	for _, test := range fixture.Cases {
		t.Run(test.Name, func(t *testing.T) {
			var strategy WorkspaceChunkingStrategy
			switch test.Name {
			case "line":
				strategy = NewLineWorkspaceChunkingStrategy()
			case "fixed_window":
				value, err := NewFixedWindowWorkspaceChunkingStrategy(
					*test.TargetBytes,
					*test.OverlapBytes,
				)
				if err != nil {
					t.Fatal(err)
				}
				strategy = value
			case "recursive":
				value, err := NewRecursiveWorkspaceChunkingStrategy(
					*test.TargetBytes,
					*test.OverlapBytes,
					test.Separators...,
				)
				if err != nil {
					t.Fatal(err)
				}
				strategy = value
			default:
				t.Fatalf("unknown fixture strategy %q", test.Name)
			}
			wire, err := strategy.workspaceChunkingStrategyWire()
			if err != nil {
				t.Fatal(err)
			}
			encoded, err := json.Marshal(wire)
			if err != nil {
				t.Fatal(err)
			}
			for _, primitive := range []string{`"kind"`, `"strategy"`, `"mode"`} {
				if strings.Contains(string(encoded), primitive) {
					t.Fatalf("typed wire contains primitive selector %s: %s", primitive, encoded)
				}
			}
		})
	}
}

func TestWorkspaceChunkingRejectsSharedInvalidWindows(t *testing.T) {
	for _, test := range loadSDKChunkingFixture(t).InvalidWindows {
		t.Run(test.Name, func(t *testing.T) {
			if _, err := NewFixedWindowWorkspaceChunkingStrategy(
				test.TargetBytes,
				test.OverlapBytes,
			); err == nil {
				t.Fatal("invalid window must fail")
			}
		})
	}
}

func TestWorkspaceChunkingValidationPrecedesCallbackRegistration(t *testing.T) {
	runtime := &fakeRuntime{}
	provider := &fixtureEmbeddingProvider{
		embed: func(
			context.Context,
			EmbeddingBatchRequest,
		) (EmbeddingBatchResponse, error) {
			t.Fatal("invalid chunking must not call the embedding provider")
			return EmbeddingBatchResponse{}, nil
		},
	}
	retrieval := NewWorkspaceRetrievalOptions(provider)
	retrieval.ChunkingStrategy = &FixedWindowWorkspaceChunkingStrategy{
		TargetBytes:  3,
		OverlapBytes: 0,
	}
	_, callbackID, err := prepareWorkspaceRetrievalOptions(runtime, &SessionOptions{
		WorkspaceRetrieval: retrieval,
	})
	if err == nil || callbackID != "" {
		t.Fatalf("invalid chunking = callback %q, error %v", callbackID, err)
	}
	runtime.mu.Lock()
	callbackCount := len(runtime.callbacks)
	runtime.mu.Unlock()
	if callbackCount != 0 {
		t.Fatalf("invalid chunking registered callbacks: %d", callbackCount)
	}
}

func TestRecursiveWorkspaceChunkingRejectsInvalidSeparatorsAndTypedNil(t *testing.T) {
	for _, separators := range [][]string{
		{},
		{"\n", "\n"},
		{"\x00"},
		{strings.Repeat("x", maximumWorkspaceSeparatorBytes+1)},
	} {
		strategy := &RecursiveWorkspaceChunkingStrategy{
			TargetBytes:  64,
			OverlapBytes: 0,
			Separators:   separators,
		}
		if _, err := strategy.workspaceChunkingStrategyWire(); err == nil {
			t.Fatalf("invalid separators accepted: %#v", separators)
		}
	}

	var typedNil *FixedWindowWorkspaceChunkingStrategy
	var strategy WorkspaceChunkingStrategy = typedNil
	if _, err := strategy.workspaceChunkingStrategyWire(); err == nil {
		t.Fatal("typed nil strategy must fail")
	}

	first, err := NewRecursiveWorkspaceChunkingStrategy(64, 0)
	if err != nil {
		t.Fatal(err)
	}
	first.Separators[0] = "changed"
	second, err := NewRecursiveWorkspaceChunkingStrategy(64, 0)
	if err != nil {
		t.Fatal(err)
	}
	if second.Separators[0] != "\n\n" {
		t.Fatalf("default separators were mutated: %#v", second.Separators)
	}
}
