package code

import (
	"context"
	"encoding/json"
	"strings"
	"time"
)

// WorkPolicy is an engineering work policy; Core validates it and the
// session's first Run admits it.
type WorkPolicy struct {
	ID           string `json:"id"`
	Revision     uint32 `json:"revision"`
	Instructions string `json:"instructions"`
}

// GoalTickResult is a non-advancing Decision or the one Run a tick admitted.
type GoalTickResult struct {
	Decision string    `json:"decision,omitempty"`
	Run      *RunSpawn `json:"run,omitempty"`
}

// ForkSession forks a saved SessionSnapshotV1 into a new session with fresh
// host options. An admitted source work policy must be supplied unchanged;
// source Goal facts stay at the source.
func (agent *Agent) ForkSession(
	ctx context.Context,
	snapshot json.RawMessage,
	workspace string,
	options *SessionOptions,
) (*Session, error) {
	const op = "agent_fork_session"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(workspace) == "" {
		return nil, invalid(op, "workspace cannot be empty")
	}
	params := map[string]any{
		"agent_id":  agent.id,
		"snapshot":  snapshot,
		"workspace": workspace,
	}
	if options != nil {
		params["options"] = options
	}
	return agent.newSession(ctx, op, params)
}

func (session *Session) goalCommand(ctx context.Context, op string, command json.RawMessage) (json.RawMessage, error) {
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	params := session.params()
	params["command"] = command
	return session.requestRaw(ctx, op, params)
}

// ApplyGoalDefinition admits a create/revise GoalDefinitionCommandV1; it starts no Run.
func (session *Session) ApplyGoalDefinition(ctx context.Context, command json.RawMessage) (json.RawMessage, error) {
	return session.goalCommand(ctx, "session_apply_goal_definition", command)
}

// ApplyGoalPlan proposes or explicitly accepts a plan (GoalPlanCommandV1).
func (session *Session) ApplyGoalPlan(ctx context.Context, command json.RawMessage) (json.RawMessage, error) {
	return session.goalCommand(ctx, "session_apply_goal_plan", command)
}

// ApplyGoalControl records pause/resume/cancel (GoalControlCommandV1) on an idle session.
func (session *Session) ApplyGoalControl(ctx context.Context, command json.RawMessage) (json.RawMessage, error) {
	return session.goalCommand(ctx, "session_apply_goal_control", command)
}

// ControlGoal pauses or cancels even while a goal slice runs: the original
// RunControl settles it first, then the control is recorded.
func (session *Session) ControlGoal(ctx context.Context, command json.RawMessage, grace time.Duration) (json.RawMessage, error) {
	const op = "session_control_goal"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if grace < 0 {
		return nil, invalid(op, "grace cannot be negative")
	}
	params := session.params()
	params["command"] = command
	params["grace_ms"] = grace.Milliseconds()
	return session.requestRaw(ctx, op, params)
}

// ApplyGoalWait declares or observes a step wait; returns {recorded, definition}.
func (session *Session) ApplyGoalWait(ctx context.Context, command json.RawMessage) (json.RawMessage, error) {
	return session.goalCommand(ctx, "session_apply_goal_wait", command)
}

// GoalDefinition returns the bound Goal read from its original facts, or nil.
func (session *Session) GoalDefinition(ctx context.Context) (json.RawMessage, error) {
	const op = "session_goal_definition"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Definition json.RawMessage `json:"definition"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	if string(result.Definition) == "null" {
		return nil, nil
	}
	return result.Definition, nil
}

// WorkPolicyDigest returns the digest of the work policy a Run admitted, or nil.
func (session *Session) WorkPolicyDigest(ctx context.Context) (*string, error) {
	const op = "session_work_policy_digest"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Digest *string `json:"digest"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Digest, err
}

// CheckGoalStepRun refuses unless the step and Run are the bound Goal's current ones.
func (session *Session) CheckGoalStepRun(
	ctx context.Context,
	goalID string,
	goalRevision uint32,
	planRevision uint32,
	stepID string,
	runID string,
) error {
	const op = "session_check_goal_step_run"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	params := session.params()
	params["goal_id"] = goalID
	params["goal_revision"] = goalRevision
	params["plan_revision"] = planRevision
	params["step_id"] = stepID
	params["run_id"] = runID
	return session.runtime.Request(ctx, op, params, nil)
}

// ReconcileGoalSlices closes stopped goal slices without replay and returns
// how many facts were appended.
func (session *Session) ReconcileGoalSlices(ctx context.Context) (uint, error) {
	const op = "session_reconcile_goal_slices"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	var result struct {
		Appended uint `json:"appended"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Appended, err
}

// PreviewGoalTick inspects a tick decision without reserving or running anything.
func (session *Session) PreviewGoalTick(ctx context.Context, command json.RawMessage) (string, error) {
	const op = "session_preview_goal_tick"
	if err := validateSession(session, ctx, op); err != nil {
		return "", err
	}
	params := session.params()
	params["command"] = command
	var result struct {
		Decision string `json:"decision"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Decision, err
}

// TickGoal admits at most one original Run for an automatic tick.
func (session *Session) TickGoal(ctx context.Context, command json.RawMessage) (*GoalTickResult, error) {
	const op = "session_tick_goal"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	params := session.params()
	params["command"] = command
	var result GoalTickResult
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// GoalTickStream is a TickGoal for an interactive host. Exactly one field is set:
// the decision, the started slice's stream, or the already admitted Run's snapshot.
type GoalTickStream struct {
	Decision string
	Started  *AgentRunStream
	Replayed *RunSnapshot
}

// StreamGoalTick admits at most one original Run and streams its events.
func (session *Session) StreamGoalTick(ctx context.Context, command json.RawMessage) (*GoalTickStream, error) {
	const op = "session_stream_goal_tick"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	params := session.params()
	params["command"] = command
	stream, err := session.runtime.Stream(ctx, op, params)
	if err != nil {
		return nil, err
	}
	if stream.Admission == nil {
		return nil, sdkError(op, CodeProtocol, "runtime omitted stream admission", nil)
	}
	select {
	case admission := <-stream.Admission:
		if admission.Err != nil {
			return nil, admission.Err
		}
		switch {
		case admission.Decision != "":
			return &GoalTickStream{Decision: admission.Decision}, nil
		case admission.Replayed != nil:
			return &GoalTickStream{Replayed: admission.Replayed}, nil
		case admission.RunID == "" || admission.SessionID != session.ID():
			return nil, sdkError(op, CodeProtocol, "stream session identity mismatch", nil)
		}
		run := &RunHandle{ID: admission.RunID, SessionID: admission.SessionID, session: session}
		return &GoalTickStream{Started: &AgentRunStream{Stream: stream, Run: run}}, nil
	case <-ctx.Done():
		return nil, contextError(op, ctx.Err())
	}
}

// AdmitSessionReviewDrafts admits host-bound ReviewFindingDraft values, all or
// nothing, and returns how many were newly admitted.
func (session *Session) AdmitSessionReviewDrafts(
	ctx context.Context,
	scenarioID string,
	sourceReviewID string,
	observedAtMS uint64,
	drafts json.RawMessage,
) (uint, error) {
	const op = "session_admit_review_drafts"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	params := session.params()
	params["scenario_id"] = scenarioID
	params["source_review_id"] = sourceReviewID
	params["observed_at_ms"] = observedAtMS
	params["drafts"] = drafts
	var result struct {
		Admitted uint `json:"admitted"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Admitted, err
}
