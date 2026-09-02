package code

import (
	"context"
	"strings"
)

func (session *Session) RegisterAgentDir(ctx context.Context, path string) (uint, error) {
	const op = "session_register_agent_dir"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	if strings.TrimSpace(path) == "" {
		return 0, invalid(op, "path cannot be empty")
	}
	params := session.params()
	params["path"] = path
	var result struct {
		Count uint `json:"count"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Count, err
}

func (session *Session) RegisterWorkerAgent(
	ctx context.Context,
	worker WorkerAgentSpec,
) (*AgentDefinition, error) {
	const op = "session_register_worker_agent"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(worker.Name) == "" || strings.TrimSpace(worker.Description) == "" {
		return nil, invalid(op, "worker name and description cannot be empty")
	}
	params := session.params()
	params["worker"] = worker
	var result AgentDefinition
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (session *Session) RegisterWorkerAgents(
	ctx context.Context,
	workers []WorkerAgentSpec,
) ([]AgentDefinition, error) {
	const op = "session_register_worker_agents"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if len(workers) == 0 {
		return nil, invalid(op, "at least one worker is required")
	}
	params := session.params()
	params["workers"] = workers
	var result []AgentDefinition
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) AddSkill(ctx context.Context, skill InlineSkill) error {
	const op = "session_add_skill"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(skill.Name) == "" {
		return invalid(op, "skill name cannot be empty")
	}
	params := session.params()
	params["skill"] = skill
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) RemoveSkill(ctx context.Context, name string) error {
	const op = "session_remove_skill"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(name) == "" {
		return invalid(op, "skill name cannot be empty")
	}
	params := session.params()
	params["name"] = name
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) SkillNames(ctx context.Context) ([]string, error) {
	const op = "session_skill_names"
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

func (session *Session) RegisterDynamicWorkflow(ctx context.Context) error {
	const op = "session_register_dynamic_workflow"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	return session.runtime.Request(ctx, op, session.params(), nil)
}

func (session *Session) RegisterDynamicWorkflowRuntime(ctx context.Context) error {
	return session.RegisterDynamicWorkflow(ctx)
}

func (session *Session) UnregisterDynamicTool(ctx context.Context, name string) error {
	const op = "session_unregister_dynamic_tool"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(name) == "" {
		return invalid(op, "tool name cannot be empty")
	}
	params := session.params()
	params["name"] = name
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) AddMCPServer(
	ctx context.Context,
	config MCPServerConfig,
) (uint, error) {
	const op = "session_add_mcp_server"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	if strings.TrimSpace(config.Name) == "" {
		return 0, invalid(op, "server name cannot be empty")
	}
	switch config.Transport.Type {
	case "stdio":
		if strings.TrimSpace(config.Transport.Command) == "" {
			return 0, invalid(op, "stdio transport command cannot be empty")
		}
	case "http", "streamable-http":
		if strings.TrimSpace(config.Transport.URL) == "" {
			return 0, invalid(op, "HTTP transport URL cannot be empty")
		}
	default:
		return 0, invalid(op, "transport type must be stdio, http, or streamable-http")
	}
	params := session.params()
	params["config"] = config
	var result struct {
		ToolCount uint `json:"tool_count"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.ToolCount, err
}

// AddMCPServerConfig is an alias for AddMCPServer.
func (session *Session) AddMCPServerConfig(
	ctx context.Context,
	config MCPServerConfig,
) (uint, error) {
	return session.AddMCPServer(ctx, config)
}

// AddMCP is the compact alias for AddMCPServer.
func (session *Session) AddMCP(
	ctx context.Context,
	config MCPServerConfig,
) (uint, error) {
	return session.AddMCPServer(ctx, config)
}

func (session *Session) RemoveMCPServer(ctx context.Context, name string) error {
	const op = "session_remove_mcp_server"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(name) == "" {
		return invalid(op, "server name cannot be empty")
	}
	params := session.params()
	params["name"] = name
	return session.runtime.Request(ctx, op, params, nil)
}

// RemoveMCP is the compact alias for RemoveMCPServer.
func (session *Session) RemoveMCP(ctx context.Context, name string) error {
	return session.RemoveMCPServer(ctx, name)
}

func (session *Session) MCPStatus(
	ctx context.Context,
) (map[string]MCPServerStatus, error) {
	const op = "session_mcp_status"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result map[string]MCPServerStatus
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

// MCPs is an alias for MCPStatus.
func (session *Session) MCPs(ctx context.Context) (map[string]MCPServerStatus, error) {
	return session.MCPStatus(ctx)
}

// CapabilityCatalogStamp returns the exact generation and digest visible to
// newly admitted runs.
func (session *Session) CapabilityCatalogStamp(ctx context.Context) (*CapabilityCatalogStamp, error) {
	const op = "session_capability_catalog_stamp"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result CapabilityCatalogStamp
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// ToolPresentationProfile returns the typed profile used for model-facing
// definitions in this session.
func (session *Session) ToolPresentationProfile(ctx context.Context) (*ToolPresentationProfile, error) {
	const op = "session_tool_presentation_profile"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result ToolPresentationProfile
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// PresentedToolDefinitions previews the model-facing definition projection
// for a prompt without granting execution authority.
func (session *Session) PresentedToolDefinitions(ctx context.Context, prompt string) ([]ToolDefinition, error) {
	const op = "session_presented_tool_definitions"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(prompt) == "" {
		return nil, invalid(op, "prompt cannot be empty")
	}
	params := session.params()
	params["prompt"] = prompt
	var result []ToolDefinition
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result, nil
}

// CurrentCognitivePackageBinding returns the exact secret-free Knowledge
// binding, when the host has installed one.
func (session *Session) CurrentCognitivePackageBinding(ctx context.Context) (*CognitivePackageBindingV1, error) {
	const op = "session_current_cognitive_package_binding"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result *CognitivePackageBindingV1
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

// EnsureRecoveryCapabilityBinding validates an exact persisted catalog
// binding before a host starts recovery.
func (session *Session) EnsureRecoveryCapabilityBinding(ctx context.Context, binding any) error {
	const op = "session_ensure_recovery_capability_binding"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if binding == nil {
		return invalid(op, "binding cannot be nil")
	}
	params := session.params()
	params["binding"] = binding
	return session.runtime.Request(ctx, op, params, nil)
}

// DrainCapabilityCleanup settles retired host capability effects.
func (session *Session) DrainCapabilityCleanup(ctx context.Context) (*CapabilityCleanupReport, error) {
	const op = "session_drain_capability_cleanup"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result CapabilityCleanupReport
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return &result, nil
}
