package code

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const sdkRealGuidelines = "This is a deterministic repository retrieval evaluation. Follow the requested one-tool protocol exactly. Never guess an identifier that is absent from the tool evidence."

func TestWorkspaceRetrievalRealDeepSeek(t *testing.T) {
	evaluationRoot := os.Getenv("A3S_REAL_EVAL_ROOT")
	if evaluationRoot == "" {
		t.Skip("set A3S_REAL_EVAL_ROOT to run the DeepSeek evaluation")
	}
	bridgeBinary := os.Getenv("A3S_CODE_GO_BRIDGE_TEST_BINARY")
	if bridgeBinary == "" {
		bridgeBinary = os.Getenv("A3S_CODE_GO_BRIDGE")
	}
	if bridgeBinary == "" {
		t.Fatal("set A3S_CODE_GO_BRIDGE_TEST_BINARY to the matching Rust bridge")
	}
	configPath := filepath.Join(evaluationRoot, ".a3s", "config.acl")
	if info, err := os.Stat(configPath); err != nil || info.IsDir() {
		t.Fatalf("repository .a3s/config.acl is required: %v", err)
	}
	fixture := loadSDKRealFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Minute)
	defer cancel()
	agent, err := Create(
		ctx,
		configPath,
		WithLocalRuntimeOptions(WithBridgePath(bridgeBinary)),
	)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := agent.Close(context.Background()); err != nil {
			t.Errorf("Agent.Close: %v", err)
		}
	})
	runs := make([]sdkRealRunMetric, 0, len(fixture.Tasks))
	for index, task := range fixture.Tasks {
		runs = append(runs, runSDKRealTask(t, ctx, agent, fixture, task, index))
	}
	summary := summarizeSDKRealRuns(runs)
	if summary.TaskAccuracy != 1 || summary.ToolProtocolRate != 1 || summary.RecallAt5 != 1 {
		t.Fatalf("quality gates failed: %#v", summary)
	}
	if summary.DocumentRequestAmplification > 1.1 {
		t.Fatalf("document request amplification = %f", summary.DocumentRequestAmplification)
	}
	if summary.NonTextProviderInputs != 0 {
		t.Fatalf("non-text provider inputs = %d", summary.NonTextProviderInputs)
	}
	if summary.ReleasedAfterCloseRate != 1 {
		t.Fatalf("released-after-close rate = %f", summary.ReleasedAfterCloseRate)
	}
	report := sdkRealEvaluationReport{
		SchemaVersion:  fixture.ReportSchemaVersion,
		FixtureID:      fixture.FixtureID,
		FixtureDigest:  fixture.Corpus.ExpectedDigest,
		SDK:            "go",
		ChatModel:      fixture.ChatModel,
		Chunking:       fixture.Chunking,
		Rerank:         fixture.Rerank,
		Summary:        summary,
		Runs:           runs,
		AllGatesPassed: true,
	}
	encoded, err := json.Marshal(report)
	if err != nil {
		t.Fatal(err)
	}
	fmt.Printf("WSR_SDK_DEEPSEEK_EVAL=%s\n", encoded)
}

func runSDKRealTask(
	t *testing.T,
	ctx context.Context,
	agent *Agent,
	fixture sdkRealFixture,
	task sdkRealFixtureTask,
	ordinal int,
) sdkRealRunMetric {
	t.Helper()
	workspace := t.TempDir()
	digest := materializeSDKRealCorpus(t, workspace, fixture)
	if digest != fixture.Corpus.ExpectedDigest {
		t.Fatalf("corpus digest = %s, want %s", digest, fixture.Corpus.ExpectedDigest)
	}
	counters := &sdkRealProviderCounters{}
	provider := &sdkRealEmbeddingProvider{fixture: fixture, counters: counters}
	retrieval := NewWorkspaceRetrievalOptions(provider)
	retrieval.Reranker = NewDeterministicWorkspaceReranker()
	chunking, err := NewRecursiveWorkspaceChunkingStrategy(
		fixture.Chunking.TargetBytes,
		fixture.Chunking.OverlapBytes,
		fixture.Chunking.Separators...,
	)
	if err != nil {
		t.Fatal(err)
	}
	retrieval.ChunkingStrategy = chunking
	constructionStarted := time.Now()
	session, err := agent.Session(ctx, workspace, &SessionOptions{
		SessionID:               fmt.Sprintf("wsr-sdk-go-%d", ordinal),
		Model:                   fixture.ChatModel,
		PlanningMode:            PlanningDisabled,
		GoalTracking:            Ptr(false),
		PermissionPolicy:        &PermissionPolicy{Allow: []string{"search(*)"}, DefaultDecision: "deny"},
		PromptSlots:             &PromptSlots{Guidelines: sdkRealGuidelines},
		MaxParseRetries:         Ptr(uint32(1)),
		MaxToolRounds:           Ptr(uint(2)),
		AutoDelegationEnabled:   Ptr(false),
		ManualDelegationEnabled: Ptr(false),
		Temperature:             Ptr(float32(0)),
		WorkspaceRetrieval:      retrieval,
	})
	if err != nil {
		t.Fatal(err)
	}
	closed := false
	defer func() {
		if !closed {
			_ = session.Close(context.Background())
		}
	}()
	sessionConstructionMS := sdkRealElapsedMS(constructionStarted)
	indexStarted := time.Now()
	status := waitForSDKRealReady(t, ctx, session)
	indexReadyMS := sdkRealElapsedMS(indexStarted)
	assertSDKRealReadyStatus(t, fixture, status, counters.snapshot())
	turnCtx, cancel := context.WithTimeout(ctx, 240*time.Second)
	defer cancel()
	turnStarted := time.Now()
	result, err := session.Run(turnCtx, sdkRealTaskPrompt(task))
	if err != nil {
		t.Fatal(err)
	}
	turnElapsedMS := sdkRealElapsedMS(turnStarted)
	runSnapshots, err := session.Runs(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(runSnapshots) != 1 || runSnapshots[0].Status != RunCompleted {
		t.Fatalf("run snapshots = %#v", runSnapshots)
	}
	events, err := session.RunEvents(ctx, runSnapshots[0].ID)
	if err != nil {
		t.Fatal(err)
	}
	calls, err := sdkRealToolCalls(events)
	if err != nil {
		t.Fatal(err)
	}
	var call sdkRealToolCall
	if len(calls) == 1 {
		call = calls[0]
	}
	rank, resultCount := sdkRealResultMetrics(call, task)
	protocolOK := result.ToolCallsCount == 1 && len(calls) == 1 &&
		call.Name == "search" && call.ExitCode == 0 &&
		call.Args["query"] == task.Query && call.Args["path"] == "." &&
		call.Args["include"] == "*.rs" && call.Args["limit"] == float64(5) &&
		call.Args["mode"] == "hybrid"
	completionCorrect := sdkRealNormalizedAnswer(result.Text) == task.ExpectedIdentifier
	if !protocolOK {
		t.Fatalf("tool protocol failed: %#v", calls)
	}
	if !completionCorrect {
		t.Fatalf("completion = %q, want %q", result.Text, task.ExpectedIdentifier)
	}
	if rank == nil {
		t.Fatalf("expected path %q absent from metadata: %#v", task.ExpectedPath, call.Metadata)
	}
	algorithm, _ := call.Metadata["algorithm"].(string)
	rankRequested := sdkRealNestedString(call.Metadata, "rerank", "requestedMode")
	rankApplied := sdkRealNestedString(call.Metadata, "rerank", "appliedMode")
	if algorithm != fixture.Rerank.Algorithm ||
		rankRequested != fixture.Rerank.RequestedMode ||
		rankApplied != fixture.Rerank.RequestedMode {
		t.Fatalf("unexpected rerank evidence: %#v", call.Metadata)
	}
	providerMetric := counters.snapshot()
	if providerMetric.QueryRequests != 1 || providerMetric.QueryInputs != 1 {
		t.Fatalf("query provider metric = %#v", providerMetric)
	}
	closeStarted := time.Now()
	if err := session.Close(ctx); err != nil {
		t.Fatal(err)
	}
	closed = true
	closeMS := sdkRealElapsedMS(closeStarted)
	closedStatus, err := session.WorkspaceRetrievalStatus(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if closedStatus.Phase != WorkspaceRetrievalClosed ||
		closedStatus.VectorRecords != 0 || closedStatus.VectorBytes != 0 {
		t.Fatalf("closed retrieval status = %#v", closedStatus)
	}
	return sdkRealRunMetric{
		Task:                  task.Name,
		CompletionCorrect:     completionCorrect,
		ToolProtocolOK:        protocolOK,
		ExpectedPathRank:      rank,
		ResultCount:           resultCount,
		Algorithm:             algorithm,
		RerankRequestedMode:   rankRequested,
		RerankAppliedMode:     rankApplied,
		SessionConstructionMS: sessionConstructionMS,
		IndexReadyMS:          indexReadyMS,
		TurnElapsedMS:         turnElapsedMS,
		CloseMS:               closeMS,
		PromptTokens:          result.Usage.PromptTokens,
		CompletionTokens:      result.Usage.CompletionTokens,
		TotalTokens:           result.Usage.TotalTokens,
		Phase:                 status.Phase,
		CoverageBPS:           status.CoverageBPS,
		EligibleFiles:         status.EligibleFiles,
		IndexedFiles:          status.IndexedFiles,
		IndexedChunks:         status.IndexedChunks,
		VectorRecords:         status.VectorRecords,
		VectorBytes:           status.VectorBytes,
		Batching: sdkRealBatchMetric{
			DocumentInputs:            status.Batching.DocumentInputs,
			DocumentTextBytes:         status.Batching.DocumentTextBytes,
			DocumentBatches:           status.Batching.DocumentBatches,
			DocumentProviderRequests:  status.Batching.DocumentProviderRequests,
			BatchLimitLowerBound:      status.Batching.BatchLimitLowerBound,
			InputLimitFlushes:         status.Batching.InputLimitFlushes,
			TextByteLimitFlushes:      status.Batching.TextByteLimitFlushes,
			VectorByteLimitFlushes:    status.Batching.VectorByteLimitFlushes,
			GenerationCompleteFlushes: status.Batching.GenerationCompleteFlushes,
			TimeToFirstReadyMS:        status.Batching.TimeToFirstReadyMS,
			NonTextInputs:             status.Batching.NonTextInputs,
		},
		Provider:           providerMetric,
		ReleasedAfterClose: true,
	}
}

func waitForSDKRealReady(
	t *testing.T,
	ctx context.Context,
	session *Session,
) WorkspaceRetrievalStatus {
	t.Helper()
	deadline := time.NewTimer(10 * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	for {
		status, err := session.WorkspaceRetrievalStatus(ctx)
		if err != nil {
			t.Fatal(err)
		}
		switch status.Phase {
		case WorkspaceRetrievalReady:
			return status
		case WorkspaceRetrievalBuilding:
		case WorkspaceRetrievalDegraded, WorkspaceRetrievalClosed, WorkspaceRetrievalDisabled:
			t.Fatalf("retrieval entered %s: %#v", status.Phase, status)
		}
		select {
		case <-ctx.Done():
			t.Fatal(ctx.Err())
		case <-deadline.C:
			t.Fatal("workspace retrieval did not become ready")
		case <-ticker.C:
		}
	}
}

func assertSDKRealReadyStatus(
	t *testing.T,
	fixture sdkRealFixture,
	status WorkspaceRetrievalStatus,
	provider sdkRealProviderMetric,
) {
	t.Helper()
	if status.CoverageBPS != 10_000 ||
		status.EligibleFiles != fixture.Corpus.TextFileCount ||
		status.IndexedFiles != fixture.Corpus.TextFileCount ||
		status.IndexedChunks != fixture.Corpus.ExpectedChunkCount ||
		status.FailedFiles != 0 ||
		status.VectorRecords != fixture.Corpus.ExpectedChunkCount {
		t.Fatalf("unexpected ready status: %#v", status)
	}
	batching := status.Batching
	if batching.DocumentInputs != fixture.Corpus.ExpectedChunkCount ||
		batching.DocumentProviderRequests != 1 || batching.BatchLimitLowerBound != 1 ||
		batching.NonTextInputs != 0 || batching.TimeToFirstReadyMS == nil {
		t.Fatalf("unexpected batching status: %#v", batching)
	}
	if provider.DocumentRequests != 1 ||
		provider.DocumentInputs != fixture.Corpus.ExpectedChunkCount ||
		provider.NonTextInputs != 0 {
		t.Fatalf("unexpected provider metric: %#v", provider)
	}
}

func sdkRealTaskPrompt(task sdkRealFixtureTask) string {
	return fmt.Sprintf(
		"Inspect the search tool schema. Make exactly one search call and no other tool call. Use query exactly: %s. Set path to '.', include to '*.rs', limit to 5, and mode to 'hybrid'. After the result, return exactly the Rust function or constant declaration name that directly answers the query and is supported by the evidence, or NOT_FOUND when no relevant declaration is present. Never return a path, file stem, module name, prose, or Markdown.",
		task.Query,
	)
}

func sdkRealNormalizedAnswer(value string) string {
	return strings.TrimSpace(strings.Trim(strings.TrimSpace(value), "`"))
}

func sdkRealElapsedMS(started time.Time) uint64 {
	return uint64(time.Since(started).Milliseconds())
}
