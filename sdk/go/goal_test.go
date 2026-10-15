package code

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const goalBridgeConfig = `
	default_model = "anthropic/test-model"
	providers "anthropic" {
		apiKey = "test-key"
		models "test-model" {
			name = "Test Model"
		}
	}
`

type goalView struct {
	Binding struct {
		GoalID       string          `json:"goal_id"`
		SessionID    string          `json:"session_id"`
		Workspace    json.RawMessage `json:"workspace"`
		GoalRevision uint32          `json:"goal_revision"`
		PolicyDigest *string         `json:"work_policy_digest"`
	} `json:"binding"`
	State            string  `json:"state"`
	GoalSequence     uint64  `json:"goal_sequence"`
	PlanRevision     *uint32 `json:"plan_revision"`
	PlanFactSequence *uint64 `json:"plan_fact_sequence"`
	PlanAccepted     bool    `json:"plan_accepted"`
}

func goalCommand(t *testing.T, fields map[string]any) json.RawMessage {
	t.Helper()
	encoded, err := json.Marshal(fields)
	if err != nil {
		t.Fatal(err)
	}
	return encoded
}

func decodeGoal(t *testing.T, raw json.RawMessage) goalView {
	t.Helper()
	var view goalView
	if err := json.Unmarshal(raw, &view); err != nil {
		t.Fatalf("goal view %s: %v", raw, err)
	}
	return view
}

func TestRustBridgeGoalSurface(t *testing.T) {
	binary := os.Getenv("A3S_CODE_GO_BRIDGE_TEST_BINARY")
	if binary == "" {
		t.Skip("set A3S_CODE_GO_BRIDGE_TEST_BINARY to run the Rust bridge integration")
	}
	ctx := context.Background()
	agent, err := Create(ctx, goalBridgeConfig, WithLocalRuntimeOptions(WithBridgePath(binary)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = agent.Close(context.Background()) })

	// Policy omitted and valid policy are both admitted; Core refuses an invalid one.
	if _, err := agent.Session(ctx, t.TempDir(), &SessionOptions{
		WorkPolicy: &WorkPolicy{ID: "", Revision: 1, Instructions: "x"},
	}); err == nil || !strings.Contains(err.Error(), "work_policy") {
		t.Fatalf("invalid work policy error = %v", err)
	}
	for _, policy := range []*WorkPolicy{nil, {ID: "eng", Revision: 1, Instructions: "Make the smallest complete change."}} {
		session, err := agent.Session(ctx, t.TempDir(), &SessionOptions{WorkPolicy: policy})
		if err != nil {
			t.Fatalf("policy %v: %v", policy, err)
		}
		digest, err := session.WorkPolicyDigest(ctx)
		if err != nil || digest != nil {
			t.Fatalf("unadmitted policy digest = %v, %v", digest, err)
		}
		if err := session.Close(ctx); err != nil {
			t.Fatal(err)
		}
	}

	session, err := agent.Session(ctx, t.TempDir(), &SessionOptions{SessionID: "go-goal"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = session.Close(context.Background()) })
	workspace := map[string]string{"id": session.Workspace(), "display_root": session.Workspace()}
	if existing, err := session.GoalDefinition(ctx); err != nil || existing != nil {
		t.Fatalf("empty goal definition = %s, %v", existing, err)
	}
	create := map[string]any{
		"kind": "create", "operation_id": "create", "goal_id": "release",
		"session_id": session.ID(), "workspace": workspace,
		"goal": map[string]any{
			"description": "Report the release code", "success_criteria": []string{"Reported exactly"},
			"progress": 0.0, "achieved": false, "created_at": time.Now().Unix(), "achieved_at": nil,
		},
		"non_goals": []string{"Do not change files"}, "scope_fact_sequences": []uint64{},
		"work_policy_digest": nil, "expected_goal_revision": nil,
		"expected_plan_revision": nil, "expected_goal_sequence": nil,
	}

	// Core's serde boundary refuses unknown fields; nothing is recorded.
	create["unexpected"] = true
	if _, err := session.ApplyGoalDefinition(ctx, goalCommand(t, create)); err == nil ||
		!strings.Contains(err.Error(), "unknown field") {
		t.Fatalf("unknown-field refusal = %v", err)
	}
	delete(create, "unexpected")
	if existing, err := session.GoalDefinition(ctx); err != nil || existing != nil {
		t.Fatalf("refused command recorded a goal: %s, %v", existing, err)
	}

	created := decodeGoal(t, mustRaw(t)(session.ApplyGoalDefinition(ctx, goalCommand(t, create))))
	if created.Binding.GoalID != "release" || created.State != "active" || created.PlanAccepted {
		t.Fatalf("created goal = %+v", created)
	}
	ref := func(kind, operation string, view goalView) map[string]any {
		return map[string]any{
			"kind": kind, "operation_id": operation, "goal_id": view.Binding.GoalID,
			"session_id": view.Binding.SessionID, "workspace": view.Binding.Workspace,
			"work_policy_digest": nil, "expected_goal_revision": view.Binding.GoalRevision,
			"expected_plan_revision": view.PlanRevision, "expected_goal_sequence": view.GoalSequence,
		}
	}
	propose := ref("propose_plan", "propose", created)
	propose["plan"] = map[string]any{
		"goal": "Release check", "complexity": "Simple", "required_tools": []string{}, "estimated_steps": 1,
		"steps": []map[string]any{{
			"id": "report", "content": "Report the release code", "status": "pending",
			"priority": "medium", "success_criteria": "Reported exactly",
		}},
	}
	propose["proposal_sequence"] = nil
	proposed := decodeGoal(t, mustRaw(t)(session.ApplyGoalPlan(ctx, goalCommand(t, propose))))
	accept := ref("accept_plan", "accept", proposed)
	accept["plan"] = nil
	accept["proposal_sequence"] = proposed.PlanFactSequence
	accepted := decodeGoal(t, mustRaw(t)(session.ApplyGoalPlan(ctx, goalCommand(t, accept))))
	if !accepted.PlanAccepted {
		t.Fatalf("plan not accepted: %+v", accepted)
	}

	// Without trusted host goal_control bounds a ready step never advances.
	tick := goalCommand(t, ref("tick", "tick-1", accepted))
	decision, err := session.PreviewGoalTick(ctx, tick)
	if err != nil || decision != "budget_exhausted" {
		t.Fatalf("preview = %q, %v", decision, err)
	}
	ticked, err := session.TickGoal(ctx, tick)
	if err != nil || ticked.Run != nil || ticked.Decision != "budget_exhausted" {
		t.Fatalf("tick = %+v, %v", ticked, err)
	}
	streamed, err := session.StreamGoalTick(ctx, tick)
	if err != nil || streamed.Started != nil || streamed.Replayed != nil || streamed.Decision != "budget_exhausted" {
		t.Fatalf("streamed tick = %+v, %v", streamed, err)
	}

	paused := decodeGoal(t, mustRaw(t)(session.ApplyGoalControl(ctx, goalCommand(t, ref("pause", "pause", accepted)))))
	if paused.State != "paused" {
		t.Fatalf("paused goal = %+v", paused)
	}
	// A stale reference is refused with Core's goal refusal, not reinterpreted.
	if _, err := session.ApplyGoalControl(ctx, goalCommand(t, ref("resume", "resume", accepted))); err == nil ||
		!strings.Contains(err.Error(), "goal.") {
		t.Fatalf("stale control refusal = %v", err)
	}
	if appended, err := session.ReconcileGoalSlices(ctx); err != nil || appended != 0 {
		t.Fatalf("reconcile = %d, %v", appended, err)
	}

	// Fork a saved SessionSnapshotV1; Core rejects a malformed one.
	store := t.TempDir()
	source, err := agent.Session(ctx, t.TempDir(), &SessionOptions{SessionID: "go-fork-src", FileSessionStoreDir: store})
	if err != nil {
		t.Fatal(err)
	}
	if err := source.Save(ctx); err != nil {
		t.Fatal(err)
	}
	snapshots, err := filepath.Glob(filepath.Join(store, "v1", "sessions", "*.json"))
	if err != nil || len(snapshots) != 1 {
		t.Fatalf("saved snapshots = %v, %v", snapshots, err)
	}
	snapshot, err := os.ReadFile(snapshots[0])
	if err != nil {
		t.Fatal(err)
	}
	forked, err := agent.ForkSession(ctx, snapshot, t.TempDir(), &SessionOptions{SessionID: "go-fork"})
	if err != nil || forked.ID() != "go-fork" {
		t.Fatalf("fork = %v, %v", forked, err)
	}
	if definition, err := forked.GoalDefinition(ctx); err != nil || definition != nil {
		t.Fatalf("forked goal = %s, %v", definition, err)
	}
	if _, err := agent.ForkSession(ctx, json.RawMessage(`{}`), t.TempDir(), nil); err == nil ||
		!strings.Contains(err.Error(), "schema_version") {
		t.Fatalf("malformed snapshot refusal = %v", err)
	}
}

func mustRaw(t *testing.T) func(json.RawMessage, error) json.RawMessage {
	return func(raw json.RawMessage, err error) json.RawMessage {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
		return raw
	}
}
