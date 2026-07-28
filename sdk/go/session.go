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
	runtime     Runtime
	handle      string
	id          string
	workspace   string
	initWarning *string

	closeOnce sync.Once
	closeErr  error
}

func (session *Session) ID() string {
	if session == nil {
		return ""
	}
	return session.id
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

func (session *Session) Info(ctx context.Context) (SessionInfo, error) {
	const op = "session_info"
	if err := validateSession(session, ctx, op); err != nil {
		return SessionInfo{}, err
	}
	var result SessionInfo
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
	})
	return session.closeErr
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
