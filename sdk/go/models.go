package code

import (
	"context"
	"encoding/json"
	"time"
)

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

// HostEnvConfig makes framework-generated IDs and timestamps reproducible.
// Set both fields for deterministic replay; nil fields keep system defaults.
type HostEnvConfig struct {
	SequentialIDPrefix *string `json:"sequential_id_prefix,omitempty"`
	FixedTimeMS        *uint64 `json:"fixed_time_ms,omitempty"`
}

// SessionOptions contains the same value-shaped session configuration exposed
// by the Rust, TypeScript, and Python SDKs.
type SessionOptions struct {
	Model                              string                  `json:"model,omitempty"`
	BuiltinSkills                      *bool                   `json:"builtin_skills,omitempty"`
	AgentDirs                          []string                `json:"agent_dirs,omitempty"`
	SkillDirs                          []string                `json:"skill_dirs,omitempty"`
	WorkerAgents                       []WorkerAgentSpec       `json:"worker_agents,omitempty"`
	QueueConfig                        *SessionQueueConfig     `json:"queue_config,omitempty"`
	PermissionPolicy                   *PermissionPolicy       `json:"permission_policy,omitempty"`
	ConfirmationPolicy                 *ConfirmationPolicy     `json:"confirmation_policy,omitempty"`
	EnforceActiveSkillToolRestrictions *bool                   `json:"enforce_active_skill_tool_restrictions,omitempty"`
	FileMemoryDir                      string                  `json:"file_memory_dir,omitempty"`
	FileSessionStoreDir                string                  `json:"file_session_store_dir,omitempty"`
	DefaultSecurity                    *bool                   `json:"default_security,omitempty"`
	WorkspaceBackend                   *WorkspaceBackendConfig `json:"workspace_backend,omitempty"`
	RemoteGit                          *RemoteGitBackendConfig `json:"remote_git,omitempty"`
	SessionID                          string                  `json:"session_id,omitempty"`
	TenantID                           string                  `json:"tenant_id,omitempty"`
	Principal                          string                  `json:"principal,omitempty"`
	AgentTemplateID                    string                  `json:"agent_template_id,omitempty"`
	CorrelationID                      string                  `json:"correlation_id,omitempty"`
	HostEnv                            *HostEnvConfig          `json:"host_env,omitempty"`
	PlanningMode                       PlanningMode            `json:"planning_mode,omitempty"`
	GoalTracking                       *bool                   `json:"goal_tracking,omitempty"`
	AutoSave                           *bool                   `json:"auto_save,omitempty"`
	MaxParseRetries                    *uint32                 `json:"max_parse_retries,omitempty"`
	ToolTimeoutMS                      *uint64                 `json:"tool_timeout_ms,omitempty"`
	LLMAPITimeoutMS                    *uint64                 `json:"llm_api_timeout_ms,omitempty"`
	CircuitBreakerThreshold            *uint32                 `json:"circuit_breaker_threshold,omitempty"`
	DuplicateToolCallThreshold         *uint32                 `json:"duplicate_tool_call_threshold,omitempty"`
	AutoCompact                        *bool                   `json:"auto_compact,omitempty"`
	AutoCompactThreshold               *float32                `json:"auto_compact_threshold,omitempty"`
	MaxContextTokens                   *uint                   `json:"max_context_tokens,omitempty"`
	ArtifactStoreLimits                *ArtifactStoreLimits    `json:"artifact_store_limits,omitempty"`
	ContinuationEnabled                *bool                   `json:"continuation_enabled,omitempty"`
	MaxContinuationTurns               *uint32                 `json:"max_continuation_turns,omitempty"`
	Temperature                        *float32                `json:"temperature,omitempty"`
	ThinkingBudget                     *uint                   `json:"thinking_budget,omitempty"`
	MaxToolRounds                      *uint                   `json:"max_tool_rounds,omitempty"`
	MaxParallelTasks                   *uint                   `json:"max_parallel_tasks,omitempty"`
	AutoDelegationEnabled              *bool                   `json:"auto_delegation_enabled,omitempty"`
	AutoDelegation                     *AutoDelegationConfig   `json:"auto_delegation,omitempty"`
	ManualDelegationEnabled            *bool                   `json:"manual_delegation_enabled,omitempty"`
	AutoParallelDelegation             *bool                   `json:"auto_parallel_delegation,omitempty"`
	LLMLogprobs                        *bool                   `json:"llm_logprobs,omitempty"`
	LLMTopLogprobs                     *uint                   `json:"llm_top_logprobs,omitempty"`
	MaxExecutionTimeMS                 *uint64                 `json:"max_execution_time_ms,omitempty"`
	RetentionLimits                    *RetentionLimits        `json:"retention_limits,omitempty"`
	Trajectory                         *TrajectoryConfig       `json:"trajectory,omitempty"`
	InlineSkills                       []InlineSkill           `json:"inline_skills,omitempty"`
	PromptSlots                        *PromptSlots            `json:"prompt_slots,omitempty"`
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

// Attachment is an image supplied with a multimodal prompt. Data is encoded as
// base64 by encoding/json and decoded by the native bridge.
type Attachment struct {
	Data      []byte `json:"data"`
	MediaType string `json:"media_type"`
}

type SessionRequest struct {
	Prompt      string       `json:"prompt"`
	History     []Message    `json:"history,omitempty"`
	Attachments []Attachment `json:"attachments,omitempty"`
}

type AgentStepSpec struct {
	TaskID          string          `json:"task_id"`
	Agent           string          `json:"agent"`
	Description     string          `json:"description"`
	Prompt          string          `json:"prompt"`
	MaxSteps        *uint           `json:"max_steps,omitempty"`
	ParentSessionID string          `json:"parent_session_id,omitempty"`
	OutputSchema    json.RawMessage `json:"output_schema,omitempty"`
}

type ToolSourceAnchor struct {
	Tool      string `json:"tool"`
	URLOrPath string `json:"url_or_path"`
}

type StepOutcome struct {
	TaskID        string             `json:"task_id"`
	SessionID     string             `json:"session_id"`
	Agent         string             `json:"agent"`
	Output        string             `json:"output"`
	Success       bool               `json:"success"`
	Structured    json.RawMessage    `json:"structured,omitempty"`
	SourceAnchors []ToolSourceAnchor `json:"source_anchors"`
}

type WorkflowBudget struct {
	ConsumedTokens uint64  `json:"consumed_tokens"`
	LimitTokens    *uint64 `json:"limit_tokens,omitempty"`
}

type ParallelResult struct {
	Outcomes []StepOutcome   `json:"outcomes"`
	Budget   *WorkflowBudget `json:"budget,omitempty"`
}

type PermissionPolicy struct {
	Deny            []string `json:"deny,omitempty"`
	Allow           []string `json:"allow,omitempty"`
	Ask             []string `json:"ask,omitempty"`
	DefaultDecision string   `json:"default_decision,omitempty"`
	Enabled         *bool    `json:"enabled,omitempty"`
}

type ConfirmationPolicy struct {
	Enabled          *bool    `json:"enabled,omitempty"`
	DefaultTimeoutMS *uint64  `json:"default_timeout_ms,omitempty"`
	TimeoutAction    string   `json:"timeout_action,omitempty"`
	YoloLanes        []string `json:"yolo_lanes,omitempty"`
}

type WorkerAgentSpec struct {
	Name                    string            `json:"name"`
	Description             string            `json:"description"`
	Kind                    string            `json:"kind,omitempty"`
	Hidden                  bool              `json:"hidden,omitempty"`
	Permissions             *PermissionPolicy `json:"permissions,omitempty"`
	Model                   string            `json:"model,omitempty"`
	Prompt                  string            `json:"prompt,omitempty"`
	MaxSteps                *uint             `json:"max_steps,omitempty"`
	ConfirmationInheritance string            `json:"confirmation_inheritance,omitempty"`
}

type AgentDefinition struct {
	Name                    string           `json:"name"`
	Description             string           `json:"description"`
	Native                  bool             `json:"native"`
	Hidden                  bool             `json:"hidden"`
	Permissions             PermissionPolicy `json:"permissions"`
	Model                   string           `json:"model,omitempty"`
	Prompt                  string           `json:"prompt,omitempty"`
	MaxSteps                *uint            `json:"max_steps,omitempty"`
	ToolFree                bool             `json:"tool_free"`
	ConfirmationInheritance string           `json:"confirmation_inheritance,omitempty"`
}

type InlineSkill struct {
	Name    string `json:"name"`
	Kind    string `json:"kind,omitempty"`
	Content string `json:"content"`
}

type AutoDelegationConfig struct {
	Enabled       *bool    `json:"enabled,omitempty"`
	AutoParallel  *bool    `json:"auto_parallel,omitempty"`
	MinConfidence *float32 `json:"min_confidence,omitempty"`
	MaxTasks      *uint    `json:"max_tasks,omitempty"`
}

type ArtifactStoreLimits struct {
	MaxArtifacts uint `json:"max_artifacts"`
	MaxBytes     uint `json:"max_bytes"`
}

type RetentionLimits struct {
	Unbounded                bool  `json:"unbounded,omitempty"`
	MaxRunsRetained          *uint `json:"max_runs_retained,omitempty"`
	MaxEventsPerRun          *uint `json:"max_events_per_run,omitempty"`
	MaxEventBytesPerRun      *uint `json:"max_event_bytes_per_run,omitempty"`
	MaxTraceEvents           *uint `json:"max_trace_events,omitempty"`
	MaxTerminalSubagentTasks *uint `json:"max_terminal_subagent_tasks,omitempty"`
}

type TrajectoryConfig struct {
	Path            string `json:"path"`
	Mode            string `json:"mode,omitempty"`
	MaxTextBytes    *uint  `json:"max_text_bytes,omitempty"`
	IncludeMessages *bool  `json:"include_messages,omitempty"`
}

type WorkspaceBackendConfig struct {
	Kind string           `json:"kind"`
	Root string           `json:"root,omitempty"`
	S3   *S3BackendConfig `json:"s3,omitempty"`
}

type S3BackendConfig struct {
	Endpoint              string  `json:"endpoint,omitempty"`
	Region                string  `json:"region,omitempty"`
	AccessKeyID           string  `json:"access_key_id"`
	SecretAccessKey       string  `json:"secret_access_key"`
	SessionToken          string  `json:"session_token,omitempty"`
	Bucket                string  `json:"bucket"`
	Prefix                string  `json:"prefix"`
	ForcePathStyle        *bool   `json:"force_path_style,omitempty"`
	RequestTimeoutMS      *uint64 `json:"request_timeout_ms,omitempty"`
	MaxReadBytes          *uint64 `json:"max_read_bytes,omitempty"`
	SearchEnabled         *bool   `json:"search_enabled,omitempty"`
	MaxObjectsScanned     *uint   `json:"max_objects_scanned,omitempty"`
	MaxGrepBytesPerObject *uint64 `json:"max_grep_bytes_per_object,omitempty"`
	SearchConcurrency     *uint   `json:"search_concurrency,omitempty"`
}

type RemoteGitBackendConfig struct {
	BaseURL          string  `json:"base_url"`
	RepoID           string  `json:"repo_id"`
	BearerToken      string  `json:"bearer_token,omitempty"`
	ClientCertPEM    string  `json:"client_cert_pem,omitempty"`
	ClientKeyPEM     string  `json:"client_key_pem,omitempty"`
	RequestTimeoutMS *uint64 `json:"request_timeout_ms,omitempty"`
	MaxDiffBytes     *uint64 `json:"max_diff_bytes,omitempty"`
	MaxLogEntries    *uint   `json:"max_log_entries,omitempty"`
}

type SubagentStatus string

const (
	SubagentRunning   SubagentStatus = "running"
	SubagentCompleted SubagentStatus = "completed"
	SubagentFailed    SubagentStatus = "failed"
	SubagentCancelled SubagentStatus = "cancelled"
)

type SubagentProgress struct {
	TimestampMS uint64          `json:"timestamp_ms"`
	Status      string          `json:"status"`
	Metadata    json.RawMessage `json:"metadata"`
}

type SubagentTask struct {
	TaskID          string             `json:"task_id"`
	ParentSessionID string             `json:"parent_session_id"`
	ChildSessionID  string             `json:"child_session_id"`
	Agent           string             `json:"agent"`
	Description     string             `json:"description"`
	Status          SubagentStatus     `json:"status"`
	StartedMS       uint64             `json:"started_ms"`
	UpdatedMS       uint64             `json:"updated_ms"`
	FinishedMS      *uint64            `json:"finished_ms,omitempty"`
	Output          *string            `json:"output,omitempty"`
	Success         *bool              `json:"success,omitempty"`
	SourceAnchors   []ToolSourceAnchor `json:"source_anchors"`
	Progress        []SubagentProgress `json:"progress"`
}

type MemoryItem struct {
	ID           string            `json:"id"`
	Content      string            `json:"content"`
	Timestamp    string            `json:"timestamp"`
	Importance   float32           `json:"importance"`
	Tags         []string          `json:"tags"`
	MemoryType   string            `json:"memory_type"`
	Metadata     map[string]string `json:"metadata"`
	AccessCount  uint32            `json:"access_count"`
	LastAccessed *string           `json:"last_accessed,omitempty"`
}

type MemoryStats struct {
	LongTermCount  uint `json:"long_term_count"`
	ShortTermCount uint `json:"short_term_count"`
	WorkingCount   uint `json:"working_count"`
}

type SessionLane string

const (
	LaneControl  SessionLane = "control"
	LaneQuery    SessionLane = "query"
	LaneExecute  SessionLane = "execute"
	LaneGenerate SessionLane = "generate"
)

type LaneHandlerConfig struct {
	Mode      string `json:"mode"`
	TimeoutMS uint64 `json:"timeout_ms,omitempty"`
}

type SessionQueueConfig struct {
	ControlConcurrency  *uint                             `json:"control_concurrency,omitempty"`
	QueryConcurrency    *uint                             `json:"query_concurrency,omitempty"`
	ExecuteConcurrency  *uint                             `json:"execute_concurrency,omitempty"`
	GenerateConcurrency *uint                             `json:"generate_concurrency,omitempty"`
	LaneHandlers        map[SessionLane]LaneHandlerConfig `json:"lane_handlers,omitempty"`
	EnableDLQ           *bool                             `json:"enable_dlq,omitempty"`
	DLQMaxSize          *uint                             `json:"dlq_max_size,omitempty"`
	EnableMetrics       *bool                             `json:"enable_metrics,omitempty"`
	EnableAlerts        *bool                             `json:"enable_alerts,omitempty"`
	TimeoutMS           *uint64                           `json:"timeout_ms,omitempty"`
	StoragePath         string                            `json:"storage_path,omitempty"`
	EnableAllFeatures   *bool                             `json:"enable_all_features,omitempty"`
}

type ExternalTask struct {
	TaskID      string          `json:"task_id"`
	SessionID   string          `json:"session_id"`
	Lane        string          `json:"lane"`
	CommandType string          `json:"command_type"`
	Payload     json.RawMessage `json:"payload"`
	TimeoutMS   uint64          `json:"timeout_ms"`
}

type ExternalTaskResult struct {
	Success bool            `json:"success"`
	Result  json.RawMessage `json:"result"`
	Error   string          `json:"error,omitempty"`
}

type LaneStatus struct {
	Lane           string `json:"lane"`
	Pending        uint   `json:"pending"`
	Active         uint   `json:"active"`
	MaxConcurrency uint   `json:"max_concurrency"`
	HandlerMode    string `json:"handler_mode"`
}

type QueueStats struct {
	TotalPending    uint                  `json:"total_pending"`
	TotalActive     uint                  `json:"total_active"`
	ExternalPending uint                  `json:"external_pending"`
	Lanes           map[string]LaneStatus `json:"lanes"`
}

type HistogramStats struct {
	Count uint64  `json:"count"`
	Sum   float64 `json:"sum"`
	Min   float64 `json:"min"`
	Max   float64 `json:"max"`
	Mean  float64 `json:"mean"`
	P50   float64 `json:"p50"`
	P90   float64 `json:"p90"`
	P95   float64 `json:"p95"`
	P99   float64 `json:"p99"`
}

type QueueMetrics struct {
	Counters   map[string]uint64         `json:"counters"`
	Gauges     map[string]float64        `json:"gauges"`
	Histograms map[string]HistogramStats `json:"histograms"`
}

type DeadLetter struct {
	CommandID   string `json:"command_id"`
	CommandType string `json:"command_type"`
	LaneID      string `json:"lane_id"`
	Error       string `json:"error"`
	Attempts    uint32 `json:"attempts"`
	FailedAt    string `json:"failed_at"`
}

type HookMatcher struct {
	Tool           string `json:"tool,omitempty"`
	PathPattern    string `json:"path_pattern,omitempty"`
	CommandPattern string `json:"command_pattern,omitempty"`
	SessionID      string `json:"session_id,omitempty"`
	Skill          string `json:"skill,omitempty"`
}

type HookConfig struct {
	Priority       *int    `json:"priority,omitempty"`
	TimeoutMS      *uint64 `json:"timeout_ms,omitempty"`
	AsyncExecution *bool   `json:"async_execution,omitempty"`
	MaxRetries     *uint32 `json:"max_retries,omitempty"`
}

type Hook struct {
	ID        string       `json:"id"`
	EventType string       `json:"event_type"`
	Matcher   *HookMatcher `json:"matcher,omitempty"`
	Config    *HookConfig  `json:"config,omitempty"`
}

type CommandInfo struct {
	Name        string  `json:"name"`
	Description string  `json:"description"`
	Usage       *string `json:"usage,omitempty"`
}

type HookResponse struct {
	Action   string `json:"action"`
	Reason   string `json:"reason,omitempty"`
	Modified any    `json:"modified,omitempty"`
	DelayMS  uint64 `json:"delay_ms,omitempty"`
}

type HookHandler func(context.Context, json.RawMessage) (*HookResponse, error)

type BudgetDecision struct {
	Decision string  `json:"decision"`
	Resource string  `json:"resource,omitempty"`
	Consumed float64 `json:"consumed,omitempty"`
	Limit    float64 `json:"limit,omitempty"`
	Message  string  `json:"message,omitempty"`
	Reason   string  `json:"reason,omitempty"`
}

type BudgetLLMContext struct {
	SessionID       string `json:"session_id"`
	EstimatedTokens uint   `json:"estimated_tokens"`
}

type BudgetToolContext struct {
	SessionID string `json:"session_id"`
	ToolName  string `json:"tool_name"`
}

type BudgetUsageContext struct {
	SessionID string     `json:"session_id"`
	Usage     TokenUsage `json:"usage"`
}

type BudgetGuardHandlers struct {
	CheckBeforeLLM  func(context.Context, BudgetLLMContext) (*BudgetDecision, error)
	RecordAfterLLM  func(context.Context, BudgetUsageContext) error
	CheckBeforeTool func(context.Context, BudgetToolContext) (*BudgetDecision, error)
	Timeout         time.Duration
}

type CommandContext struct {
	SessionID   string             `json:"session_id"`
	Workspace   string             `json:"workspace"`
	Model       string             `json:"model"`
	HistoryLen  uint               `json:"history_len"`
	TotalTokens uint64             `json:"total_tokens"`
	TotalCost   float64            `json:"total_cost"`
	ToolNames   []string           `json:"tool_names"`
	MCPServers  []CommandMCPServer `json:"mcp_servers"`
}

type CommandMCPServer struct {
	Name      string `json:"name"`
	ToolCount uint   `json:"tool_count"`
}

type CommandHandler func(context.Context, string, CommandContext) (string, error)
