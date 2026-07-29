package code

import (
	"context"
	"strings"
)

func (session *Session) ToolNames(ctx context.Context) ([]string, error) {
	const op = "session_tool_names"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Names []string `json:"names"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result.Names, nil
}

func (session *Session) ToolDefinitions(ctx context.Context) ([]ToolDefinition, error) {
	const op = "session_tool_definitions"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []ToolDefinition
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) TraceEvents(ctx context.Context) ([]TraceEvent, error) {
	const op = "session_trace_events"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []TraceEvent
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) GetArtifact(
	ctx context.Context,
	artifactURI string,
) (*ToolArtifact, error) {
	const op = "session_get_artifact"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(artifactURI) == "" {
		return nil, invalid(op, "artifact URI cannot be empty")
	}
	params := session.params()
	params["artifact_uri"] = artifactURI
	var result struct {
		Artifact *ToolArtifact `json:"artifact"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result.Artifact, nil
}

func (session *Session) Runs(ctx context.Context) ([]RunSnapshot, error) {
	const op = "session_runs"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []RunSnapshot
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) RunSnapshot(
	ctx context.Context,
	runID string,
) (*RunSnapshot, error) {
	const op = "session_run_snapshot"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(runID) == "" {
		return nil, invalid(op, "run id cannot be empty")
	}
	params := session.params()
	params["run_id"] = runID
	var result struct {
		Snapshot *RunSnapshot `json:"snapshot"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result.Snapshot, nil
}

func (session *Session) RunEvents(ctx context.Context, runID string) ([]Event, error) {
	const op = "session_run_events"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(runID) == "" {
		return nil, invalid(op, "run id cannot be empty")
	}
	params := session.params()
	params["run_id"] = runID
	var result struct {
		Events []Event `json:"events"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result.Events, nil
}

func (session *Session) RunEventPage(
	ctx context.Context,
	runID string,
	afterSequence *uint,
	limit uint,
) (*RunEventPage, error) {
	const op = "session_run_event_page"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(runID) == "" {
		return nil, invalid(op, "run id cannot be empty")
	}
	if limit == 0 {
		return nil, invalid(op, "limit must be greater than zero")
	}
	params := session.params()
	params["run_id"] = runID
	params["limit"] = limit
	if afterSequence != nil {
		params["after_sequence"] = *afterSequence
	}
	var result struct {
		Page *RunEventPage `json:"page"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result.Page, nil
}

func (session *Session) CurrentRun(ctx context.Context) (*CurrentRun, error) {
	const op = "session_current_run"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Run *CurrentRun `json:"run"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result.Run, nil
}

func (session *Session) ActiveTools(ctx context.Context) ([]ActiveTool, error) {
	const op = "session_active_tools"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []ActiveTool
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) SubagentTask(
	ctx context.Context,
	taskID string,
) (*SubagentTask, error) {
	const op = "session_subagent_task"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(taskID) == "" {
		return nil, invalid(op, "task id cannot be empty")
	}
	params := session.params()
	params["task_id"] = taskID
	var result struct {
		Task *SubagentTask `json:"task"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result.Task, nil
}

func (session *Session) SubagentTasks(ctx context.Context) ([]SubagentTask, error) {
	const op = "session_subagent_tasks"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []SubagentTask
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) PendingSubagentTasks(ctx context.Context) ([]SubagentTask, error) {
	const op = "session_pending_subagent_tasks"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []SubagentTask
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) CancelSubagentTask(
	ctx context.Context,
	taskID string,
) (bool, error) {
	const op = "session_cancel_subagent_task"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	if strings.TrimSpace(taskID) == "" {
		return false, invalid(op, "task id cannot be empty")
	}
	params := session.params()
	params["task_id"] = taskID
	var result struct {
		Cancelled bool `json:"cancelled"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Cancelled, err
}

func (session *Session) CancelRun(ctx context.Context, runID string) (bool, error) {
	const op = "session_cancel_run"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	if strings.TrimSpace(runID) == "" {
		return false, invalid(op, "run id cannot be empty")
	}
	params := session.params()
	params["run_id"] = runID
	var result struct {
		Cancelled bool `json:"cancelled"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Cancelled, err
}

func (session *Session) PendingConfirmations(
	ctx context.Context,
) ([]PendingConfirmation, error) {
	const op = "session_pending_confirmations"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []PendingConfirmation
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) ConfirmToolUse(
	ctx context.Context,
	toolID string,
	approved bool,
	reason string,
) (bool, error) {
	const op = "session_confirm_tool_use"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	if strings.TrimSpace(toolID) == "" {
		return false, invalid(op, "tool id cannot be empty")
	}
	params := session.params()
	params["tool_id"] = toolID
	params["approved"] = approved
	if reason != "" {
		params["reason"] = reason
	}
	var result struct {
		Confirmed bool `json:"confirmed"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Confirmed, err
}

func (session *Session) CancelConfirmations(ctx context.Context) (uint, error) {
	const op = "session_cancel_confirmations"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	var result struct {
		Count uint `json:"count"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Count, err
}

func (session *Session) VerificationReports(
	ctx context.Context,
) ([]VerificationReport, error) {
	const op = "session_verification_reports"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []VerificationReport
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) RecordVerificationReports(
	ctx context.Context,
	reports []VerificationReport,
) error {
	const op = "session_record_verification_reports"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if len(reports) == 0 {
		return invalid(op, "at least one report is required")
	}
	params := session.params()
	params["reports"] = reports
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) VerificationSummary(
	ctx context.Context,
) (VerificationSummary, error) {
	const op = "session_verification_summary"
	if err := validateSession(session, ctx, op); err != nil {
		return VerificationSummary{}, err
	}
	var result VerificationSummary
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result, err
}

func (session *Session) VerificationSummaryText(ctx context.Context) (string, error) {
	const op = "session_verification_summary_text"
	if err := validateSession(session, ctx, op); err != nil {
		return "", err
	}
	var result struct {
		Text string `json:"text"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Text, err
}

func (session *Session) VerificationPresets(
	ctx context.Context,
) ([]VerificationPreset, error) {
	const op = "session_verification_presets"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []VerificationPreset
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) VerifyCommands(
	ctx context.Context,
	subject string,
	commands []VerificationCommand,
) (*VerificationReport, error) {
	const op = "session_verify_commands"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(subject) == "" {
		return nil, invalid(op, "subject cannot be empty")
	}
	if len(commands) == 0 {
		return nil, invalid(op, "at least one command is required")
	}
	params := session.params()
	params["subject"] = subject
	params["commands"] = commands
	var result VerificationReport
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}
