//go:build release_real_llm

package code

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"testing"
	"time"
)

// TestGoalRealLLM proves the four work-policy x goal modes with one real Run
// per session through the freshly built bridge.
func TestGoalRealLLM(t *testing.T) {
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
	t.Cleanup(func() { _ = agent.Close(context.Background()) })
	no := false
	options := func(id string, policy *WorkPolicy) *SessionOptions {
		return &SessionOptions{
			SessionID: id, PlanningMode: PlanningDisabled,
			ManualDelegationEnabled: &no, AutoDelegationEnabled: &no, ContinuationEnabled: &no,
			SandboxNetwork: "deny", PermissionPolicy: &PermissionPolicy{DefaultDecision: "deny"},
			WorkPolicy: policy,
		}
	}
	create := func(session *Session, digest *string) json.RawMessage {
		return goalCommand(t, map[string]any{
			"kind": "create", "operation_id": "create", "goal_id": "release",
			"session_id": session.ID(),
			"workspace":  map[string]string{"id": session.Workspace(), "display_root": session.Workspace()},
			"goal": map[string]any{
				"description": "Report the release code", "success_criteria": []string{"Reported exactly"},
				"progress": 0.0, "achieved": false, "created_at": time.Now().Unix(), "achieved_at": nil,
			},
			"non_goals": []string{}, "scope_fact_sequences": []uint64{}, "work_policy_digest": digest,
			"expected_goal_revision": nil, "expected_plan_revision": nil, "expected_goal_sequence": nil,
		})
	}
	const prompt = "Reply with exactly the word READY. Do not use tools."
	started := time.Now()

	plain, err := agent.Session(ctx, t.TempDir(), options("go-goal-live-plain", nil))
	if err != nil {
		t.Fatal(err)
	}
	if result, err := plain.Send(ctx, prompt, nil); err != nil || !strings.Contains(result.Text, "READY") {
		t.Fatalf("plain run = %+v, %v", result, err)
	}
	if digest, err := plain.WorkPolicyDigest(ctx); err != nil || digest != nil {
		t.Fatalf("no policy digest = %v, %v", digest, err)
	}
	if view := decodeGoal(t, mustRaw(t)(plain.ApplyGoalDefinition(ctx, create(plain, nil)))); view.Binding.PolicyDigest != nil {
		t.Fatalf("plain goal binding = %+v", view)
	}

	policy := &WorkPolicy{ID: "eng", Revision: 1, Instructions: "Answer exactly what is asked and change nothing else."}
	governed, err := agent.Session(ctx, t.TempDir(), options("go-goal-live-policy", policy))
	if err != nil {
		t.Fatal(err)
	}
	if digest, err := governed.WorkPolicyDigest(ctx); err != nil || digest != nil {
		t.Fatalf("unadmitted digest = %v, %v", digest, err)
	}
	result, err := governed.Send(ctx, prompt, nil)
	if err != nil || !strings.Contains(result.Text, "READY") {
		t.Fatalf("governed run = %+v, %v", result, err)
	}
	digest, err := governed.WorkPolicyDigest(ctx)
	if err != nil || digest == nil || *digest == "" {
		t.Fatalf("admitted digest = %v, %v", digest, err)
	}
	if definition, err := governed.GoalDefinition(ctx); err != nil || definition != nil {
		t.Fatalf("policy without goal = %s, %v", definition, err)
	}
	if _, err := governed.ApplyGoalDefinition(ctx, create(governed, nil)); err == nil || !strings.Contains(err.Error(), "goal.") {
		t.Fatalf("missing digest refusal = %v", err)
	}
	bound := decodeGoal(t, mustRaw(t)(governed.ApplyGoalDefinition(ctx, create(governed, digest))))
	if bound.Binding.PolicyDigest == nil || *bound.Binding.PolicyDigest != *digest {
		t.Fatalf("bound digest = %v, want %s", bound.Binding.PolicyDigest, *digest)
	}
	// Trusted host bounds admit one goal slice; its events arrive as a stream.
	source, err := os.ReadFile(config)
	if err != nil {
		t.Fatal(err)
	}
	bounded, err := Create(ctx, fmt.Sprintf(
		"%s\ngoal_control {\n  automatic_enabled = true\n  max_slices = \"1\"\n  max_no_progress_slices = \"1\"\n  deadline_unix_ms = \"%d\"\n}\n",
		source, time.Now().Add(10*time.Minute).UnixMilli(),
	), WithLocalRuntimeOptions(WithBridgePath(binary)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = bounded.Close(context.Background()) })
	yes := true
	sliceOptions := options("go-goal-live-slice", nil)
	sliceOptions.PlanningMode, sliceOptions.GoalTracking = PlanningEnabled, &yes
	slice, err := bounded.Session(ctx, t.TempDir(), sliceOptions)
	if err != nil {
		t.Fatal(err)
	}
	ref := func(kind, operation string, view goalView) map[string]any {
		return map[string]any{
			"kind": kind, "operation_id": operation, "goal_id": view.Binding.GoalID,
			"session_id": view.Binding.SessionID, "workspace": view.Binding.Workspace,
			"work_policy_digest": nil, "expected_goal_revision": view.Binding.GoalRevision,
			"expected_plan_revision": view.PlanRevision, "expected_goal_sequence": view.GoalSequence,
		}
	}
	goal := decodeGoal(t, mustRaw(t)(slice.ApplyGoalDefinition(ctx, create(slice, nil))))
	propose := ref("propose_plan", "propose", goal)
	propose["proposal_sequence"] = nil
	propose["plan"] = map[string]any{
		"goal": "Release check", "complexity": "Simple", "required_tools": []string{}, "estimated_steps": 1,
		"steps": []map[string]any{{
			"id": "report", "content": prompt, "status": "pending",
			"priority": "medium", "success_criteria": "Replied READY",
		}},
	}
	proposed := decodeGoal(t, mustRaw(t)(slice.ApplyGoalPlan(ctx, goalCommand(t, propose))))
	accept := ref("accept_plan", "accept", proposed)
	accept["plan"] = nil
	accept["proposal_sequence"] = proposed.PlanFactSequence
	accepted := decodeGoal(t, mustRaw(t)(slice.ApplyGoalPlan(ctx, goalCommand(t, accept))))
	tick := goalCommand(t, ref("tick", "tick-1", accepted))
	streamed, err := slice.StreamGoalTick(ctx, tick)
	if err != nil || streamed.Started == nil || streamed.Started.Run == nil || streamed.Decision != "" {
		t.Fatalf("streamed tick = %+v, %v", streamed, err)
	}
	events := 0
	for range streamed.Started.Stream.Events {
		events++
	}
	snapshot, err := streamed.Started.Run.Snapshot(ctx)
	if err != nil || snapshot.ID != "tick-1" || snapshot.Status == "running" || events == 0 {
		t.Fatalf("slice snapshot = %+v, events %d, %v", snapshot, events, err)
	}
	again, err := slice.StreamGoalTick(ctx, tick)
	if err != nil || again.Started != nil {
		t.Fatalf("a repeated operation starts nothing: %+v, %v", again, err)
	}
	fmt.Fprintf(os.Stderr, "{\"sdk\":\"go\",\"digest\":%q,\"tokens\":%d,\"slice\":%q,\"events\":%d,\"again\":%q,\"ms\":%d}\n",
		*digest, result.Usage.TotalTokens, snapshot.Status, events, again.Decision, time.Since(started).Milliseconds())
}
