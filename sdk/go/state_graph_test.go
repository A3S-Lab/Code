package code

import (
	"context"
	"encoding/json"
	"errors"
	"reflect"
	"testing"
)

func TestStateGraphRuntimeUsesCoreOperations(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(_ context.Context, operation string, params map[string]any) (any, error) {
			switch operation {
			case "state_graph_create":
				return map[string]any{"graph_handle": "graph-1"}, nil
			case "state_graph_info":
				return StateGraphInfo{BranchID: "branch-1", Version: 1, EventCount: 3}, nil
			case "state_graph_propose_patch":
				return map[string]any{"applied": true}, nil
			case "state_graph_run_goal", "state_graph_emit_custom":
				return GraphEventRecord{ID: "event-1", Event: json.RawMessage(`{"type":"custom"}`)}, nil
			case "state_graph_graph":
				return GraphProjection{Version: 1, Objects: map[string]GraphObject{}}, nil
			case "state_graph_events":
				return []GraphEventRecord{}, nil
			case "state_graph_fork":
				return map[string]any{"graph_handle": "graph-2"}, nil
			case "state_graph_diff":
				return GraphDiff{}, nil
			case "state_graph_close":
				return nil, nil
			default:
				t.Fatalf("unexpected operation %q with params %#v", operation, params)
				return nil, nil
			}
		},
	}
	graph, err := NewStateGraphRuntime(context.Background(), WithStateGraphRuntime(runtime))
	if err != nil {
		t.Fatal(err)
	}
	if got, err := graph.BranchID(context.Background()); err != nil || got != "branch-1" {
		t.Fatalf("BranchID = %q, %v", got, err)
	}
	if got, err := graph.Version(context.Background()); err != nil || got != 1 {
		t.Fatalf("Version = %d, %v", got, err)
	}
	applied, err := graph.ProposePatch(context.Background(), GraphPatch{
		ExpectedGraphVersion: 0,
		Operations:           []PatchOperation{AddObjectOperation("task-1", "task", map[string]any{"open": true})},
	})
	if err != nil || !applied {
		t.Fatalf("ProposePatch = %v, %v", applied, err)
	}
	if _, err := graph.RunGoal(context.Background(), "finish"); err != nil {
		t.Fatal(err)
	}
	fork, err := graph.ForkAt(context.Background(), 3)
	if err != nil || fork == nil {
		t.Fatalf("ForkAt = %#v, %v", fork, err)
	}
	if _, err := graph.Diff(context.Background(), fork); err != nil {
		t.Fatal(err)
	}
	if err := graph.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	ops := runtime.operations()
	want := []string{
		"sdk_capabilities",
		"state_graph_create",
		"state_graph_info",
		"state_graph_info",
		"state_graph_propose_patch",
		"state_graph_run_goal",
		"state_graph_fork",
		"state_graph_diff",
		"state_graph_close",
	}
	if !reflect.DeepEqual(ops, want) {
		t.Fatalf("operations = %v, want %v", ops, want)
	}
}

func TestStateGraphTransportKeepsStandaloneBridgeUntilLastForkCloses(t *testing.T) {
	runtime := &fakeRuntime{}
	transport := newStateGraphRuntimeTransport(runtime, true)
	if !transport.acquire() {
		t.Fatal("acquire unexpectedly failed")
	}
	if err := transport.release(); err != nil {
		t.Fatal(err)
	}
	if runtime.closed {
		t.Fatal("bridge closed while a graph fork still held a reference")
	}
	if err := transport.release(); err != nil {
		t.Fatal(err)
	}
	if !runtime.closed {
		t.Fatal("last graph reference must close the standalone bridge")
	}
	if transport.acquire() {
		t.Fatal("acquire must fail after the final reference is released")
	}
}

func TestStateGraphCloseDoesNotCloseCallerOwnedRuntime(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(_ context.Context, operation string, _ map[string]any) (any, error) {
			if operation == "state_graph_close" {
				return nil, nil
			}
			return map[string]any{"graph_handle": "graph-1"}, nil
		},
	}
	graph, err := NewStateGraphRuntime(context.Background(), WithStateGraphRuntime(runtime))
	if err != nil {
		t.Fatal(err)
	}
	if err := graph.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	if runtime.closed {
		t.Fatal("caller-owned runtime must remain open after graph close")
	}
	if _, err := graph.Info(context.Background()); !errors.Is(err, ErrStateGraphClosed) {
		t.Fatalf("Info after close = %v, want ErrStateGraphClosed", err)
	}
}

func TestStateGraphExternalProjectionUsesTypedOperations(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(_ context.Context, operation string, _ map[string]any) (any, error) {
			switch operation {
			case "state_graph_create":
				return map[string]any{"graph_handle": "graph-1"}, nil
			case "state_graph_check_external":
				return map[string]any{"outcome": nil}, nil
			case "state_graph_project_external":
				return map[string]any{"outcome": "applied"}, nil
			case "state_graph_close":
				return nil, nil
			default:
				return map[string]any{}, nil
			}
		},
	}
	graph, err := NewStateGraphRuntime(context.Background(), WithStateGraphRuntime(runtime))
	if err != nil {
		t.Fatal(err)
	}
	event := ExternalGraphEvent{Source: "queue", StreamID: "orders", Sequence: 1, EventID: "e1", Name: "created", Payload: map[string]any{"id": "o1"}}
	if outcome, err := graph.CheckExternal(context.Background(), event); err != nil || outcome != nil {
		t.Fatalf("CheckExternal = %#v, %v", outcome, err)
	}
	outcome, err := graph.ProjectExternal(context.Background(), event, GraphPatch{Operations: []PatchOperation{}})
	if err != nil || outcome != ExternalProjectionApplied {
		t.Fatalf("ProjectExternal = %q, %v", outcome, err)
	}
	if err := graph.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	if got := runtime.operations(); !reflect.DeepEqual(got, []string{
		"sdk_capabilities", "state_graph_create", "state_graph_check_external",
		"state_graph_project_external", "state_graph_close",
	}) {
		t.Fatalf("operations = %v", got)
	}
}
