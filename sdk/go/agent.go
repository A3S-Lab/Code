package code

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"

	"github.com/A3S-Lab/Code/sdk/go/v6/internal/bridge"
)

type AgentOption interface {
	applyAgent(*agentConfig)
}

type agentOptionFunc func(*agentConfig)

func (option agentOptionFunc) applyAgent(config *agentConfig) {
	option(config)
}

type agentConfig struct {
	runtime             Runtime
	localRuntimeOptions []LocalRuntimeOption
}

// WithRuntime injects a custom concurrency-safe runtime. Its lifecycle remains
// owned by the caller.
func WithRuntime(runtime Runtime) AgentOption {
	return agentOptionFunc(func(config *agentConfig) {
		config.runtime = runtime
	})
}

// WithLocalRuntimeOptions configures the automatically-created LocalRuntime.
func WithLocalRuntimeOptions(options ...LocalRuntimeOption) AgentOption {
	values := append([]LocalRuntimeOption(nil), options...)
	return agentOptionFunc(func(config *agentConfig) {
		config.localRuntimeOptions = append(config.localRuntimeOptions, values...)
	})
}

// Agent is the workspace-independent entry point. It is safe for concurrent
// use.
type Agent struct {
	runtime      Runtime
	ownsRuntime  bool
	id           string
	capabilities Capabilities

	closeOnce sync.Once
	closeErr  error
}

// Create initializes an Agent from an ACL file path or inline ACL source.
func Create(ctx context.Context, configSource string, options ...AgentOption) (*Agent, error) {
	const op = "agent_create"
	if ctx == nil {
		return nil, invalid(op, "context cannot be nil")
	}
	if strings.TrimSpace(configSource) == "" {
		return nil, invalid(op, "config source cannot be empty")
	}

	config := agentConfig{}
	for _, option := range options {
		if option != nil {
			option.applyAgent(&config)
		}
	}
	ownsRuntime := false
	runtime := config.runtime
	if runtime == nil {
		local, err := NewLocalRuntime(ctx, config.localRuntimeOptions...)
		if err != nil {
			return nil, err
		}
		runtime = local
		ownsRuntime = true
	}

	capabilities, err := handshake(ctx, runtime)
	if err != nil {
		if ownsRuntime {
			_ = runtime.Close()
		}
		return nil, err
	}
	var created struct {
		AgentID string `json:"agent_id"`
	}
	if err := runtime.Request(ctx, op, map[string]any{
		"config_source": configSource,
	}, &created); err != nil {
		if ownsRuntime {
			_ = runtime.Close()
		}
		return nil, err
	}
	if created.AgentID == "" {
		if ownsRuntime {
			_ = runtime.Close()
		}
		return nil, sdkError(op, CodeProtocol, "bridge returned an empty agent id", nil)
	}
	return &Agent{
		runtime:      runtime,
		ownsRuntime:  ownsRuntime,
		id:           created.AgentID,
		capabilities: capabilities,
	}, nil
}

// NewAgent is an alias for Create.
func NewAgent(ctx context.Context, configSource string, options ...AgentOption) (*Agent, error) {
	return Create(ctx, configSource, options...)
}

func (agent *Agent) Capabilities() Capabilities {
	if agent == nil {
		return Capabilities{}
	}
	value := agent.capabilities
	value.Operations = append([]string(nil), value.Operations...)
	return value
}

func SupportedOperations() []string {
	return append([]string(nil), bridge.RequiredOperations...)
}

func (agent *Agent) Session(
	ctx context.Context,
	workspace string,
	options *SessionOptions,
) (*Session, error) {
	const op = "session_create"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(workspace) == "" {
		return nil, invalid(op, "workspace cannot be empty")
	}
	params := map[string]any{
		"agent_id":  agent.id,
		"workspace": workspace,
	}
	if options != nil {
		params["options"] = options
	}
	return agent.newSession(ctx, op, params)
}

func (agent *Agent) ResumeSession(
	ctx context.Context,
	persistedSessionID string,
	options *SessionOptions,
) (*Session, error) {
	const op = "session_resume"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(persistedSessionID) == "" {
		return nil, invalid(op, "persisted session id cannot be empty")
	}
	params := map[string]any{
		"agent_id":             agent.id,
		"persisted_session_id": persistedSessionID,
	}
	if options != nil {
		params["options"] = options
	}
	return agent.newSession(ctx, op, params)
}

func (agent *Agent) newSession(
	ctx context.Context,
	operation string,
	params map[string]any,
) (*Session, error) {
	var created struct {
		SessionHandle string  `json:"session_handle"`
		SessionID     string  `json:"session_id"`
		Workspace     string  `json:"workspace"`
		InitWarning   *string `json:"init_warning"`
	}
	if err := agent.runtime.Request(ctx, operation, params, &created); err != nil {
		return nil, err
	}
	if created.SessionHandle == "" || created.SessionID == "" {
		return nil, sdkError(
			operation,
			CodeProtocol,
			"bridge returned incomplete session identity",
			nil,
		)
	}
	return &Session{
		runtime:     agent.runtime,
		handle:      created.SessionHandle,
		id:          created.SessionID,
		workspace:   created.Workspace,
		initWarning: created.InitWarning,
	}, nil
}

func (agent *Agent) RefreshMCPTools(ctx context.Context) error {
	const op = "agent_refresh_mcp_tools"
	if err := validateAgent(agent, ctx, op); err != nil {
		return err
	}
	return agent.runtime.Request(ctx, op, map[string]any{"agent_id": agent.id}, nil)
}

func (agent *Agent) ListSessions(ctx context.Context) ([]string, error) {
	const op = "agent_list_sessions"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		SessionIDs []string `json:"session_ids"`
	}
	if err := agent.runtime.Request(
		ctx,
		op,
		map[string]any{"agent_id": agent.id},
		&result,
	); err != nil {
		return nil, err
	}
	return result.SessionIDs, nil
}

func (agent *Agent) CloseSession(ctx context.Context, sessionID string) (bool, error) {
	const op = "agent_close_session"
	if err := validateAgent(agent, ctx, op); err != nil {
		return false, err
	}
	if strings.TrimSpace(sessionID) == "" {
		return false, invalid(op, "session id cannot be empty")
	}
	var result struct {
		Closed bool `json:"closed"`
	}
	err := agent.runtime.Request(ctx, op, map[string]any{
		"agent_id":   agent.id,
		"session_id": sessionID,
	}, &result)
	return result.Closed, err
}

func (agent *Agent) IsClosed(ctx context.Context) (bool, error) {
	const op = "agent_is_closed"
	if err := validateAgent(agent, ctx, op); err != nil {
		return false, err
	}
	var result struct {
		Closed bool `json:"closed"`
	}
	err := agent.runtime.Request(
		ctx,
		op,
		map[string]any{"agent_id": agent.id},
		&result,
	)
	return result.Closed, err
}

// Close closes all sessions and the Agent. It also closes an automatically
// created local runtime. Close is idempotent.
func (agent *Agent) Close(ctx context.Context) error {
	const op = "agent_close"
	if agent == nil {
		return nil
	}
	if err := validateAgent(agent, ctx, op); err != nil {
		return err
	}
	agent.closeOnce.Do(func() {
		remoteErr := agent.runtime.Request(
			ctx,
			op,
			map[string]any{"agent_id": agent.id},
			nil,
		)
		var runtimeErr error
		if agent.ownsRuntime {
			runtimeErr = agent.runtime.Close()
		}
		agent.closeErr = errors.Join(remoteErr, runtimeErr)
	})
	return agent.closeErr
}

func handshake(ctx context.Context, runtime Runtime) (Capabilities, error) {
	const op = "sdk_capabilities"
	if runtime == nil {
		return Capabilities{}, invalid(op, "runtime cannot be nil")
	}
	var capabilities Capabilities
	if err := runtime.Request(ctx, op, nil, &capabilities); err != nil {
		return Capabilities{}, err
	}
	if capabilities.ProtocolVersion != bridge.ProtocolVersion {
		return Capabilities{}, sdkError(
			op,
			CodeProtocol,
			fmt.Sprintf(
				"runtime protocol version %d is unsupported",
				capabilities.ProtocolVersion,
			),
			nil,
		)
	}
	if capabilities.EventProtocolVersion != bridge.EventProtocolVersion {
		return Capabilities{}, sdkError(
			op,
			CodeProtocol,
			fmt.Sprintf(
				"runtime event protocol version %d is unsupported",
				capabilities.EventProtocolVersion,
			),
			nil,
		)
	}

	available := make(map[string]struct{}, len(capabilities.Operations))
	for _, operation := range capabilities.Operations {
		if _, duplicate := available[operation]; duplicate {
			return Capabilities{}, sdkError(
				op,
				CodeProtocol,
				"runtime capability inventory contains duplicate operation "+operation,
				nil,
			)
		}
		available[operation] = struct{}{}
	}
	missing := make([]string, 0)
	for _, operation := range bridge.RequiredOperations {
		if _, ok := available[operation]; !ok {
			missing = append(missing, operation)
		}
	}
	if len(missing) > 0 {
		sort.Strings(missing)
		return Capabilities{}, sdkError(
			op,
			CodeUnavailable,
			"runtime is missing required operations: "+strings.Join(missing, ", "),
			nil,
		)
	}
	capabilities.Operations = append([]string(nil), capabilities.Operations...)
	return capabilities, nil
}

func validateAgent(agent *Agent, ctx context.Context, operation string) error {
	if agent == nil || agent.runtime == nil || agent.id == "" {
		return invalid(operation, "agent is not initialized")
	}
	if ctx == nil {
		return invalid(operation, "context cannot be nil")
	}
	if err := ctx.Err(); err != nil {
		return contextError(operation, err)
	}
	return nil
}
