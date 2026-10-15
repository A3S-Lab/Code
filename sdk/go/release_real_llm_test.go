//go:build release_real_llm

package code

import (
	"context"
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"
)

// TestReleaseRealLLM explicitly requires a real ACL and a freshly built bridge.
// The opt-in tag keeps provider traffic out of the hermetic release suite.
func TestReleaseRealLLM(t *testing.T) {
	config, binary := os.Getenv("A3S_CONFIG_FILE"), os.Getenv("A3S_CODE_GO_BRIDGE_TEST_BINARY")
	if config == "" || binary == "" {
		t.Fatal("A3S_CONFIG_FILE and A3S_CODE_GO_BRIDGE_TEST_BINARY are required")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 240*time.Second)
	defer cancel()
	agent, err := Create(ctx, config, WithLocalRuntimeOptions(WithBridgePath(binary)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := agent.Close(context.Background()); err != nil {
			t.Error(err)
		}
	})
	no, yes := false, true
	session, err := agent.Session(ctx, t.TempDir(), &SessionOptions{
		SessionID: "go-release-real-913", PlanningMode: PlanningDisabled,
		ManualDelegationEnabled: &no, AutoDelegationEnabled: &no, ContinuationEnabled: &no,
		ReadOnlySession: &yes, SandboxNetwork: "deny",
		PermissionPolicy: &PermissionPolicy{DefaultDecision: "deny"},
	})
	if err != nil {
		t.Fatal(err)
	}
	const marker = "SDK-LIVE-913:GO:42"
	var previous *RunHandle
	var receipts []map[string]any
	for _, prompt := range []string{
		"Remember the verification code " + marker + ". Reply with exactly that code. Do not use tools.",
		"Return only the verification code from my previous message. Do not use tools.",
	} {
		if err := session.SetPermissionPolicy(ctx, PermissionPolicy{DefaultDecision: "deny"}); err != nil {
			t.Fatal(err)
		}
		admitted, err := session.StreamWithRun(ctx, prompt, nil)
		if err != nil {
			t.Fatal(err)
		}
		if admitted.Run == nil || admitted.Run.SessionID != session.ID() {
			t.Fatal("missing original Run identity")
		}
		if previous != nil {
			if previous.ID == admitted.Run.ID {
				t.Fatal("different turns reused a Run")
			}
			cancelled, err := previous.Cancel(ctx)
			if err != nil || cancelled {
				t.Fatalf("old handle cancelled later Run: %v %v", cancelled, err)
			}
		}
		types := map[string]int{}
		for event := range admitted.Stream.Events {
			types[event.Type]++
		}
		if err := <-admitted.Stream.Done; err != nil {
			t.Fatal(err)
		}
		snapshot, err := admitted.Run.Snapshot(ctx)
		if err != nil {
			t.Fatal(err)
		}
		if snapshot.ID != admitted.Run.ID || snapshot.Status != RunCompleted || snapshot.ResultText == nil || strings.TrimSpace(*snapshot.ResultText) != marker {
			t.Fatalf("unexpected original Run snapshot: %#v", snapshot)
		}
		if types["text_delta"] == 0 || types["agent_end"] != 1 || types["tool_execution_start"] != 0 {
			t.Fatalf("unexpected events: %v", types)
		}
		target := map[string]string{"schema": "a3s.code.execution-target.v1", "session_id": session.ID(), "run_id": admitted.Run.ID}
		request, err := json.Marshal(map[string]any{"target": target})
		if err != nil {
			t.Fatal(err)
		}
		evidence, err := session.ReadEvaluationEvidence(ctx, request)
		if err != nil {
			t.Fatal(err)
		}
		var decoded struct {
			Target map[string]string `json:"target"`
		}
		if err := json.Unmarshal(evidence, &decoded); err != nil {
			t.Fatal(err)
		}
		if len(decoded.Target) != len(target) {
			t.Fatal("evidence target shape drift")
		}
		for key, value := range target {
			if decoded.Target[key] != value {
				t.Fatal("evidence target drift")
			}
		}
		foreign, err := json.Marshal(map[string]any{"target": map[string]string{"schema": target["schema"], "session_id": "foreign", "run_id": admitted.Run.ID}})
		if err != nil {
			t.Fatal(err)
		}
		if _, err := session.ReadEvaluationEvidence(ctx, foreign); err == nil || !strings.Contains(strings.ToLower(err.Error()), "not found") {
			t.Fatalf("foreign evidence must refuse: %v", err)
		}
		receipts = append(receipts, map[string]any{"run_id": admitted.Run.ID, "status": snapshot.Status, "result": *snapshot.ResultText, "event_types": types, "evidence_target": decoded.Target})
		previous = admitted.Run
	}
	report, err := json.Marshal(map[string]any{"sdk": "go", "version": "9.1.3", "model": session.ModelName(), "session_id": session.ID(), "receipts": receipts})
	if err != nil {
		t.Fatal(err)
	}
	t.Log("SDK_RELEASE_REAL " + string(report))
}
