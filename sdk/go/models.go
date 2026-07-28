package code

import "encoding/json"

type Capabilities struct {
	ProtocolVersion      int      `json:"protocol_version"`
	EventProtocolVersion int      `json:"event_protocol_version"`
	Operations           []string `json:"operations"`
}

type PlanningMode string

const (
	PlanningAuto     PlanningMode = "auto"
	PlanningEnabled  PlanningMode = "enabled"
	PlanningDisabled PlanningMode = "disabled"
)

type PromptSlots struct {
	Role          string `json:"role,omitempty"`
	Guidelines    string `json:"guidelines,omitempty"`
	ResponseStyle string `json:"response_style,omitempty"`
	Extra         string `json:"extra,omitempty"`
}

// SessionOptions contains value-shaped session overrides supported by the
// language-neutral bridge. Rust trait injections remain available only to
// native Rust embedders.
type SessionOptions struct {
	Model                              string       `json:"model,omitempty"`
	AgentDirs                          []string     `json:"agent_dirs,omitempty"`
	SkillDirs                          []string     `json:"skill_dirs,omitempty"`
	EnforceActiveSkillToolRestrictions *bool        `json:"enforce_active_skill_tool_restrictions,omitempty"`
	FileMemoryDir                      string       `json:"file_memory_dir,omitempty"`
	FileSessionStoreDir                string       `json:"file_session_store_dir,omitempty"`
	SessionID                          string       `json:"session_id,omitempty"`
	TenantID                           string       `json:"tenant_id,omitempty"`
	Principal                          string       `json:"principal,omitempty"`
	AgentTemplateID                    string       `json:"agent_template_id,omitempty"`
	CorrelationID                      string       `json:"correlation_id,omitempty"`
	PlanningMode                       PlanningMode `json:"planning_mode,omitempty"`
	GoalTracking                       *bool        `json:"goal_tracking,omitempty"`
	AutoSave                           *bool        `json:"auto_save,omitempty"`
	MaxParseRetries                    *uint32      `json:"max_parse_retries,omitempty"`
	ToolTimeoutMS                      *uint64      `json:"tool_timeout_ms,omitempty"`
	LLMAPITimeoutMS                    *uint64      `json:"llm_api_timeout_ms,omitempty"`
	CircuitBreakerThreshold            *uint32      `json:"circuit_breaker_threshold,omitempty"`
	DuplicateToolCallThreshold         *uint32      `json:"duplicate_tool_call_threshold,omitempty"`
	AutoCompact                        *bool        `json:"auto_compact,omitempty"`
	AutoCompactThreshold               *float32     `json:"auto_compact_threshold,omitempty"`
	MaxContextTokens                   *uint        `json:"max_context_tokens,omitempty"`
	ContinuationEnabled                *bool        `json:"continuation_enabled,omitempty"`
	MaxContinuationTurns               *uint32      `json:"max_continuation_turns,omitempty"`
	Temperature                        *float32     `json:"temperature,omitempty"`
	ThinkingBudget                     *uint        `json:"thinking_budget,omitempty"`
	MaxToolRounds                      *uint        `json:"max_tool_rounds,omitempty"`
	MaxParallelTasks                   *uint        `json:"max_parallel_tasks,omitempty"`
	AutoDelegationEnabled              *bool        `json:"auto_delegation_enabled,omitempty"`
	ManualDelegationEnabled            *bool        `json:"manual_delegation_enabled,omitempty"`
	AutoParallelDelegation             *bool        `json:"auto_parallel_delegation,omitempty"`
	PromptSlots                        *PromptSlots `json:"prompt_slots,omitempty"`
}

// Ptr is a convenience for pointer-valued options that distinguish an
// explicit zero value from an omitted setting.
func Ptr[T any](value T) *T {
	return &value
}

type ContentBlock map[string]any

type Message struct {
	Role             string         `json:"role"`
	Content          []ContentBlock `json:"content"`
	ReasoningContent string         `json:"reasoning_content,omitempty"`
}

func UserMessage(text string) Message {
	return Message{
		Role: "user",
		Content: []ContentBlock{{
			"type": "text",
			"text": text,
		}},
	}
}

func AssistantMessage(text string) Message {
	return Message{
		Role: "assistant",
		Content: []ContentBlock{{
			"type": "text",
			"text": text,
		}},
	}
}

type TokenUsage struct {
	PromptTokens     uint  `json:"prompt_tokens"`
	CompletionTokens uint  `json:"completion_tokens"`
	TotalTokens      uint  `json:"total_tokens"`
	CacheReadTokens  *uint `json:"cache_read_tokens,omitempty"`
	CacheWriteTokens *uint `json:"cache_write_tokens,omitempty"`
}

type AgentResult struct {
	Text                    string               `json:"text"`
	Messages                []Message            `json:"messages"`
	Usage                   TokenUsage           `json:"usage"`
	ToolCallsCount          uint                 `json:"tool_calls_count"`
	VerificationReports     []VerificationReport `json:"verification_reports"`
	VerificationSummary     VerificationSummary  `json:"verification_summary"`
	VerificationSummaryText string               `json:"verification_summary_text"`
	HasPendingVerification  bool                 `json:"has_pending_verification"`
}

// Event is the lossless, open-string event envelope shared by every SDK.
// Unknown future event types and their payloads are preserved.
type Event struct {
	Version  int             `json:"version"`
	Type     string          `json:"type"`
	Payload  json.RawMessage `json:"payload"`
	Metadata json.RawMessage `json:"metadata,omitempty"`
}

func (event Event) DecodePayload(target any) error {
	return json.Unmarshal(event.Payload, target)
}

type ToolCallResult struct {
	Name      string          `json:"name"`
	Output    string          `json:"output"`
	ExitCode  int             `json:"exit_code"`
	Metadata  json.RawMessage `json:"metadata,omitempty"`
	ErrorKind json.RawMessage `json:"error_kind,omitempty"`
}

type ToolDefinition struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Parameters  json.RawMessage `json:"parameters"`
}

type ToolArtifact struct {
	ArtifactID    string `json:"artifact_id"`
	ArtifactURI   string `json:"artifact_uri"`
	ToolName      string `json:"tool_name"`
	Content       string `json:"content"`
	OriginalBytes uint   `json:"original_bytes"`
	ShownBytes    uint   `json:"shown_bytes"`
}

type RunStatus string

const (
	RunCreated   RunStatus = "created"
	RunPlanning  RunStatus = "planning"
	RunExecuting RunStatus = "executing"
	RunVerifying RunStatus = "verifying"
	RunCompleted RunStatus = "completed"
	RunFailed    RunStatus = "failed"
	RunCancelled RunStatus = "cancelled"
)

type RunSnapshot struct {
	ID          string    `json:"id"`
	SessionID   string    `json:"session_id"`
	Status      RunStatus `json:"status"`
	Prompt      string    `json:"prompt"`
	CreatedAtMS uint64    `json:"created_at_ms"`
	UpdatedAtMS uint64    `json:"updated_at_ms"`
	ResultText  *string   `json:"result_text,omitempty"`
	Error       *string   `json:"error,omitempty"`
	EventCount  uint      `json:"event_count"`
}

type CurrentRun struct {
	ID        string       `json:"id"`
	SessionID string       `json:"session_id"`
	Snapshot  *RunSnapshot `json:"snapshot"`
}

type RunEventPage struct {
	Events                  []Event `json:"events"`
	FirstAvailableSequence  *uint   `json:"first_available_sequence"`
	LatestSequenceExclusive uint    `json:"latest_sequence_exclusive"`
	NextAfterSequence       *uint   `json:"next_after_sequence"`
	RetentionGap            bool    `json:"retention_gap"`
	HasMore                 bool    `json:"has_more"`
}

type ActiveTool struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	StartedAtMS uint64 `json:"started_at_ms"`
}

type TraceEvent struct {
	Schema       string          `json:"schema"`
	Kind         string          `json:"kind"`
	Name         string          `json:"name"`
	Success      bool            `json:"success"`
	ExitCode     int             `json:"exit_code"`
	DurationMS   uint64          `json:"duration_ms"`
	OutputBytes  uint            `json:"output_bytes"`
	MetadataKeys []string        `json:"metadata_keys"`
	ArtifactURIs []string        `json:"artifact_uris"`
	Details      json.RawMessage `json:"details,omitempty"`
}

type PendingConfirmation struct {
	ToolID      string          `json:"tool_id"`
	ToolName    string          `json:"tool_name"`
	Args        json.RawMessage `json:"args"`
	RemainingMS uint64          `json:"remaining_ms"`
}

type VerificationStatus string

const (
	VerificationPassed      VerificationStatus = "passed"
	VerificationFailed      VerificationStatus = "failed"
	VerificationNeedsReview VerificationStatus = "needs_review"
	VerificationSkipped     VerificationStatus = "skipped"
)

type VerificationCheck struct {
	ID             string             `json:"id"`
	Kind           string             `json:"kind"`
	Description    string             `json:"description"`
	Required       bool               `json:"required"`
	Status         VerificationStatus `json:"status"`
	SuggestedTools []string           `json:"suggested_tools"`
	EvidenceURIs   []string           `json:"evidence_uris"`
	ResidualRisk   *string            `json:"residual_risk,omitempty"`
}

type VerificationReport struct {
	Schema        string              `json:"schema"`
	Subject       string              `json:"subject"`
	Status        VerificationStatus  `json:"status"`
	Checks        []VerificationCheck `json:"checks"`
	ResidualRisks []string            `json:"residual_risks"`
}

type VerificationSummary struct {
	Status                    VerificationStatus `json:"status"`
	ReportCount               uint               `json:"report_count"`
	RequiredCheckCount        uint               `json:"required_check_count"`
	PendingRequiredCheckCount uint               `json:"pending_required_check_count"`
	FailedCheckCount          uint               `json:"failed_check_count"`
	ResidualRiskCount         uint               `json:"residual_risk_count"`
	PendingSubjects           []string           `json:"pending_subjects"`
	FailedSubjects            []string           `json:"failed_subjects"`
}

type VerificationCommand struct {
	ID          string  `json:"id"`
	Kind        string  `json:"kind"`
	Description string  `json:"description"`
	Command     string  `json:"command"`
	Required    bool    `json:"required"`
	TimeoutMS   *uint64 `json:"timeout_ms,omitempty"`
}

type VerificationPreset struct {
	ID          string                `json:"id"`
	ProjectKind string                `json:"project_kind"`
	Description string                `json:"description"`
	Commands    []VerificationCommand `json:"commands"`
}

type MCPTransport struct {
	Type    string            `json:"type"`
	Command string            `json:"command,omitempty"`
	Args    []string          `json:"args,omitempty"`
	URL     string            `json:"url,omitempty"`
	Headers map[string]string `json:"headers,omitempty"`
}

type MCPOAuth struct {
	AuthURL      string   `json:"auth_url"`
	TokenURL     string   `json:"token_url"`
	ClientID     string   `json:"client_id"`
	ClientSecret string   `json:"client_secret,omitempty"`
	Scopes       []string `json:"scopes,omitempty"`
	RedirectURI  string   `json:"redirect_uri"`
	AccessToken  string   `json:"access_token,omitempty"`
}

type MCPServerConfig struct {
	Name            string            `json:"name"`
	Transport       MCPTransport      `json:"transport"`
	Enabled         *bool             `json:"enabled,omitempty"`
	Env             map[string]string `json:"env,omitempty"`
	OAuth           *MCPOAuth         `json:"oauth,omitempty"`
	ToolTimeoutSecs uint64            `json:"tool_timeout_secs,omitempty"`
}

type MCPServerStatus struct {
	Name      string  `json:"name"`
	Connected bool    `json:"connected"`
	Enabled   bool    `json:"enabled"`
	ToolCount uint    `json:"tool_count"`
	Error     *string `json:"error,omitempty"`
}

type DelegateTaskOptions struct {
	Description string `json:"description"`
	Agent       string `json:"agent,omitempty"`
	Prompt      string `json:"prompt,omitempty"`
	Model       string `json:"model,omitempty"`
	MaxSteps    *uint  `json:"max_steps,omitempty"`
}

type WebSearchOptions struct {
	Query   string   `json:"query"`
	Engines []string `json:"engines,omitempty"`
	Limit   *uint    `json:"limit,omitempty"`
	Timeout *uint64  `json:"timeout,omitempty"`
	Proxy   string   `json:"proxy,omitempty"`
	Format  string   `json:"format,omitempty"`
}

type GitOptions struct {
	Command          string `json:"command"`
	Subcommand       string `json:"subcommand,omitempty"`
	Name             string `json:"name,omitempty"`
	Path             string `json:"path,omitempty"`
	NewBranch        *bool  `json:"new_branch,omitempty"`
	Base             string `json:"base,omitempty"`
	Force            *bool  `json:"force,omitempty"`
	MaxCount         *uint  `json:"max_count,omitempty"`
	Message          string `json:"message,omitempty"`
	IncludeUntracked *bool  `json:"include_untracked,omitempty"`
	Target           string `json:"target,omitempty"`
	Ref              string `json:"ref,omitempty"`
}
