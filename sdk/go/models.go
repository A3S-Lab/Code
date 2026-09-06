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
	// Schema and ProductCapabilities are the cross-SDK product capability
	// inventory projected by Rust Core. Operations remains the low-level bridge
	// transport contract for compatibility and diagnostics.
	Schema              string              `json:"schema,omitempty"`
	ProductCapabilities []ProductCapability `json:"capabilities,omitempty"`
}

// ProductCapability is a stable, discoverable Core capability descriptor.
// HostOwned describes ownership of policy/credentials/lifecycle, not whether
// the operation is available through this SDK.
type ProductCapability struct {
	ID          string   `json:"id"`
	Category    string   `json:"category"`
	Description string   `json:"description"`
	Operations  []string `json:"operations"`
	HostOwned   bool     `json:"host_owned"`
}

// MoliRuntimeStatus is the secret-free result of a Moli runtime discovery
// request. A nil Executable means that the runtime has not been installed (or
// is not discoverable) yet; callers can call EnsureMoli to provision it.
type MoliRuntimeStatus struct {
	Schema       string  `json:"schema"`
	Version      string  `json:"version"`
	Target       *string `json:"target"`
	Executable   *string `json:"executable"`
	Packaged     bool    `json:"packaged"`
	CacheDir     *string `json:"cache_dir"`
	AutoDownload bool    `json:"auto_download"`
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

type TaskPriority string

const (
	TaskPriorityUrgent      TaskPriority = "urgent"
	TaskPriorityInteractive TaskPriority = "interactive"
	TaskPriorityForeground  TaskPriority = "foreground"
	TaskPriorityBackground  TaskPriority = "background"
	TaskPriorityMaintenance TaskPriority = "maintenance"
)

// TaskPriorityCounts reports scheduler occupancy grouped by priority class.
type TaskPriorityCounts struct {
	Urgent      uint64 `json:"urgent"`
	Interactive uint64 `json:"interactive"`
	Foreground  uint64 `json:"foreground"`
	Background  uint64 `json:"background"`
	Maintenance uint64 `json:"maintenance"`
}

// TaskSchedulerStats is a point-in-time snapshot of the scheduler shared by
// every session created from one Agent.
type TaskSchedulerStats struct {
	MaxActive         uint64             `json:"maxActive"`
	Active            uint64             `json:"active"`
	Pending           uint64             `json:"pending"`
	ActiveByPriority  TaskPriorityCounts `json:"activeByPriority"`
	PendingByPriority TaskPriorityCounts `json:"pendingByPriority"`
	Closed            bool               `json:"closed"`
}

// TaskSchedulerHealthSnapshot contains the current scheduler occupancy plus
// bounded cumulative admission and fairness counters. It never includes task
// labels, execution identities, or queued payloads.
type TaskSchedulerHealthSnapshot struct {
	MaxActive         uint64             `json:"maxActive"`
	Active            uint64             `json:"active"`
	Pending           uint64             `json:"pending"`
	ActiveByPriority  TaskPriorityCounts `json:"activeByPriority"`
	PendingByPriority TaskPriorityCounts `json:"pendingByPriority"`
	Admitted          uint64             `json:"admitted"`
	Released          uint64             `json:"released"`
	Cancelled         uint64             `json:"cancelled"`
	Rejected          uint64             `json:"rejected"`
	AgingPromotions   uint64             `json:"agingPromotions"`
	PeakActive        uint64             `json:"peakActive"`
	TotalWaitMicros   uint64             `json:"totalWaitMicros"`
	AverageWaitMicros uint64             `json:"averageWaitMicros"`
	MaxWaitMicros     uint64             `json:"maxWaitMicros"`
	Closed            bool               `json:"closed"`
}

// ExecutionIdentityV1 is the digest-only identity used by provider-pool
// diagnostics. It contains no provider labels, credentials, or request data.
type ExecutionIdentityV1 struct {
	Schema string `json:"schema"`
	Domain string `json:"domain"`
	Digest string `json:"digest"`
}

// ModelGenerationPool describes one provider/model capacity budget.
type ModelGenerationPool struct {
	Identity       ExecutionIdentityV1 `json:"identity"`
	MaxConcurrency uint64              `json:"maxConcurrency"`
}

// TaskSchedulerQuotaHealthSnapshot contains bounded health for one provider
// quota identity and its current or most-recent idle epoch.
type TaskSchedulerQuotaHealthSnapshot struct {
	Identity          ExecutionIdentityV1 `json:"identity"`
	MaxActive         uint64              `json:"maxActive"`
	Observed          bool                `json:"observed"`
	Live              bool                `json:"live"`
	Active            uint64              `json:"active"`
	Pending           uint64              `json:"pending"`
	Blocked           bool                `json:"blocked"`
	Admitted          uint64              `json:"admitted"`
	Released          uint64              `json:"released"`
	Cancelled         uint64              `json:"cancelled"`
	Rejected          uint64              `json:"rejected"`
	PeakActive        uint64              `json:"peakActive"`
	TotalWaitMicros   uint64              `json:"totalWaitMicros"`
	AverageWaitMicros uint64              `json:"averageWaitMicros"`
	MaxWaitMicros     uint64              `json:"maxWaitMicros"`
}

// ModelGenerationPoolHealthSnapshot composes local gate occupancy with the
// optional shared scheduler projection for one session.
type ModelGenerationPoolHealthSnapshot struct {
	Pool                ModelGenerationPool               `json:"pool"`
	LocalMaxConcurrency uint64                            `json:"localMaxConcurrency"`
	LocalReserved       uint64                            `json:"localReserved"`
	LocalAvailable      uint64                            `json:"localAvailable"`
	Scheduler           *TaskSchedulerQuotaHealthSnapshot `json:"scheduler,omitempty"`
}

// DefaultSecurityProvider enables Core's built-in taint tracking and output
// sanitization. Its concrete type is the provider selection; callers do not
// pass a raw backend name.
type DefaultSecurityProvider struct{}

// NewDefaultSecurityProvider constructs the built-in security provider spec.
func NewDefaultSecurityProvider() *DefaultSecurityProvider {
	return &DefaultSecurityProvider{}
}

// BrowserBackend selects the browser used for JavaScript-rendered search
// engines. Moli has been the default since A3S Code 8.1.0.
type BrowserBackend string

const (
	BrowserBackendMoli       BrowserBackend = "moli"
	BrowserBackendChrome     BrowserBackend = "chrome"
	BrowserBackendLightpanda BrowserBackend = "lightpanda"
)

// SearchEngineConfig controls one a3s-search engine.
type SearchEngineConfig struct {
	Enabled *bool    `json:"enabled,omitempty"`
	Weight  *float64 `json:"weight,omitempty"`
	Timeout *uint64  `json:"timeout,omitempty"`
}

// SearchHealthConfig controls temporary engine suspension after failures.
type SearchHealthConfig struct {
	MaxFailures    *uint32 `json:"maxFailures,omitempty"`
	SuspendSeconds *uint64 `json:"suspendSeconds,omitempty"`
}

// HeadlessConfig controls the JavaScript-capable browser used by web_search.
// Moli is provisioned from the package sidecar or a shared per-user cache;
// multiple Code processes reuse the same verified installation.
type HeadlessConfig struct {
	Backend                 BrowserBackend `json:"backend,omitempty"`
	MaxTabs                 *uint          `json:"maxTabs,omitempty"`
	BrowserPath             string         `json:"browserPath,omitempty"`
	AutoDownloadMoli        *bool          `json:"autoDownloadMoli,omitempty"`
	MoliVersion             string         `json:"moliVersion,omitempty"`
	MoliSHA256              string         `json:"moliSha256,omitempty"`
	MoliCacheDir            string         `json:"moliCacheDir,omitempty"`
	MoliDownloadTimeoutSecs *uint64        `json:"moliDownloadTimeoutSecs,omitempty"`
	LaunchArgs              []string       `json:"launchArgs,omitempty"`
	ProxyURL                string         `json:"proxyUrl,omitempty"`
}

// SearchConfig is the value-shaped per-session a3s-search configuration.
// The Engines field is encoded as the core wire key "engine" for compatibility
// with ACL and the Rust configuration type.
type SearchConfig struct {
	Timeout  *uint64                       `json:"timeout,omitempty"`
	Health   *SearchHealthConfig           `json:"health,omitempty"`
	Engines  map[string]SearchEngineConfig `json:"engine,omitempty"`
	Headless *HeadlessConfig               `json:"headless,omitempty"`
}

// NewMoliHeadlessConfig returns an explicit configuration using the bundled
// or shared-cache Moli runtime.
func NewMoliHeadlessConfig() *HeadlessConfig {
	return &HeadlessConfig{Backend: BrowserBackendMoli}
}

// SessionOptions contains the same value-shaped session configuration exposed
// by the Rust, TypeScript, and Python SDKs.
type SessionOptions struct {
	Model                              string              `json:"model,omitempty"`
	TaskPriority                       TaskPriority        `json:"task_priority,omitempty"`
	BuiltinSkills                      *bool               `json:"builtin_skills,omitempty"`
	AgentDirs                          []string            `json:"agent_dirs,omitempty"`
	SkillDirs                          []string            `json:"skill_dirs,omitempty"`
	WorkerAgents                       []WorkerAgentSpec   `json:"worker_agents,omitempty"`
	QueueConfig                        *SessionQueueConfig `json:"queue_config,omitempty"`
	SearchConfig                       *SearchConfig       `json:"search_config,omitempty"`
	PermissionPolicy                   *PermissionPolicy   `json:"permission_policy,omitempty"`
	ConfirmationPolicy                 *ConfirmationPolicy `json:"confirmation_policy,omitempty"`
	EnforceActiveSkillToolRestrictions *bool               `json:"enforce_active_skill_tool_restrictions,omitempty"`
	FileMemoryDir                      string              `json:"file_memory_dir,omitempty"`
	FileSessionStoreDir                string              `json:"file_session_store_dir,omitempty"`
	// SecurityProvider selects the typed security boundary for this session.
	SecurityProvider *DefaultSecurityProvider `json:"security_provider,omitempty"`
	// Deprecated: use SecurityProvider: NewDefaultSecurityProvider().
	DefaultSecurity            *bool                      `json:"default_security,omitempty"`
	WorkspaceBackend           *WorkspaceBackendConfig    `json:"workspace_backend,omitempty"`
	RemoteGit                  *RemoteGitBackendConfig    `json:"remote_git,omitempty"`
	WorkspaceRetrieval         *WorkspaceRetrievalOptions `json:"-"`
	SessionID                  string                     `json:"session_id,omitempty"`
	TenantID                   string                     `json:"tenant_id,omitempty"`
	Principal                  string                     `json:"principal,omitempty"`
	AgentTemplateID            string                     `json:"agent_template_id,omitempty"`
	CorrelationID              string                     `json:"correlation_id,omitempty"`
	HostEnv                    *HostEnvConfig             `json:"host_env,omitempty"`
	PlanningMode               PlanningMode               `json:"planning_mode,omitempty"`
	GoalTracking               *bool                      `json:"goal_tracking,omitempty"`
	AutoSave                   *bool                      `json:"auto_save,omitempty"`
	MaxParseRetries            *uint32                    `json:"max_parse_retries,omitempty"`
	ToolTimeoutMS              *uint64                    `json:"tool_timeout_ms,omitempty"`
	LLMAPITimeoutMS            *uint64                    `json:"llm_api_timeout_ms,omitempty"`
	CircuitBreakerThreshold    *uint32                    `json:"circuit_breaker_threshold,omitempty"`
	DuplicateToolCallThreshold *uint32                    `json:"duplicate_tool_call_threshold,omitempty"`
	AutoCompact                *bool                      `json:"auto_compact,omitempty"`
	AutoCompactThreshold       *float32                   `json:"auto_compact_threshold,omitempty"`
	MaxContextTokens           *uint                      `json:"max_context_tokens,omitempty"`
	ArtifactStoreLimits        *ArtifactStoreLimits       `json:"artifact_store_limits,omitempty"`
	ToolResultTransformPolicy  *ToolResultTransformPolicy `json:"tool_result_transform_policy,omitempty"`
	ToolPresentationProfile    *ToolPresentationProfile   `json:"tool_presentation_profile,omitempty"`
	ContinuationEnabled        *bool                      `json:"continuation_enabled,omitempty"`
	MaxContinuationTurns       *uint32                    `json:"max_continuation_turns,omitempty"`
	Temperature                *float32                   `json:"temperature,omitempty"`
	ThinkingBudget             *uint                      `json:"thinking_budget,omitempty"`
	MaxToolRounds              *uint                      `json:"max_tool_rounds,omitempty"`
	MaxParallelTasks           *uint                      `json:"max_parallel_tasks,omitempty"`
	AutoDelegationEnabled      *bool                      `json:"auto_delegation_enabled,omitempty"`
	AutoDelegation             *AutoDelegationConfig      `json:"auto_delegation,omitempty"`
	ManualDelegationEnabled    *bool                      `json:"manual_delegation_enabled,omitempty"`
	AutoParallelDelegation     *bool                      `json:"auto_parallel_delegation,omitempty"`
	LLMLogprobs                *bool                      `json:"llm_logprobs,omitempty"`
	LLMTopLogprobs             *uint                      `json:"llm_top_logprobs,omitempty"`
	MaxExecutionTimeMS         *uint64                    `json:"max_execution_time_ms,omitempty"`
	RetentionLimits            *RetentionLimits           `json:"retention_limits,omitempty"`
	Trajectory                 *TrajectoryConfig          `json:"trajectory,omitempty"`
	InlineSkills               []InlineSkill              `json:"inline_skills,omitempty"`
	PromptSlots                *PromptSlots               `json:"prompt_slots,omitempty"`
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

// CapabilityCatalogStamp identifies the exact live Core capability generation.
type CapabilityCatalogStamp struct {
	Generation uint64 `json:"generation"`
	Digest     string `json:"digest"`
}

// CapabilityCleanupReport is the bounded result of retiring host capability
// effects.
type CapabilityCleanupReport struct {
	RollbackBatches uint `json:"rollback_batches"`
	RetiredBatches  uint `json:"retired_batches"`
	EffectsClosed   uint `json:"effects_closed"`
	EffectsFailed   uint `json:"effects_failed"`
	EffectsTimedOut uint `json:"effects_timed_out"`
	Clean           bool `json:"clean"`
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

// CognitiveContextLimits freezes the bounded prompt-injection surface of one
// exact cognitive package binding.
type CognitiveContextLimits struct {
	MaxResults       uint `json:"maxResults"`
	MaxDocumentBytes uint `json:"maxDocumentBytes"`
	MaxTotalBytes    uint `json:"maxTotalBytes"`
}

// CognitiveKnowledgeBindingV1 identifies one exact Knowledge surface owned by
// the embedding Knowledge host.
type CognitiveKnowledgeBindingV1 struct {
	Schema              string `json:"schema"`
	SurfaceID           string `json:"surfaceId"`
	FormatVersion       string `json:"formatVersion"`
	ContentDigest       string `json:"contentDigest"`
	SearchSchema        string `json:"searchSchema"`
	ReadSchema          string `json:"readSchema"`
	CitationSchema      string `json:"citationSchema"`
	LifecycleGeneration uint64 `json:"lifecycleGeneration"`
	GenerationDigest    string `json:"generationDigest"`
}

// CognitivePackageBindingV1 is the secret-free exact cognitive authority
// recorded on an admitted Run.
type CognitivePackageBindingV1 struct {
	Schema                   string                      `json:"schema"`
	PackageID                string                      `json:"packageId"`
	PackageVersion           string                      `json:"packageVersion"`
	LifecycleGeneration      uint64                      `json:"lifecycleGeneration"`
	GenerationDigest         string                      `json:"generationDigest"`
	CapabilitySnapshotDigest string                      `json:"capabilitySnapshotDigest"`
	Knowledge                CognitiveKnowledgeBindingV1 `json:"knowledge"`
	Limits                   CognitiveContextLimits      `json:"limits"`
}

type RunSnapshot struct {
	ID                      string                     `json:"id"`
	SessionID               string                     `json:"session_id"`
	Status                  RunStatus                  `json:"status"`
	Prompt                  string                     `json:"prompt"`
	CognitivePackageBinding *CognitivePackageBindingV1 `json:"cognitive_package_binding,omitempty"`
	CreatedAtMS             uint64                     `json:"created_at_ms"`
	UpdatedAtMS             uint64                     `json:"updated_at_ms"`
	ResultText              *string                    `json:"result_text,omitempty"`
	Error                   *string                    `json:"error,omitempty"`
	EventCount              uint                       `json:"event_count"`
}

// RunSpawn is the result of admitting an exact host-selected run ID.
// Replayed is true when the compatible run already existed and no duplicate
// worker was started.
type RunSpawn struct {
	Snapshot RunSnapshot `json:"snapshot"`
	Replayed bool        `json:"replayed"`
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

// ToolResultTransformPolicy is the exact versioned projection policy pinned
// to a session and enforced unchanged across resume.
type ToolResultTransformPolicy struct {
	Schema                string `json:"schema"`
	MaxOutputBytes        uint   `json:"max_output_bytes"`
	HeadBytes             uint   `json:"head_bytes"`
	TailBytes             uint   `json:"tail_bytes"`
	FoldRepeatedLines     bool   `json:"fold_repeated_lines"`
	RepeatedLineThreshold uint   `json:"repeated_line_threshold"`
	StructuredSampleItems uint   `json:"structured_sample_items"`
}

// ToolPresentationProfileV1Schema identifies the closed version-1 model-facing
// Tool presentation contract.
const ToolPresentationProfileV1Schema = "a3s.code.tool-presentation-profile.v1"

// ToolPresentationMode is a closed model-facing presentation choice.
type ToolPresentationMode string

const (
	ToolPresentationAdaptive ToolPresentationMode = "adaptive"
	ToolPresentationDirect   ToolPresentationMode = "direct"
	ToolPresentationCode     ToolPresentationMode = "code"
	ToolPresentationDisabled ToolPresentationMode = "disabled"
)

// ToolPresentationProfile changes only definitions submitted to the model. It
// does not select an A3S Use generation or replace the governed Tool executor.
type ToolPresentationProfile struct {
	Schema string               `json:"schema"`
	Mode   ToolPresentationMode `json:"mode"`
}

// NewToolPresentationProfile constructs a version-1 typed profile.
func NewToolPresentationProfile(mode ToolPresentationMode) *ToolPresentationProfile {
	return &ToolPresentationProfile{
		Schema: ToolPresentationProfileV1Schema,
		Mode:   mode,
	}
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
