package code

import (
	"context"
	"strings"
	"sync"
	"time"
)

type SessionInfo struct {
	SessionID       string  `json:"session_id"`
	Workspace       string  `json:"workspace"`
	InitWarning     *string `json:"init_warning"`
	TenantID        *string `json:"tenant_id"`
	Principal       *string `json:"principal"`
	AgentTemplateID *string `json:"agent_template_id"`
	CorrelationID   *string `json:"correlation_id"`
}

// Session is a workspace-bound agent execution context. Conversation methods
// are single-flight; read-only observation methods may be called concurrently.
type Session struct {
	runtime         Runtime
	owner           *Agent
	handle          string
	id              string
	workspace       string
	initWarning     *string
	tenantID        *string
	principal       *string
	agentTemplateID *string
	correlationID   *string

	closeOnce sync.Once
	closeErr  error

	callbackMu        sync.Mutex
	hookCallbacks     map[string]string
	commandCallbacks  map[string]string
	budgetCallback    string
	retrievalCallback string
}

func (session *Session) ID() string {
	if session == nil {
		return ""
	}
	return session.id
}

func (session *Session) SessionID() string {
	return session.ID()
}

func (session *Session) Workspace() string {
	if session == nil {
		return ""
	}
	return session.workspace
}

func (session *Session) InitWarning() *string {
	if session == nil || session.initWarning == nil {
		return nil
	}
	value := *session.initWarning
	return &value
}

func (session *Session) TenantID() *string {
	return copyStringPointer(session, func(value *Session) *string { return value.tenantID })
}

func (session *Session) Principal() *string {
	return copyStringPointer(session, func(value *Session) *string { return value.principal })
}

func (session *Session) AgentTemplateID() *string {
	return copyStringPointer(session, func(value *Session) *string { return value.agentTemplateID })
}

func (session *Session) CorrelationID() *string {
	return copyStringPointer(session, func(value *Session) *string { return value.correlationID })
}

func copyStringPointer(session *Session, get func(*Session) *string) *string {
	if session == nil {
		return nil
	}
	pointer := get(session)
	if pointer == nil {
		return nil
	}
	value := *pointer
	return &value
}

func (session *Session) Info(ctx context.Context) (SessionInfo, error) {
	const op = "session_info"
	if err := validateSession(session, ctx, op); err != nil {
		return SessionInfo{}, err
	}
	var result SessionInfo
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result, err
}

// TaskSchedulerStats returns current occupancy of the Agent-wide priority
// scheduler shared by this session and its siblings.
func (session *Session) TaskSchedulerStats(ctx context.Context) (TaskSchedulerStats, error) {
	const op = "session_task_scheduler_stats"
	if err := validateSession(session, ctx, op); err != nil {
		return TaskSchedulerStats{}, err
	}
	var result TaskSchedulerStats
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result, err
}

// TaskSchedulerHealth returns scheduler occupancy plus bounded cumulative
// admission and fairness counters shared by this session and its siblings.
func (session *Session) TaskSchedulerHealth(ctx context.Context) (TaskSchedulerHealthSnapshot, error) {
	const op = "session_task_scheduler_health"
	if err := validateSession(session, ctx, op); err != nil {
		return TaskSchedulerHealthSnapshot{}, err
	}
	var result TaskSchedulerHealthSnapshot
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result, err
}

// Send executes one prompt and waits for the complete response. A nil history
// uses and updates the session's internal conversation history.
func (session *Session) Send(
	ctx context.Context,
	prompt string,
	history []Message,
) (*AgentResult, error) {
	const op = "session_send"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(prompt) == "" {
		return nil, invalid(op, "prompt cannot be empty")
	}
	params := session.params()
	params["prompt"] = prompt
	if history != nil {
		params["history"] = history
	}
	var result AgentResult
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		if ctx.Err() != nil {
			session.bestEffortCancel()
		}
		return nil, err
	}
	return &result, nil
}

// Run is the short form of Send using the session's internal history.
func (session *Session) Run(ctx context.Context, prompt string) (*AgentResult, error) {
	return session.Send(ctx, prompt, nil)
}

func (session *Session) SendRequest(
	ctx context.Context,
	request SessionRequest,
) (*AgentResult, error) {
	if len(request.Attachments) > 0 {
		return session.SendWithAttachments(
			ctx,
			request.Prompt,
			request.Attachments,
			request.History,
		)
	}
	return session.Send(ctx, request.Prompt, request.History)
}

func (session *Session) ResumeRun(
	ctx context.Context,
	checkpointRunID string,
) (*AgentResult, error) {
	const op = "session_resume_run"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(checkpointRunID) == "" {
		return nil, invalid(op, "checkpoint run id cannot be empty")
	}
	params := session.params()
	params["checkpoint_run_id"] = checkpointRunID
	var result AgentResult
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// SpawnRunWithID admits an exact host-selected run ID and executes it in the
// background. A compatible existing ID is replayed without duplicate work.
func (session *Session) SpawnRunWithID(
	ctx context.Context,
	runID string,
	prompt string,
) (*RunSpawn, error) {
	const op = "session_spawn_run_with_id"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(runID) == "" {
		return nil, invalid(op, "run id cannot be empty")
	}
	if strings.TrimSpace(prompt) == "" {
		return nil, invalid(op, "prompt cannot be empty")
	}
	params := session.params()
	params["run_id"] = runID
	params["prompt"] = prompt
	var result RunSpawn
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// SpawnRecoveryWithRunID resumes a checkpoint under an exact host-selected
// run ID in the background. A compatible existing ID is replayed without
// duplicate recovery work.
func (session *Session) SpawnRecoveryWithRunID(
	ctx context.Context,
	checkpointRunID string,
	runID string,
) (*RunSpawn, error) {
	const op = "session_spawn_recovery_with_run_id"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(checkpointRunID) == "" {
		return nil, invalid(op, "checkpoint run id cannot be empty")
	}
	if strings.TrimSpace(runID) == "" {
		return nil, invalid(op, "run id cannot be empty")
	}
	params := session.params()
	params["checkpoint_run_id"] = checkpointRunID
	params["run_id"] = runID
	var result RunSpawn
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (session *Session) SendWithAttachments(
	ctx context.Context,
	prompt string,
	attachments []Attachment,
	history []Message,
) (*AgentResult, error) {
	const op = "session_send_with_attachments"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(prompt) == "" {
		return nil, invalid(op, "prompt cannot be empty")
	}
	if len(attachments) == 0 {
		return nil, invalid(op, "at least one attachment is required")
	}
	params := session.params()
	params["prompt"] = prompt
	params["attachments"] = attachments
	if history != nil {
		params["history"] = history
	}
	var result AgentResult
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// Stream starts one prompt and returns its event stream. A nil history uses and
// updates the session's internal conversation history.
func (session *Session) Stream(
	ctx context.Context,
	prompt string,
	history []Message,
) (*EventStream, error) {
	const op = "session_stream"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(prompt) == "" {
		return nil, invalid(op, "prompt cannot be empty")
	}
	params := session.params()
	params["prompt"] = prompt
	if history != nil {
		params["history"] = history
	}
	return session.runtime.Stream(ctx, op, params)
}

func (session *Session) StreamRequest(
	ctx context.Context,
	request SessionRequest,
) (*EventStream, error) {
	if len(request.Attachments) > 0 {
		return session.StreamWithAttachments(
			ctx,
			request.Prompt,
			request.Attachments,
			request.History,
		)
	}
	return session.Stream(ctx, request.Prompt, request.History)
}

func (session *Session) StreamWithAttachments(
	ctx context.Context,
	prompt string,
	attachments []Attachment,
	history []Message,
) (*EventStream, error) {
	const op = "session_stream_with_attachments"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(prompt) == "" {
		return nil, invalid(op, "prompt cannot be empty")
	}
	if len(attachments) == 0 {
		return nil, invalid(op, "at least one attachment is required")
	}
	params := session.params()
	params["prompt"] = prompt
	params["attachments"] = attachments
	if history != nil {
		params["history"] = history
	}
	return session.runtime.Stream(ctx, op, params)
}

func (session *Session) Parallel(
	ctx context.Context,
	specs []AgentStepSpec,
	budgetTokens *uint64,
) (*ParallelResult, error) {
	const op = "session_parallel"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if len(specs) == 0 {
		return nil, invalid(op, "at least one step is required")
	}
	params := session.params()
	params["specs"] = specs
	if budgetTokens != nil {
		params["budget_tokens"] = *budgetTokens
	}
	var result ParallelResult
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (session *Session) ParallelResumable(
	ctx context.Context,
	specs []AgentStepSpec,
	workflowID string,
) ([]StepOutcome, error) {
	const op = "session_parallel_resumable"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if len(specs) == 0 {
		return nil, invalid(op, "at least one step is required")
	}
	if strings.TrimSpace(workflowID) == "" {
		return nil, invalid(op, "workflow id cannot be empty")
	}
	params := session.params()
	params["specs"] = specs
	params["workflow_id"] = workflowID
	var result []StepOutcome
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) WorkflowStep(
	ctx context.Context,
	spec AgentStepSpec,
) (*StepOutcome, error) {
	const op = "session_workflow_step"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(spec.TaskID) == "" || strings.TrimSpace(spec.Agent) == "" {
		return nil, invalid(op, "step task id and agent cannot be empty")
	}
	params := session.params()
	params["spec"] = spec
	var result StepOutcome
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (session *Session) Cancel(ctx context.Context) (bool, error) {
	const op = "session_cancel"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	var result struct {
		Cancelled bool `json:"cancelled"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Cancelled, err
}

func (session *Session) CancelAndSettle(
	ctx context.Context,
	grace time.Duration,
	abortGrace time.Duration,
) (bool, error) {
	const op = "session_cancel_and_settle"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	if grace < 0 || abortGrace < 0 {
		return false, invalid(op, "grace durations cannot be negative")
	}
	params := session.params()
	params["grace_ms"] = grace.Milliseconds()
	params["abort_grace_ms"] = abortGrace.Milliseconds()
	var result struct {
		Settled bool `json:"settled"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Settled, err
}

func (session *Session) History(ctx context.Context) ([]Message, error) {
	const op = "session_history"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Messages []Message `json:"messages"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result.Messages, nil
}

func (session *Session) IsClosed(ctx context.Context) (bool, error) {
	const op = "session_is_closed"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	var result struct {
		Closed bool `json:"closed"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Closed, err
}

func (session *Session) Save(ctx context.Context) error {
	const op = "session_save"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	return session.runtime.Request(ctx, op, session.params(), nil)
}

// Close is idempotent and does not close the parent Agent or shared runtime.
func (session *Session) Close(ctx context.Context) error {
	const op = "session_close"
	if session == nil {
		return nil
	}
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	session.closeOnce.Do(func() {
		session.closeErr = session.runtime.Request(ctx, op, session.params(), nil)
		session.releaseCallbacks()
	})
	return session.closeErr
}

func (session *Session) releaseCallbacks() {
	runtime, ok := session.runtime.(callbackRuntime)
	if !ok {
		return
	}
	session.callbackMu.Lock()
	ids := make([]string, 0, len(session.hookCallbacks)+len(session.commandCallbacks)+1)
	for _, id := range session.hookCallbacks {
		ids = append(ids, id)
	}
	for _, id := range session.commandCallbacks {
		ids = append(ids, id)
	}
	if session.budgetCallback != "" {
		ids = append(ids, session.budgetCallback)
	}
	retrievalCallback := session.retrievalCallback
	session.hookCallbacks = nil
	session.commandCallbacks = nil
	session.budgetCallback = ""
	session.retrievalCallback = ""
	session.callbackMu.Unlock()
	for _, id := range ids {
		runtime.unregisterCallback(id)
	}
	if retrievalCallback != "" {
		runtime.unregisterCallback(retrievalCallback)
		if session.owner != nil {
			session.owner.forgetRetrievalCallback(retrievalCallback)
		}
	}
}

func (session *Session) releaseRetrievalCallback() {
	if session == nil {
		return
	}
	session.callbackMu.Lock()
	callbackID := session.retrievalCallback
	session.retrievalCallback = ""
	session.callbackMu.Unlock()
	if callbackID == "" {
		return
	}
	if session.owner != nil {
		session.owner.releaseRetrievalCallback(callbackID)
		return
	}
	if runtime, ok := session.runtime.(callbackRuntime); ok {
		runtime.unregisterCallback(callbackID)
	}
}

func (session *Session) params() map[string]any {
	return map[string]any{"session_handle": session.handle}
}

func (session *Session) bestEffortCancel() {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_, _ = session.Cancel(ctx)
}

func validateSession(session *Session, ctx context.Context, operation string) error {
	if session == nil || session.runtime == nil || session.handle == "" {
		return invalid(operation, "session is not initialized")
	}
	if ctx == nil {
		return invalid(operation, "context cannot be nil")
	}
	if err := ctx.Err(); err != nil {
		return contextError(operation, err)
	}
	return nil
}
