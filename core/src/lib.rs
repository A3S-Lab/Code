//! A3S Code Core Library
//!
//! Harness-driven runtime for coding agents.
//!
//! `Agent` and `AgentSession` are the primary 2.0 API. Lower-level session
//! runtime state is internal; persistence data flows through `store::SessionData`.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use a3s_code_core::{Agent, AgentEvent};
//!
//! # async fn run() -> anyhow::Result<()> {
//! // From an ACL-compatible config file path (.acl)
//! let agent = Agent::new("agent.acl").await?;
//!
//! // Create a workspace-bound session
//! let session = agent.session_async("/my-project", None).await?;
//!
//! // Non-streaming
//! let result = session.send("What files handle auth?", None).await?;
//! println!("{}", result.text);
//!
//! // Streaming (AgentEvent is #[non_exhaustive])
//! let (mut rx, _handle) = session.stream("Refactor auth", None).await?;
//! while let Some(event) = rx.recv().await {
//!     match event {
//!         AgentEvent::TextDelta { text } => print!("{text}"),
//!         AgentEvent::End { .. } => break,
//!         _ => {} // required: #[non_exhaustive]
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Disposable Workers
//!
//! ```rust,no_run
//! use a3s_code_core::{Agent, SessionOptions, WorkerAgentSpec};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let agent = Agent::new("agent.acl").await?;
//! let frontend = WorkerAgentSpec::implementer(
//!     "frontend-cow",
//!     "Small verified frontend fixes",
//! )
//! .with_model_ref("openai/gpt-4o")
//! .with_max_steps(24);
//!
//! let session = agent.session_async(
//!     "/my-project",
//!     Some(SessionOptions::new().with_worker_agent(frontend)),
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! ```text
//! Agent (config-driven facade)
//!   +-- AgentSession (workspace-bound execution API)
//!       +-- internal turn runner
//!       +-- ContextAssembler / ContextProvider
//!       +-- ToolSelector
//!       +-- ToolExecutor
//!       +-- ProgramExecutor (PTC)
//!       +-- SkillRegistry
//!       +-- Permission / confirmation
//!       +-- Trace / artifacts / verification evidence
//!
//! Advanced infrastructure:
//!   +-- optional lane queues for explicit external/hybrid dispatch
//! ```

pub(crate) mod agent;
pub(crate) mod agent_api;
pub mod agent_protocol;
pub mod agent_protocol_harness;
pub mod agent_protocol_host;
pub mod budget;
pub mod capability;
pub(crate) mod child_run;
pub mod code_intelligence;
pub mod cognitive_context;
pub mod commands;
pub(crate) mod compaction;
pub mod config;
pub mod context;
pub mod dynamic_workflow;
pub mod embedding;
pub mod error;
pub mod event_protocol;
pub mod flow_graph;
pub(crate) mod git;
pub mod harness_evidence;
pub mod hitl;
pub mod hooks;
pub mod host_env;
pub(crate) mod language;
pub mod llm;
pub mod loop_checkpoint;
pub mod mcp;
pub mod memory;
pub mod orchestration;
pub(crate) mod ordered_parallel;
pub mod permissions;
pub mod planning;
pub mod program;
pub(crate) mod prompts;
pub mod queue;
pub mod release;
pub mod retention;
pub(crate) mod retry;
pub mod rl_trajectory;
pub mod run;
pub(crate) mod safety_gate;
pub mod sandbox;
#[cfg(feature = "headless-search")]
pub mod search_runtime;
pub mod security;
#[cfg(feature = "serve")]
pub mod serve;
pub(crate) mod session_lane_queue;
pub mod skills;
pub(crate) mod sse;
pub mod state_graph;
pub mod store;
pub mod subagent;
pub mod subagent_task_tracker;
pub mod task_scheduler;
pub mod telemetry;
#[cfg(feature = "telemetry")]
pub mod telemetry_otel;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod text;
pub(crate) mod tool_confirmation;
pub mod tools;
pub mod trace;
pub mod verification;
pub mod workspace;

// Re-export key types at crate root for ergonomic usage
pub use agent::{AgentEvent, AgentResult};
pub use agent_api::{
    Agent, AgentRunSpawn, AgentSession, ReadFileOptions, SessionBuilder, SessionOptions,
    ToolCallResult,
};
pub use agent_protocol::{
    AgentProtocolChangeSetRequestV1, AgentProtocolChangeSetV1, AgentProtocolCommandActionV1,
    AgentProtocolCommandReceiptV1, AgentProtocolCommandV1, AgentProtocolError,
    AgentProtocolEventPageRequestV1, AgentProtocolEventPageV1, AgentProtocolEventRecordV1,
    AgentProtocolRunCancelV1, AgentProtocolRunIdentityV1, AgentProtocolRunRecoverV1,
    AgentProtocolRunStartV1, AgentProtocolRunStateV1, AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1,
    AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1, AGENT_PROTOCOL_CHANGE_SET_HTTP_PATH_V1,
    AGENT_PROTOCOL_COMMAND_HTTP_PATH_V1, AGENT_PROTOCOL_EVENT_PAGE_HTTP_PATH_V1,
    AGENT_PROTOCOL_MAX_CHANGE_SET_BYTES, AGENT_PROTOCOL_MAX_CHANGE_SET_RESPONSE_BYTES,
    AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE, AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES,
    AGENT_PROTOCOL_MAX_EVENT_PAGE_BYTES, AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES,
    AGENT_PROTOCOL_MAX_EVENT_RECORD_BYTES, AGENT_PROTOCOL_MAX_EVENT_TYPE_BYTES,
    AGENT_PROTOCOL_MAX_ID_BYTES, AGENT_PROTOCOL_MAX_PROMPT_BYTES, AGENT_PROTOCOL_MAX_REASON_BYTES,
    AGENT_PROTOCOL_V1,
};
pub use agent_protocol_harness::{
    AgentProtocolHarness, AgentProtocolHarnessError, AGENT_PROTOCOL_HARNESS_MAX_SESSIONS,
};
pub use agent_protocol_host::{AgentProtocolHost, AgentProtocolHostError};
pub use code_intelligence::{
    CodeDiagnostic, CodeDiagnosticSeverity, CodeIntelligenceCapabilities, CodeIntelligenceError,
    CodeIntelligenceLanguageStatus, CodeIntelligenceResult, CodeIntelligenceState,
    CodeIntelligenceStatus, CodeLocation, CodePosition, CodeQueryResult, CodeRange, CodeSymbolKind,
    DocumentRevision, DocumentSnapshot, DocumentSymbol, LanguageId, LocalCodeIntelligence,
    NavigationKind, SymbolInformation, WorkspaceCodeIntelligence,
};
pub use cognitive_context::{
    CognitiveContextDocumentV1, CognitiveContextError, CognitiveContextLimits,
    CognitiveContextProvider, CognitiveContextRequestV1, CognitiveContextResponseV1,
    CognitiveContextResult, CognitiveContextSession, CognitiveKnowledgeBindingV1,
    CognitiveKnowledgeCitationV1, CognitivePackageBindingV1,
    COGNITIVE_CONTEXT_REQUEST_DIGEST_DOMAIN, COGNITIVE_CONTEXT_REQUEST_SCHEMA,
    COGNITIVE_CONTEXT_RESPONSE_SCHEMA, COGNITIVE_KNOWLEDGE_BINDING_SCHEMA,
    COGNITIVE_PACKAGE_BINDING_SCHEMA, OKF_KNOWLEDGE_CITATION_SCHEMA,
    OKF_KNOWLEDGE_READ_REQUEST_SCHEMA, OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA,
};
pub use config::{
    AutoDelegationConfig, CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities,
    OsConfig, ProviderConfig,
};
pub use dynamic_workflow::{
    dynamic_workflow_store_path, DynamicWorkflowRuntime, DynamicWorkflowScriptLimits,
    DynamicWorkflowTool, DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH,
};
pub use embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingError, EmbeddingExecution,
    EmbeddingExecutor, EmbeddingExecutorConfig, EmbeddingFailureKind, EmbeddingInput,
    EmbeddingNormalization, EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError,
    EmbeddingResult, EmbeddingVector,
};
pub use error::SessionBuildResource;
pub use error::{CodeError, Result};
pub use event_protocol::{
    run_event_envelope_v1, AgentEventProjectionV1, AgentEventTypeV1, EventEnvelopeV1,
    EventProtocolError, AGENT_EVENT_TYPES_V1, EVENT_ENVELOPE_V1_VERSION,
};
pub use flow_graph::{
    run_object_id as flow_run_object_id, step_object_id as flow_step_object_id,
    FileFlowDecisionLedger, FlowDecision, FlowDecisionClaimOutcome, FlowDecisionDispatchError,
    FlowDecisionDispatcher, FlowDecisionHealthSnapshot, FlowDecisionHealthStatus,
    FlowDecisionLedger, FlowDecisionRequest, FlowDecisionSink, FlowDecisionStep,
    FlowGraphHealthSnapshot, FlowGraphHealthStatus, FlowGraphObserver, MemoryFlowDecisionLedger,
    FLOW_GRAPH_SOURCE,
};
pub use harness_evidence::{
    HarnessEvidenceError, ModelInputKindV1, ModelInputSnapshotV1, ModelUsageSnapshotV1,
    RunCapabilitySnapshotV1, RunPolicyCeilingSnapshotV1, ToolResultContextUsageV1,
    WorkspaceCapabilitySnapshotV1, WorkspaceRetrievalCapabilitySnapshotV1,
    MODEL_INPUT_SNAPSHOT_V1_SCHEMA, MODEL_USAGE_SNAPSHOT_V1_SCHEMA,
    RUN_CAPABILITY_SNAPSHOT_V1_SCHEMA,
};
pub use llm::{
    clear_http_metrics_callback, set_http_metrics_callback, AnthropicClient, Attachment,
    ContentBlock, HttpMetricsCallback, HttpMetricsRecord, ImageSource, LlmClient, LlmResponse,
    Message, ModelGenerationAdmission, ModelGenerationAdmissionError, ModelGenerationConcurrency,
    ModelGenerationPermit, OpenAiClient, TokenUsage,
};
pub use orchestration::{
    execute_loop, execute_pipeline, execute_steps_parallel, execute_steps_parallel_resumable,
    AgentExecutor, AgentStepSpec, BudgetSnapshot, LoopDecision, PipelineStage, StepOutcome,
    Workflow, WorkflowBudget, WorkflowBuilder, WorkflowCheckpoint, WorkflowEvent,
    WorkflowStepRecord, WORKFLOW_CHECKPOINT_SCHEMA_VERSION,
};
pub use prompts::{AgentStyle, DetectionConfidence, PlanningMode, SystemPromptSlots};
pub use rl_trajectory::{RlTrajectoryConfig, RlTrajectoryMode, RlTrajectoryRecorder};
pub use run::{
    ActiveToolSnapshot, InMemoryRunStore, RunEventRecord, RunHandle, RunRecord, RunReservation,
    RunSnapshot, RunStatus, RunWorkspaceChangeSet, RunWorkspaceChangeSetError,
};
pub use state_graph::{
    graph_event_head, Behavior, BehaviorContext, BehaviorError, EventFilter, ExternalEvent,
    ExternalProjectionOutcome, FileGraphEventStore, FnBehavior, GraphDiff, GraphEvent,
    GraphEventRecord, GraphEventStore, GraphObject, GraphPatch, GraphRelation, GraphRuntime,
    GraphSaveOutcome, MemoryGraphEventStore, ObjectId, PatchOperation, RelationId, ReplayError,
    RuntimeError as GraphRuntimeError, RuntimeLimits, StateGraph, GRAPH_EVENT_SCHEMA_VERSION,
};
pub use subagent::{
    AgentDefinition, AgentRegistry, CattleAgentKind, CattleAgentSpec, ConfirmationInheritance,
    WorkerAgentKind, WorkerAgentSpec,
};
pub use subagent_task_tracker::{
    InMemorySubagentTaskTracker, SubagentProgressEntry, SubagentStatus, SubagentTaskSnapshot,
};
pub use task_scheduler::{
    TaskPriority, TaskPriorityCounts, TaskScheduler, TaskSchedulerConfig, TaskSchedulerError,
    TaskSchedulerStats,
};
pub use tools::{ToolCapabilities, ToolErrorKind, ToolOutputKind};
pub use workspace::{
    ChunkCatalogLimits, ChunkCatalogSnapshot, ChunkingConfig, CommandOutput, CommandOutputObserver,
    CommandOutputSummary, CommandRequest, CustomWorkspaceChunkingStrategy,
    FixedWindowChunkingOptions, LexicalSearchHit, LexicalSearchRequest, LexicalSearchResult,
    LocalWorkspaceAccessPolicy, LocalWorkspaceBackend, LocalWorkspaceFile,
    LocalWorkspaceFileStatus, LocalWorkspaceManifest, LocalWorkspaceManifestSnapshot,
    ManifestWorkspaceBackend, RecentWorkspaceFile, RecursiveChunkingOptions, RemoteGitBackend,
    RemoteGitBackendConfig, RemoteGitConflict, VirtualPathResolver, WorkspaceCapabilities,
    WorkspaceChunk, WorkspaceChunkCatalog, WorkspaceChunkId, WorkspaceChunkRange,
    WorkspaceChunkingError, WorkspaceChunkingInput, WorkspaceChunkingStrategy,
    WorkspaceCommandRunner, WorkspaceDirEntry, WorkspaceEligibilityPolicy,
    WorkspaceEmbeddingBatchMetrics, WorkspaceError, WorkspaceFileChange, WorkspaceFileChangeKind,
    WorkspaceFileSystem, WorkspaceFileSystemExt, WorkspaceFileType, WorkspaceGit,
    WorkspaceGitBranch, WorkspaceGitCheckoutOutput, WorkspaceGitCheckoutRequest,
    WorkspaceGitCommit, WorkspaceGitCreateBranchRequest, WorkspaceGitCreateWorktreeRequest,
    WorkspaceGitDiffRequest, WorkspaceGitRemote, WorkspaceGitRemoveWorktreeRequest,
    WorkspaceGitStash, WorkspaceGitStashProvider, WorkspaceGitStashRequest, WorkspaceGitStatus,
    WorkspaceGitWorktree, WorkspaceGitWorktreeMutation, WorkspaceGitWorktreeProvider,
    WorkspaceGlobRequest, WorkspaceGlobResult, WorkspaceGrepOutcome, WorkspaceGrepRequest,
    WorkspaceGrepResult, WorkspaceHybridChannelRank, WorkspaceHybridChannelStatus,
    WorkspaceHybridFallbackReason, WorkspaceHybridSearchHit, WorkspaceHybridSearchRequest,
    WorkspaceHybridSearchResult, WorkspaceIndexError, WorkspacePath, WorkspacePathResolver,
    WorkspaceRef, WorkspaceRerankAlgorithm, WorkspaceRerankFallbackReason, WorkspaceRerankMode,
    WorkspaceRerankOptions, WorkspaceRerankStatus, WorkspaceResult, WorkspaceRetrievalChannel,
    WorkspaceRetrievalError, WorkspaceRetrievalOptions, WorkspaceRetrievalPhase,
    WorkspaceRetrievalResult, WorkspaceRetrievalRuntime, WorkspaceRetrievalStatus, WorkspaceSearch,
    WorkspaceSemanticFallbackReason, WorkspaceSemanticIndexLimits, WorkspaceSemanticSearchHit,
    WorkspaceSemanticSearchRequest, WorkspaceSemanticSearchResult, WorkspaceServices,
    WorkspaceServicesBuilder, WorkspaceTextRange, WorkspaceTextReader, WorkspaceVersionConflict,
    WorkspaceWriteOutcome,
};
#[cfg(feature = "s3")]
pub use workspace::{S3BackendConfig, S3WorkspaceBackend};
