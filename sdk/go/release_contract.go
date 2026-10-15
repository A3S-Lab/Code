package code

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// VerificationContract is a host-owned exact command and its covered paths.
type VerificationContract struct {
	ID      string   `json:"id"`
	Command string   `json:"command"`
	Covers  []string `json:"covers"`
}

// SamplingHandler receives an exact server name and MCP createMessage params.
// The host owns consent and model selection. Context cancellation and errors
// refuse the request; no handler is installed by default.
type SamplingHandler func(context.Context, string, json.RawMessage) (map[string]any, error)

func (agent *Agent) GlobalMCPConfigs(ctx context.Context) ([]MCPServerConfig, error) {
	const op = "agent_global_mcp_configs"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	var result []MCPServerConfig
	err := agent.runtime.Request(ctx, op, map[string]any{"agent_id": agent.id}, &result)
	return result, err
}

func (agent *Agent) HasSamplingHandlerFactory(ctx context.Context) (bool, error) {
	const op = "agent_has_sampling_handler_factory"
	if err := validateAgent(agent, ctx, op); err != nil {
		return false, err
	}
	var result bool
	err := agent.runtime.Request(ctx, op, map[string]any{"agent_id": agent.id}, &result)
	return result, err
}

// SetSamplingHandlerFactory arms subsequently connected servers. Existing
// connections keep their original callback until the Agent closes.
func (agent *Agent) SetSamplingHandlerFactory(ctx context.Context, handler SamplingHandler, timeout time.Duration) error {
	const op = "agent_set_sampling_handler_factory"
	if err := validateAgent(agent, ctx, op); err != nil {
		return err
	}
	if handler == nil {
		return invalid(op, "sampling handler cannot be nil")
	}
	if timeout == 0 {
		timeout = 30 * time.Second
	}
	if timeout < time.Millisecond || timeout > 5*time.Minute {
		return invalid(op, "sampling timeout must be from 1ms to 5m")
	}
	callbacks, ok := agent.runtime.(callbackRuntime)
	if !ok {
		return sdkError(op, CodeUnavailable, "runtime does not support Go callbacks", nil)
	}
	id, err := callbacks.registerCallback(func(ctx context.Context, method string, payload json.RawMessage) (any, error) {
		if method != "sampling/createMessage" {
			return nil, fmt.Errorf("unexpected sampling callback method %q", method)
		}
		var input struct {
			Server string          `json:"server_name"`
			Params json.RawMessage `json:"params"`
		}
		if err := json.Unmarshal(payload, &input); err != nil {
			return nil, err
		}
		result, err := handler(ctx, input.Server, input.Params)
		if err == nil && result == nil {
			return nil, fmt.Errorf("sampling result must be an object")
		}
		return result, err
	})
	if err != nil {
		return err
	}
	if err = agent.runtime.Request(ctx, op, map[string]any{"agent_id": agent.id, "handler_id": id, "timeout_ms": timeout.Milliseconds()}, nil); err != nil {
		callbacks.unregisterCallback(id)
		return err
	}
	// Reuse the existing Agent-owned callback lifecycle and close drain.
	agent.trackRetrievalCallback("", id)
	return nil
}

func (session *Session) ProjectedCapabilityNames(ctx context.Context) ([]string, error) {
	const op = "session_projected_capability_names"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []string
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result, err
}

func (session *Session) SetPermissionPolicy(ctx context.Context, policy PermissionPolicy) error {
	const op = "session_set_permission_policy"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	params := session.params()
	params["policy"] = policy
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) SetPermissionPosture(ctx context.Context, posture string) error {
	const op = "session_set_permission_posture"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	switch posture {
	case "default", "plan", "auto", "always-approve", "yolo":
	default:
		return invalid(op, "unknown permission posture")
	}
	if posture == "always-approve" {
		posture = "yolo"
	}
	params := session.params()
	params["posture"] = posture
	return session.runtime.Request(ctx, op, params, nil)
}

// ForkSessionAt irreversibly discards later facts in this session's log.
// It does not create a session or roll back files, vectors or durable memory.
func (session *Session) ForkSessionAt(ctx context.Context, seq uint64) (uint64, error) {
	const op = "session_fork_session_at"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	params := session.params()
	params["seq"] = seq
	var result uint64
	err := session.runtime.Request(ctx, op, params, &result)
	return result, err
}

func (session *Session) ReadEvaluationEvidence(ctx context.Context, request json.RawMessage) (json.RawMessage, error) {
	const op = "session_read_evaluation_evidence"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	params := session.params()
	params["request"] = request
	var result json.RawMessage
	err := session.runtime.Request(ctx, op, params, &result)
	return result, err
}

// AgentRunStream pairs the stream with the Core identity captured at admission.
type AgentRunStream struct {
	Stream *EventStream
	Run    *RunHandle
}
type StreamAdmission struct {
	RunID     string       `json:"run_id"`
	SessionID string       `json:"session_id"`
	Decision  string       `json:"decision,omitempty"`
	Replayed  *RunSnapshot `json:"replayed,omitempty"`
	Err       error        `json:"-"`
}

// RunHandle always addresses its original Run through typed Core control.
type RunHandle struct {
	ID        string
	SessionID string
	session   *Session
}

func (run *RunHandle) Snapshot(ctx context.Context) (*RunSnapshot, error) {
	return run.session.RunSnapshot(ctx, run.ID)
}
func (run *RunHandle) Cancel(ctx context.Context) (bool, error) {
	return run.session.CancelRun(ctx, run.ID)
}

func (session *Session) StreamWithRun(ctx context.Context, prompt string, history []Message) (*AgentRunStream, error) {
	const op = "session_stream_with_run"
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
		result := &AgentRunStream{Stream: stream}
		if admission.RunID != "" {
			if admission.SessionID != session.ID() {
				return nil, sdkError(op, CodeProtocol, "stream session identity mismatch", nil)
			}
			result.Run = &RunHandle{ID: admission.RunID, SessionID: admission.SessionID, session: session}
		}
		return result, nil
	case <-ctx.Done():
		return nil, contextError(op, ctx.Err())
	}
}
