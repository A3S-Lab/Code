//go:build release_contract

package code

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func releaseAgent(t *testing.T, config string) (*Agent, context.Context) {
	t.Helper()
	binary := os.Getenv("A3S_CODE_GO_BRIDGE_TEST_BINARY")
	if binary == "" {
		t.Fatal("A3S_CODE_GO_BRIDGE_TEST_BINARY is required for release-contract integration")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	t.Cleanup(cancel)
	agent, err := Create(ctx, config, WithLocalRuntimeOptions(WithBridgePath(binary)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := agent.Close(context.Background()); err != nil {
			t.Error(err)
		}
	})
	return agent, ctx
}

func TestReleaseSamplingBridge(t *testing.T) {
	const config = `default_model = "openai/fixture"
providers "openai" {
  api_key = "fixture-unused"
  models "fixture" { name = "Fixture" }
}`
	for _, name := range []string{"unset", "async", "error", "panic", "nil", "timeout", "ceiling"} {
		t.Run(name, func(t *testing.T) {
			agent, ctx := releaseAgent(t, config)
			armed, err := agent.HasSamplingHandlerFactory(ctx)
			if err != nil || armed {
				t.Fatalf("default factory: %v %v", armed, err)
			}
			var calls atomic.Int64
			if name != "unset" {
				timeout := time.Second
				if name == "timeout" {
					timeout = 30 * time.Millisecond
				}
				err = agent.SetSamplingHandlerFactory(ctx, func(ctx context.Context, server string, params json.RawMessage) (map[string]any, error) {
					calls.Add(1)
					if server != name {
						return nil, fmt.Errorf("wrong server %s", server)
					}
					switch name {
					case "error":
						return nil, fmt.Errorf("fixture refusal")
					case "panic":
						panic("fixture panic")
					case "nil":
						return nil, nil
					case "timeout":
						<-ctx.Done()
						return nil, ctx.Err()
					}
					return map[string]any{"role": "assistant", "model": "fixture", "content": map[string]any{"type": "text", "text": "host-answer"}}, nil
				}, timeout)
				if err != nil {
					t.Fatal(err)
				}
			}
			marker := filepath.Join(t.TempDir(), "replies.json")
			peer, err := filepath.Abs("../fixtures/mcp_sampling_server.py")
			if err != nil {
				t.Fatal(err)
			}
			python := os.Getenv("A3S_TEST_PYTHON")
			if python == "" {
				python, err = exec.LookPath("python3")
				if err != nil {
					python, err = exec.LookPath("python")
				}
				if err != nil {
					t.Fatal("Python is required for the controlled MCP protocol fixture")
				}
			}
			count := "1"
			if name == "ceiling" {
				count = "65"
			}
			if err = agent.SyncGlobalMCPServers(ctx, []MCPServerConfig{{Name: name, Transport: MCPTransport{Type: "stdio", Command: python, Args: []string{peer, marker, count}}}}); err != nil {
				t.Fatal(err)
			}
			configs, err := agent.GlobalMCPConfigs(ctx)
			if err != nil || len(configs) != 1 || configs[0].Name != name {
				t.Fatalf("configs: %#v %v", configs, err)
			}
			data, err := os.ReadFile(marker)
			if err != nil {
				t.Fatal(err)
			}
			var replies []map[string]json.RawMessage
			if err = json.Unmarshal(data, &replies); err != nil {
				t.Fatal(err)
			}
			if name == "async" || name == "ceiling" {
				if replies[0]["result"] == nil {
					t.Fatalf("sampling failed: %s", data)
				}
				expected := 1
				if name == "ceiling" {
					expected = 64
					if replies[64]["error"] == nil {
						t.Fatal("ceiling bypassed")
					}
				}
				if calls.Load() != int64(expected) {
					t.Fatalf("calls %d expected %d", calls.Load(), expected)
				}
			} else if replies[0]["error"] == nil {
				t.Fatalf("sampling must refuse: %s", data)
			}
		})
	}
}

func TestReleaseWorkspaceReadOverlay(t *testing.T) {
	const config = `default_model = "openai/fixture"
providers "openai" {
  api_key = "fixture-unused"
  models "fixture" { name = "Fixture" }
}`
	agent, ctx := releaseAgent(t, config)
	root := t.TempDir()
	if _, err := agent.Session(ctx, root, &SessionOptions{WorkspaceReadOverlay: map[string]string{"../outside.txt": "refuse"}}); err == nil {
		t.Fatal("escaping overlay path must be refused")
	}
	for name, text := range map[string]string{"buffer.txt": "saved\n", "disk.txt": "disk\n"} {
		if err := os.WriteFile(filepath.Join(root, name), []byte(text), 0600); err != nil {
			t.Fatal(err)
		}
	}
	session, err := agent.Session(ctx, root, &SessionOptions{WorkspaceReadOverlay: map[string]string{"./buffer.txt": "unsaved\n"}, PermissionPolicy: &PermissionPolicy{DefaultDecision: "allow"}})
	if err != nil {
		t.Fatal(err)
	}
	defer session.Close(ctx)
	for name, want := range map[string]string{"buffer.txt": "unsaved", "disk.txt": "disk"} {
		text, err := session.ReadFile(ctx, name, nil)
		if err != nil || !strings.Contains(text, want) {
			t.Fatalf("%s: %q %v", name, text, err)
		}
	}
	text, err := os.ReadFile(filepath.Join(root, "buffer.txt"))
	if err != nil || string(text) != "saved\n" {
		t.Fatalf("overlay mutated backend: %q %v", text, err)
	}
}

func TestReleaseOriginalRunStream(t *testing.T) {
	var requests atomic.Int64
	releasePartial := make(chan struct{})
	blockingResponse := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		call := requests.Add(1)
		w.Header().Set("Content-Type", "text/event-stream")
		time.Sleep(40 * time.Millisecond)
		fmt.Fprint(w, "data: {\"id\":\"fixture\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"fixture-answer\"},\"finish_reason\":null}]}\n\n")
		if call == 3 || call == 5 {
			w.(http.Flusher).Flush()
			if call == 5 {
				close(blockingResponse)
			}
			select {
			case <-r.Context().Done():
			case <-releasePartial:
			}
			return
		}
		fmt.Fprint(w, "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":3,\"total_tokens\":13}}\n\ndata: [DONE]\n\n")
	}))
	defer func() {
		close(releasePartial)
		server.Close()
	}()
	agent, ctx := releaseAgent(t, fmt.Sprintf(`default_model = "openai/fixture"
providers "openai" {
  api_key = "fixture-unused"
  base_url = "%s/v1"
  models "fixture" { name = "Fixture" }
}
memory { llm_extraction = false }`, server.URL))
	continuation, delegation := false, false
	zero, budget := uint(0), uint(4096)
	for _, options := range []*SessionOptions{
		{SandboxNetwork: "invalid"},
		{PathInjectionBudget: &zero},
		{VerificationContracts: []VerificationContract{{ID: "", Command: ""}}},
	} {
		if _, err := agent.Session(ctx, t.TempDir(), options); err == nil {
			t.Fatal("invalid release option was admitted")
		}
	}
	workspace := t.TempDir()
	session, err := agent.Session(ctx, workspace, &SessionOptions{SessionID: "go-release-stream", PlanningMode: PlanningDisabled, ContinuationEnabled: &continuation, ManualDelegationEnabled: &delegation, PermissionPolicy: &PermissionPolicy{DefaultDecision: "deny"}, WatchSkillDirs: []string{filepath.Join(workspace, "future-skills")}, SandboxNetwork: "deny", PathInjectionBudget: &budget})
	if err != nil {
		t.Fatal(err)
	}
	drain := func(stream *EventStream) {
		for range stream.Events {
		}
		if err := <-stream.Done; err != nil {
			t.Fatal(err)
		}
	}
	local, err := session.StreamWithRun(ctx, "/help", nil)
	if err != nil || local.Run != nil {
		t.Fatalf("local admission: %#v %v", local, err)
	}
	drain(local.Stream)
	first, err := session.StreamWithRun(ctx, "Return fixture-answer without tools.", nil)
	if err != nil || first.Run == nil {
		t.Fatalf("first admission: %v", err)
	}
	drain(first.Stream)
	second, err := session.StreamWithRun(ctx, "Repeat fixture-answer without tools.", nil)
	if err != nil || second.Run == nil {
		t.Fatalf("second admission: %v", err)
	}
	if first.Run.ID == second.Run.ID {
		t.Fatal("Run identity reused")
	}
	if cancelled, err := first.Run.Cancel(ctx); err != nil || cancelled {
		t.Fatalf("old cancellation: %v %v", cancelled, err)
	}
	drain(second.Stream)
	snapshot, err := second.Run.Snapshot(ctx)
	if err != nil || snapshot == nil || snapshot.Status != "completed" {
		t.Fatalf("second snapshot: %#v %v", snapshot, err)
	}
	contextReport, err := session.ContextReport(ctx)
	if err != nil || contextReport.Usage.PromptTokens != 20 || contextReport.Usage.CompletionTokens != 6 || contextReport.ToolResults != 0 || len(contextReport.Heaviest) != 0 {
		t.Fatalf("context report: %#v %v", contextReport, err)
	}
	if err = session.SetPermissionPosture(ctx, "invalid"); err == nil {
		t.Fatal("unknown posture accepted")
	}
	if err = session.SetPermissionPosture(ctx, "plan"); err != nil {
		t.Fatal(err)
	}
	if err = session.SetPermissionPolicy(ctx, PermissionPolicy{DefaultDecision: "deny"}); err != nil {
		t.Fatal(err)
	}
	if _, err = session.ProjectedCapabilityNames(ctx); err != nil {
		t.Fatal(err)
	}
	if removed, err := session.ForkSessionAt(ctx, ^uint64(0)); err != nil || removed != 0 {
		t.Fatalf("noop cut: %d %v", removed, err)
	}
	request := json.RawMessage(fmt.Sprintf(`{"target":{"schema":"a3s.code.execution-target.v1","session_id":%q,"run_id":%q}}`, session.SessionID(), second.Run.ID))
	if evidence, err := session.ReadEvaluationEvidence(ctx, request); err != nil || len(evidence) == 0 {
		t.Fatalf("evidence: %s %v", evidence, err)
	}
	foreign := json.RawMessage(fmt.Sprintf(`{"target":{"schema":"a3s.code.execution-target.v1","session_id":"foreign","run_id":%q}}`, second.Run.ID))
	if _, err := session.ReadEvaluationEvidence(ctx, foreign); err == nil {
		t.Fatal("foreign session evidence was admitted")
	}
	cancelled, err := session.StreamWithRun(ctx, "Cancel this partial fixture response.", nil)
	if err != nil || cancelled.Run == nil {
		t.Fatalf("cancel admission: %v", err)
	}
	accepted, ends, diagnostics := false, 0, 0
	for event := range cancelled.Stream.Events {
		switch event.Type {
		case "text_delta":
			if !accepted {
				accepted, err = cancelled.Run.Cancel(ctx)
				if err != nil || !accepted {
					t.Fatalf("original cancellation: %v %v", accepted, err)
				}
			}
		case "agent_end":
			ends++
		case "error":
			diagnostics++
		}
	}
	if err = <-cancelled.Stream.Done; err != nil {
		t.Fatal(err)
	}
	if !accepted || ends != 0 || diagnostics != 1 {
		t.Fatalf("cancel publication: accepted=%v ends=%d errors=%d", accepted, ends, diagnostics)
	}
	snapshot, err = cancelled.Run.Snapshot(ctx)
	if err != nil || snapshot == nil || snapshot.Status != "cancelled" {
		t.Fatalf("cancel snapshot: %#v %v", snapshot, err)
	}
	continued, err := session.StreamWithRun(ctx, "Continue after cancellation.", nil)
	if err != nil || continued.Run == nil || continued.Run.ID == cancelled.Run.ID {
		t.Fatalf("continued admission: %v", err)
	}
	if accepted, err = cancelled.Run.Cancel(ctx); err != nil || accepted {
		t.Fatalf("old cancelled handle: %v %v", accepted, err)
	}
	drain(continued.Stream)
	snapshot, err = continued.Run.Snapshot(ctx)
	if err != nil || snapshot == nil || snapshot.Status != "completed" || requests.Load() != 4 {
		t.Fatalf("continued snapshot: %#v requests=%d error=%v", snapshot, requests.Load(), err)
	}
	blocking := make(chan error, 1)
	go func() {
		_, err := session.Send(ctx, "Cancel this blocking partial response.", nil)
		blocking <- err
	}()
	select {
	case <-blockingResponse:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	active, err := session.CurrentRun(ctx)
	if err != nil || active == nil || active.ID == "" {
		t.Fatalf("blocking Run: %#v %v", active, err)
	}
	if accepted, err := session.CancelRun(ctx, active.ID); err != nil || !accepted {
		t.Fatalf("blocking cancellation: %v %v", accepted, err)
	}
	select {
	case err := <-blocking:
		if !errors.Is(err, ErrRunCancelled) || !strings.Contains(err.Error(), active.ID) {
			t.Fatalf("blocking cancellation error: %v", err)
		}
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	snapshot, err = session.RunSnapshot(ctx, active.ID)
	if err != nil || snapshot == nil || snapshot.Status != "cancelled" {
		t.Fatalf("blocking cancelled snapshot: %#v %v", snapshot, err)
	}
	afterBlocking, err := session.StreamWithRun(ctx, "Continue after blocking cancellation.", nil)
	if err != nil || afterBlocking.Run == nil || afterBlocking.Run.ID == active.ID {
		t.Fatalf("blocking continued admission: %v", err)
	}
	if accepted, err := session.CancelRun(ctx, active.ID); err != nil || accepted {
		t.Fatalf("old blocking cancellation: %v %v", accepted, err)
	}
	drain(afterBlocking.Stream)
	snapshot, err = afterBlocking.Run.Snapshot(ctx)
	if err != nil || snapshot == nil || snapshot.Status != "completed" || requests.Load() != 6 {
		t.Fatalf("blocking continued snapshot: %#v requests=%d error=%v", snapshot, requests.Load(), err)
	}
}
