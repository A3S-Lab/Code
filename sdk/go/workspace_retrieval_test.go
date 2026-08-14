package code

import (
	"context"
	"encoding/json"
	"errors"
	"math"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

type fixtureEmbeddingProvider struct {
	embed func(context.Context, EmbeddingBatchRequest) (EmbeddingBatchResponse, error)
}

func (provider *fixtureEmbeddingProvider) Descriptor() EmbeddingProviderDescriptor {
	return EmbeddingProviderDescriptor{
		Provider:      "go-fixture",
		Model:         "deterministic-v1",
		Dimension:     4,
		Normalization: EmbeddingNormalizationUnit,
	}
}

func (provider *fixtureEmbeddingProvider) Embed(
	ctx context.Context,
	request EmbeddingBatchRequest,
) (EmbeddingBatchResponse, error) {
	return provider.embed(ctx, request)
}

func TestWorkspaceRetrievalProviderLifecycle(t *testing.T) {
	provider := &fixtureEmbeddingProvider{
		embed: func(
			_ context.Context,
			request EmbeddingBatchRequest,
		) (EmbeddingBatchResponse, error) {
			vectors := make([]EmbeddingVector, 0, len(request.Inputs))
			for _, input := range request.Inputs {
				vectors = append(vectors, EmbeddingVector{
					ID:     input.ID,
					Values: []float32{1, 0, 0, 0},
				})
			}
			return EmbeddingBatchResponse{Vectors: vectors}, nil
		},
	}
	runtime := &fakeRuntime{}
	runtime.request = func(
		ctx context.Context,
		operation string,
		params map[string]any,
	) (any, error) {
		switch operation {
		case "agent_create":
			return map[string]any{"agent_id": "agent-1"}, nil
		case "session_create":
			wire, ok := params["options"].(map[string]any)
			if !ok {
				t.Fatalf("prepared options = %#v", params["options"])
			}
			retrieval, ok := wire["workspace_retrieval"].(workspaceRetrievalWireOptions)
			if !ok || retrieval.HandlerID == "" || retrieval.Dimension != 4 {
				t.Fatalf("retrieval wire options = %#v", wire["workspace_retrieval"])
			}
			runtime.mu.Lock()
			callback := runtime.callbacks[retrieval.HandlerID]
			runtime.mu.Unlock()
			if callback == nil {
				t.Fatal("embedding callback was not registered before session creation")
			}
			value, err := callback(
				ctx,
				"embedding",
				json.RawMessage(`{"inputs":[{"id":"chunk-1","text":"cleanup"}],"text_bytes":7}`),
			)
			if err != nil {
				t.Fatal(err)
			}
			response := value.(EmbeddingBatchResponse)
			if len(response.Vectors) != 1 || response.Vectors[0].ID != "chunk-1" {
				t.Fatalf("embedding response = %#v", response)
			}
			return map[string]any{
				"session_handle": "session-handle",
				"session_id":     "session-id",
				"workspace":      "C:/repo",
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
	session, err := agent.Session(context.Background(), "C:/repo", &SessionOptions{
		WorkspaceRetrieval: NewWorkspaceRetrievalOptions(provider),
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
}

func TestWorkspaceRetrievalCreationFailureCleansUpCallback(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			_ map[string]any,
		) (any, error) {
			switch operation {
			case "agent_create":
				return map[string]any{"agent_id": "agent-1"}, nil
			case "session_create":
				return nil, errors.New("session failed")
			default:
				return map[string]any{}, nil
			}
		},
	}
	agent, err := Create(context.Background(), "inline acl", WithRuntime(runtime))
	if err != nil {
		t.Fatal(err)
	}
	provider := &fixtureEmbeddingProvider{
		embed: func(
			context.Context,
			EmbeddingBatchRequest,
		) (EmbeddingBatchResponse, error) {
			return EmbeddingBatchResponse{}, nil
		},
	}
	_, err = agent.Session(context.Background(), "C:/repo", &SessionOptions{
		WorkspaceRetrieval: NewWorkspaceRetrievalOptions(provider),
	})
	if err == nil {
		t.Fatal("session creation should fail")
	}
	runtime.mu.Lock()
	defer runtime.mu.Unlock()
	if len(runtime.callbacks) != 0 {
		t.Fatalf("callbacks leaked after failed create: %#v", runtime.callbacks)
	}
}

func TestWorkspaceRetrievalProviderIsBoundAtSessionCreation(t *testing.T) {
	var firstCalls atomic.Uint64
	var replacementCalls atomic.Uint64
	first := &fixtureEmbeddingProvider{
		embed: func(
			_ context.Context,
			request EmbeddingBatchRequest,
		) (EmbeddingBatchResponse, error) {
			firstCalls.Add(1)
			return EmbeddingBatchResponse{Vectors: []EmbeddingVector{{
				ID: request.Inputs[0].ID, Values: []float32{1, 0, 0, 0},
			}}}, nil
		},
	}
	replacement := &fixtureEmbeddingProvider{
		embed: func(
			_ context.Context,
			request EmbeddingBatchRequest,
		) (EmbeddingBatchResponse, error) {
			replacementCalls.Add(1)
			return EmbeddingBatchResponse{Vectors: []EmbeddingVector{{
				ID: request.Inputs[0].ID, Values: []float32{0, 1, 0, 0},
			}}}, nil
		},
	}
	runtime := &fakeRuntime{}
	options := &SessionOptions{
		WorkspaceRetrieval: NewWorkspaceRetrievalOptions(first),
	}
	prepared, callbackID, err := prepareWorkspaceRetrievalOptions(runtime, options)
	if err != nil {
		t.Fatal(err)
	}
	defer runtime.unregisterCallback(callbackID)
	if prepared == nil || callbackID == "" {
		t.Fatalf("prepared = %#v, callback id = %q", prepared, callbackID)
	}

	options.WorkspaceRetrieval.Provider = replacement
	runtime.mu.Lock()
	callback := runtime.callbacks[callbackID]
	runtime.mu.Unlock()
	if callback == nil {
		t.Fatal("embedding callback was not registered")
	}
	_, err = callback(
		context.Background(),
		"embedding",
		json.RawMessage(`{"inputs":[{"id":"chunk-1","text":"cleanup"}],"text_bytes":7}`),
	)
	if err != nil {
		t.Fatal(err)
	}
	if firstCalls.Load() != 1 || replacementCalls.Load() != 0 {
		t.Fatalf(
			"provider calls = first %d, replacement %d",
			firstCalls.Load(),
			replacementCalls.Load(),
		)
	}
}

func TestWorkspaceRetrievalProviderFailuresAreTypedAndRedacted(t *testing.T) {
	retryAfter := 25 * time.Millisecond
	failure := embeddingFailure(context.Background(), &EmbeddingError{
		Kind:       EmbeddingFailureRateLimited,
		RetryAfter: retryAfter,
		Err:        errors.New("private remote response body"),
	})
	if failure.Kind != EmbeddingFailureRateLimited ||
		failure.RetryAfterMS == nil || *failure.RetryAfterMS != 25 {
		t.Fatalf("failure = %#v", failure)
	}
	encoded, err := json.Marshal(failure)
	if err != nil {
		t.Fatal(err)
	}
	if string(encoded) != `{"kind":"rate_limited","retry_after_ms":25}` {
		t.Fatalf("serialized failure leaks or drifts: %s", encoded)
	}
	if finiteVector([]float32{1, float32(math.NaN())}) {
		t.Fatal("non-finite vectors must be rejected")
	}
}

func TestWorkspaceRetrievalTypedSessionMethods(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (any, error) {
			switch operation {
			case "session_workspace_retrieval_status":
				return map[string]any{
					"phase":          "ready",
					"indexed_chunks": 2,
				}, nil
			case "session_semantic_search":
				return map[string]any{
					"hits": []any{map[string]any{
						"chunk": map[string]any{
							"path":            "src/session.go",
							"digest_verified": true,
						},
						"score": 0.75,
					}},
					"status": map[string]any{"phase": "ready"},
				}, nil
			case "session_hybrid_search":
				request := params["request"].(WorkspaceSearchRequest)
				if request.Query != "terminate_owned_tasks" {
					t.Fatalf("request = %#v", request)
				}
				return map[string]any{
					"hits": []any{map[string]any{
						"chunk":            map[string]any{"path": "src/session.go"},
						"fused_score":      0.5,
						"rerank_score":     0.5,
						"redundancy_score": 0.0,
						"exact_identifier": true,
						"channels": []any{map[string]any{
							"channel": "exact",
							"rank":    1,
						}},
					}},
					"semantic_status": map[string]any{"phase": "ready"},
					"channels": []any{map[string]any{
						"channel":         "exact",
						"candidate_count": 1,
					}},
					"rerank": map[string]any{
						"requested_mode":      "rrf_only",
						"applied_mode":        "rrf_only",
						"algorithm":           "rrf_k60",
						"selected_candidates": 1,
					},
				}, nil
			default:
				t.Fatalf("unexpected operation %q", operation)
				return nil, nil
			}
		},
	}
	session := testSession(runtime)
	status, err := session.WorkspaceRetrievalStatus(context.Background())
	if err != nil || status.Phase != WorkspaceRetrievalReady || status.IndexedChunks != 2 {
		t.Fatalf("status = %#v, %v", status, err)
	}
	semantic, err := session.SemanticSearch(context.Background(), WorkspaceSearchRequest{
		Query: "session cleanup",
	})
	if err != nil || len(semantic.Hits) != 1 ||
		!semantic.Hits[0].Chunk.DigestVerified {
		t.Fatalf("semantic = %#v, %v", semantic, err)
	}
	hybrid, err := session.HybridSearch(context.Background(), WorkspaceSearchRequest{
		Query: "terminate_owned_tasks",
	})
	if err != nil || len(hybrid.Hits) != 1 ||
		!hybrid.Hits[0].ExactIdentifier ||
		hybrid.Hits[0].RerankScore != hybrid.Hits[0].FusedScore ||
		hybrid.Rerank.AppliedMode != WorkspaceRerankRRFOnly ||
		hybrid.Rerank.Algorithm != WorkspaceRerankAlgorithmRRFK60 ||
		hybrid.Channels[0].CandidateCount != 1 {
		t.Fatalf("hybrid = %#v, %v", hybrid, err)
	}
}

func TestRustBridgeWorkspaceRetrievalIntegration(t *testing.T) {
	binary := os.Getenv("A3S_CODE_GO_BRIDGE_TEST_BINARY")
	if binary == "" {
		t.Skip("set A3S_CODE_GO_BRIDGE_TEST_BINARY to run the Rust bridge integration")
	}
	workspace := t.TempDir()
	source := filepath.Join(workspace, "src", "session_cleanup.rs")
	if err := os.MkdirAll(filepath.Dir(source), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(
		source,
		[]byte("pub fn terminate_owned_tasks() {\n    // release every session resource\n}\n"),
		0o644,
	); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	agent, err := Create(
		ctx,
		`
			default_model = "anthropic/test-model"
			providers "anthropic" {
				apiKey = "test-key"
				models "test-model" {
					name = "Test Model"
				}
			}
		`,
		WithLocalRuntimeOptions(WithBridgePath(binary)),
	)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := agent.Close(context.Background()); err != nil {
			t.Errorf("Agent.Close: %v", err)
		}
	})

	var providerCalls atomic.Uint32
	provider := &fixtureEmbeddingProvider{
		embed: func(
			_ context.Context,
			request EmbeddingBatchRequest,
		) (EmbeddingBatchResponse, error) {
			providerCalls.Add(1)
			vectors := make([]EmbeddingVector, 0, len(request.Inputs))
			for _, input := range request.Inputs {
				values := []float32{0, 1, 0, 0}
				text := strings.ToLower(input.Text)
				if strings.Contains(text, "cleanup") ||
					strings.Contains(text, "release every session resource") {
					values = []float32{1, 0, 0, 0}
				}
				vectors = append(vectors, EmbeddingVector{ID: input.ID, Values: values})
			}
			return EmbeddingBatchResponse{Vectors: vectors}, nil
		},
	}
	retrieval := NewWorkspaceRetrievalOptions(provider)
	retrieval.MaxRecords = 100
	retrieval.MaxBytes = 1024 * 1024
	session, err := agent.Session(ctx, workspace, &SessionOptions{
		WorkspaceRetrieval: retrieval,
	})
	if err != nil {
		t.Fatal(err)
	}
	status := waitForRetrievalPhase(t, ctx, session, WorkspaceRetrievalReady)
	if status.IndexedChunks == 0 {
		t.Fatalf("ready status has no indexed chunks: %#v", status)
	}
	semantic, err := session.SemanticSearch(ctx, WorkspaceSearchRequest{
		Query: "cleanup session resources",
		Limit: 3,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(semantic.Hits) == 0 ||
		semantic.Hits[0].Chunk.Path != "src/session_cleanup.rs" ||
		!semantic.Hits[0].Chunk.DigestVerified {
		t.Fatalf("semantic result = %#v", semantic)
	}
	hybrid, err := session.HybridSearch(ctx, WorkspaceSearchRequest{
		Query: "terminate_owned_tasks",
		Limit: 3,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(hybrid.Hits) == 0 ||
		hybrid.Hits[0].Chunk.Path != "src/session_cleanup.rs" ||
		!hybrid.Hits[0].ExactIdentifier ||
		hybrid.Rerank.AppliedMode != WorkspaceRerankRRFOnly ||
		!hasWorkspaceChannel(hybrid.Hits[0].Channels, WorkspaceRetrievalExact) {
		t.Fatalf("hybrid result = %#v", hybrid)
	}
	if providerCalls.Load() < 2 {
		t.Fatalf("provider calls = %d, want at least 2", providerCalls.Load())
	}
	if err := session.Close(ctx); err != nil {
		t.Fatal(err)
	}

	slowWorkspace := t.TempDir()
	slowSource := filepath.Join(slowWorkspace, "src", "session_cleanup.rs")
	if err := os.MkdirAll(filepath.Dir(slowSource), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(
		slowSource,
		[]byte("pub fn terminate_owned_tasks() {\n    // release every session resource\n}\n"),
		0o644,
	); err != nil {
		t.Fatal(err)
	}
	slowCtx, slowCancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer slowCancel()
	started := make(chan struct{})
	cancelled := make(chan struct{})
	var startOnce sync.Once
	var cancelOnce sync.Once
	slowProvider := &fixtureEmbeddingProvider{
		embed: func(
			ctx context.Context,
			_ EmbeddingBatchRequest,
		) (EmbeddingBatchResponse, error) {
			startOnce.Do(func() { close(started) })
			<-ctx.Done()
			cancelOnce.Do(func() { close(cancelled) })
			return EmbeddingBatchResponse{}, ctx.Err()
		},
	}
	slowSession, err := agent.Session(slowCtx, slowWorkspace, &SessionOptions{
		WorkspaceRetrieval: NewWorkspaceRetrievalOptions(slowProvider),
	})
	if err != nil {
		t.Fatal(err)
	}
	lastStatus := WorkspaceRetrievalStatus{}
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	waiting := true
	for waiting {
		select {
		case <-started:
			waiting = false
		case <-ticker.C:
			lastStatus, err = slowSession.WorkspaceRetrievalStatus(slowCtx)
			if err != nil {
				t.Fatal(err)
			}
			if lastStatus.Phase == WorkspaceRetrievalDegraded ||
				lastStatus.Phase == WorkspaceRetrievalClosed {
				t.Fatalf("slow retrieval terminated before provider start: %#v", lastStatus)
			}
		case <-slowCtx.Done():
			t.Fatalf(
				"background index did not invoke the Go provider; last status: %#v",
				lastStatus,
			)
		}
	}
	if err := slowSession.Close(slowCtx); err != nil {
		t.Fatal(err)
	}
	select {
	case <-cancelled:
	case <-time.After(2 * time.Second):
		t.Fatal("session close did not cancel the Go embedding context")
	}
	closed, err := slowSession.WorkspaceRetrievalStatus(slowCtx)
	if err != nil {
		t.Fatal(err)
	}
	if closed.Phase != WorkspaceRetrievalClosed {
		t.Fatalf("closed status = %#v", closed)
	}
}

func waitForRetrievalPhase(
	t *testing.T,
	ctx context.Context,
	session *Session,
	want WorkspaceRetrievalPhase,
) WorkspaceRetrievalStatus {
	t.Helper()
	for {
		status, err := session.WorkspaceRetrievalStatus(ctx)
		if err != nil {
			t.Fatal(err)
		}
		if status.Phase == want {
			return status
		}
		if status.Phase == WorkspaceRetrievalDegraded ||
			status.Phase == WorkspaceRetrievalClosed {
			t.Fatalf("retrieval entered terminal phase before %s: %#v", want, status)
		}
		select {
		case <-ctx.Done():
			t.Fatalf("timed out waiting for retrieval phase %s: %v", want, ctx.Err())
		case <-time.After(20 * time.Millisecond):
		}
	}
}

func hasWorkspaceChannel(
	ranks []WorkspaceHybridChannelRank,
	want WorkspaceRetrievalChannel,
) bool {
	for _, rank := range ranks {
		if rank.Channel == want {
			return true
		}
	}
	return false
}
