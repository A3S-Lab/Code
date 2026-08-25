package code

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"

	"github.com/A3S-Lab/Code/sdk/go/v8/internal/bridge"
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

	callbackMu                sync.Mutex
	retrievalCallbacks        map[string]struct{}
	sessionRetrievalCallbacks map[string]map[string]struct{}
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

func (agent *Agent) ReplaceSession(
	ctx context.Context,
	current *Session,
	options *SessionOptions,
) (*Session, error) {
	const op = "agent_replace_session"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	if current == nil || current.handle == "" {
		return nil, invalid(op, "current session is not initialized")
	}
	params := current.params()
	params["agent_id"] = agent.id
	if options != nil {
		params["options"] = options
	}
	replacement, err := agent.newSession(ctx, op, params)
	if err != nil {
		return nil, err
	}
	current.releaseRetrievalCallback()
	return replacement, nil
}

func (agent *Agent) SessionForAgent(
	ctx context.Context,
	workspace string,
	agentName string,
	agentDirs []string,
	options *SessionOptions,
) (*Session, error) {
	const op = "agent_session_for_agent"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(workspace) == "" {
		return nil, invalid(op, "workspace cannot be empty")
	}
	if strings.TrimSpace(agentName) == "" {
		return nil, invalid(op, "agent name cannot be empty")
	}
	params := map[string]any{
		"agent_id":   agent.id,
		"workspace":  workspace,
		"agent_name": agentName,
		"agent_dirs": agentDirs,
	}
	if options != nil {
		params["options"] = options
	}
	return agent.newSession(ctx, op, params)
}

func (agent *Agent) SessionForWorker(
	ctx context.Context,
	workspace string,
	worker WorkerAgentSpec,
	options *SessionOptions,
) (*Session, error) {
	const op = "agent_session_for_worker"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(workspace) == "" {
		return nil, invalid(op, "workspace cannot be empty")
	}
	if strings.TrimSpace(worker.Name) == "" || strings.TrimSpace(worker.Description) == "" {
		return nil, invalid(op, "worker name and description cannot be empty")
	}
	params := map[string]any{
		"agent_id":  agent.id,
		"workspace": workspace,
		"worker":    worker,
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
	preparedOptions, callbackID, err := prepareSessionOptions(
		agent.runtime,
		params["options"],
	)
	if err != nil {
		return nil, err
	}
	if preparedOptions != nil {
		params["options"] = preparedOptions
	}
	owned := false
	defer func() {
		if callbackID != "" && !owned {
			agent.unregisterRetrievalCallback(callbackID)
		}
	}()
	var created struct {
		SessionHandle   string  `json:"session_handle"`
		SessionID       string  `json:"session_id"`
		Workspace       string  `json:"workspace"`
		InitWarning     *string `json:"init_warning"`
		TenantID        *string `json:"tenant_id"`
		Principal       *string `json:"principal"`
		AgentTemplateID *string `json:"agent_template_id"`
		CorrelationID   *string `json:"correlation_id"`
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
	session := &Session{
		runtime:           agent.runtime,
		owner:             agent,
		handle:            created.SessionHandle,
		id:                created.SessionID,
		workspace:         created.Workspace,
		initWarning:       created.InitWarning,
		tenantID:          created.TenantID,
		principal:         created.Principal,
		agentTemplateID:   created.AgentTemplateID,
		correlationID:     created.CorrelationID,
		retrievalCallback: callbackID,
	}
	if callbackID != "" {
		agent.trackRetrievalCallback(created.SessionID, callbackID)
		owned = true
	}
	return session, nil
}

func (agent *Agent) RefreshMCPTools(ctx context.Context) error {
	const op = "agent_refresh_mcp_tools"
	if err := validateAgent(agent, ctx, op); err != nil {
		return err
	}
	return agent.runtime.Request(ctx, op, map[string]any{"agent_id": agent.id}, nil)
}

// TaskSchedulerStats returns current occupancy of the priority scheduler
// shared by every session created from this Agent.
func (agent *Agent) TaskSchedulerStats(ctx context.Context) (TaskSchedulerStats, error) {
	const op = "agent_task_scheduler_stats"
	if err := validateAgent(agent, ctx, op); err != nil {
		return TaskSchedulerStats{}, err
	}
	var result TaskSchedulerStats
	err := agent.runtime.Request(ctx, op, map[string]any{"agent_id": agent.id}, &result)
	return result, err
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
	if err == nil && result.Closed {
		agent.releaseSessionRetrievalCallbacks(sessionID)
	}
	return result.Closed, err
}

func (agent *Agent) DisconnectIdleMCP(
	ctx context.Context,
	idleThresholdMS uint64,
) ([]string, error) {
	const op = "agent_disconnect_idle_mcp"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Names []string `json:"names"`
	}
	err := agent.runtime.Request(ctx, op, map[string]any{
		"agent_id":          agent.id,
		"idle_threshold_ms": idleThresholdMS,
	}, &result)
	return result.Names, err
}

// ServeHandle observes and stops one filesystem-first serve daemon.
type ServeHandle struct {
	runtime           Runtime
	handle            string
	owner             *Agent
	retrievalCallback string
	stopOnce          sync.Once
	stopErr           error
}

// ServeStatus is the latest observable lifecycle state of a serve daemon.
type ServeStatus struct {
	Phase       string  `json:"phase"`
	FailureCode *string `json:"failure_code"`
	Ready       bool    `json:"ready"`
	Stopped     bool    `json:"stopped"`
}

// ServeAgentDir starts a filesystem-first daemon and returns only after its
// schedules, sessions, and tools are prepared and ready.
func (agent *Agent) ServeAgentDir(
	ctx context.Context,
	dir string,
	workspace string,
	options *SessionOptions,
) (*ServeHandle, error) {
	const op = "agent_serve_agent_dir"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(dir) == "" || strings.TrimSpace(workspace) == "" {
		return nil, invalid(op, "agent directory and workspace cannot be empty")
	}
	params := map[string]any{
		"agent_id":  agent.id,
		"dir":       dir,
		"workspace": workspace,
	}
	if options != nil {
		prepared, callbackID, err := prepareWorkspaceRetrievalOptions(agent.runtime, options)
		if err != nil {
			return nil, err
		}
		params["options"] = prepared
		var result struct {
			Handle string `json:"serve_handle"`
		}
		if err := agent.runtime.Request(ctx, op, params, &result); err != nil {
			agent.unregisterRetrievalCallback(callbackID)
			return nil, err
		}
		if result.Handle == "" {
			agent.unregisterRetrievalCallback(callbackID)
			return nil, sdkError(op, CodeProtocol, "bridge returned an empty serve handle", nil)
		}
		if callbackID != "" {
			agent.trackRetrievalCallback("", callbackID)
		}
		return &ServeHandle{
			runtime:           agent.runtime,
			handle:            result.Handle,
			owner:             agent,
			retrievalCallback: callbackID,
		}, nil
	}
	var result struct {
		Handle string `json:"serve_handle"`
	}
	if err := agent.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	if result.Handle == "" {
		return nil, sdkError(op, CodeProtocol, "bridge returned an empty serve handle", nil)
	}
	return &ServeHandle{runtime: agent.runtime, handle: result.Handle, owner: agent}, nil
}

// Status returns the daemon's current readiness and terminal failure state.
func (handle *ServeHandle) Status(ctx context.Context) (ServeStatus, error) {
	const op = "agent_serve_status"
	if ctx == nil {
		return ServeStatus{}, invalid(op, "context cannot be nil")
	}
	if handle == nil || handle.runtime == nil || strings.TrimSpace(handle.handle) == "" {
		return ServeStatus{}, invalid(op, "serve handle is not initialized")
	}
	var status ServeStatus
	err := handle.runtime.Request(ctx, op, map[string]any{
		"serve_handle": handle.handle,
	}, &status)
	return status, err
}

// Stop requests graceful shutdown and waits for the daemon task to settle.
func (handle *ServeHandle) Stop(ctx context.Context) error {
	const op = "agent_stop_serve"
	if handle == nil {
		return nil
	}
	if ctx == nil {
		return invalid(op, "context cannot be nil")
	}
	if handle.runtime == nil || strings.TrimSpace(handle.handle) == "" {
		return invalid(op, "serve handle is not initialized")
	}
	handle.stopOnce.Do(func() {
		handle.stopErr = handle.runtime.Request(ctx, op, map[string]any{
			"serve_handle": handle.handle,
		}, nil)
		if handle.stopErr == nil {
			handle.releaseRetrievalCallback()
		}
	})
	return handle.stopErr
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
		agent.releaseAllRetrievalCallbacks()
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
