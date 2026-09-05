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
pub mod core_identity;
pub mod durable_memory;
pub mod dynamic_workflow;
pub mod embedding;
pub mod error;
pub mod evaluation;
pub mod event_protocol;
pub mod execution_identity;
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
#[cfg(feature = "headless-search")]
pub mod moli_runtime;
pub mod orchestration;
pub(crate) mod ordered_parallel;
pub mod permissions;
pub mod planning;
pub mod program;
pub(crate) mod prompts;
pub mod queue;
pub mod release;
pub mod research;
pub mod retention;
pub(crate) mod retry;
pub mod rl_trajectory;
pub mod run;
pub mod run_control;
pub(crate) mod safety_gate;
pub mod sandbox;
pub mod sdk_capabilities;
#[cfg(feature = "headless-search")]
pub mod search_runtime;
pub mod security;
#[cfg(feature = "serve")]
pub mod serve;
pub mod session_checkpoint;
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
pub mod use_runtime_tasks;
pub mod verification;
pub mod workspace;

// Re-export key types at crate root for ergonomic usage
pub use agent::{AgentEvent, AgentExecutionFailure, AgentResult};
pub use agent_api::{
    Agent, AgentRunSpawn, AgentSession, ProjectedFlowHandle, ProjectedUiHandle, ReadFileOptions,
    SessionBuilder, SessionOptions, ToolCallResult,
};
pub use agent_protocol::{
    AgentProtocolChangeSetRequestV1, AgentProtocolChangeSetV1, AgentProtocolCommandActionV1,
    AgentProtocolCommandReceiptV1, AgentProtocolCommandV1, AgentProtocolError,
    AgentProtocolEventPageRequestV1, AgentProtocolEventPageV1, AgentProtocolEventRecordV1,
    AgentProtocolRunCancelV1, AgentProtocolRunIdentityV1, AgentProtocolRunRecoverExactV1,
    AgentProtocolRunRecoverV1, AgentProtocolRunStartV1, AgentProtocolRunStateV1,
    AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1, AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1,
    AGENT_PROTOCOL_CHANGE_SET_HTTP_PATH_V1, AGENT_PROTOCOL_COMMAND_HTTP_PATH_V1,
    AGENT_PROTOCOL_EVENT_PAGE_HTTP_PATH_V1, AGENT_PROTOCOL_MAX_CHANGE_SET_BYTES,
    AGENT_PROTOCOL_MAX_CHANGE_SET_RESPONSE_BYTES, AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE,
    AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES, AGENT_PROTOCOL_MAX_EVENT_PAGE_BYTES,
    AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES, AGENT_PROTOCOL_MAX_EVENT_RECORD_BYTES,
    AGENT_PROTOCOL_MAX_EVENT_TYPE_BYTES, AGENT_PROTOCOL_MAX_ID_BYTES,
    AGENT_PROTOCOL_MAX_PROMPT_BYTES, AGENT_PROTOCOL_MAX_REASON_BYTES, AGENT_PROTOCOL_V1,
};
pub use agent_protocol_harness::{
    AgentProtocolCheckpointRecoveryError, AgentProtocolHarness, AgentProtocolHarnessError,
    AGENT_PROTOCOL_HARNESS_MAX_SESSIONS,
};
pub use agent_protocol_host::{
    AgentProtocolExactRecoveryError, AgentProtocolHost, AgentProtocolHostError,
};
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
pub use core_identity::{
    ArtifactRef, CapabilityStamp, CoreEventIdentity, CoreIdentity, CoreIdentityError,
    EvidenceCursor, LogicalClock, ManualLogicalClock, OperationId, SourceRevision,
    SystemLogicalClock, CORE_EVENT_IDENTITY_DIGEST_DOMAIN_V1, CORE_EVENT_IDENTITY_SCHEMA_V1,
    CORE_EVENT_PAYLOAD_DIGEST_DOMAIN_V1, CORE_IDENTITY_MAX_ARTIFACT_BYTES,
    CORE_IDENTITY_MAX_EVENT_TYPE_BYTES, CORE_IDENTITY_MAX_ID_BYTES,
    CORE_IDENTITY_MAX_MEDIA_TYPE_BYTES, CORE_IDENTITY_MAX_PAYLOAD_BYTES, CORE_IDENTITY_SCHEMA_V1,
};
pub use durable_memory::{
    DurableMemoryActivation, DurableMemoryBindingV1, DurableMemoryMode, DurableMemoryRecallChannel,
    DurableMemoryRecallHit, DurableMemoryRecallPolicy, DurableMemoryRecallPreview,
    DurableMemorySemanticBindingV1, DurableMemorySemanticError, DurableMemorySemanticRecall,
    DurableMemorySemanticRecallPolicy, DurableMemorySemanticRefreshCheckpoint,
    DurableMemorySemanticRefreshReceipt, DurableMemorySession, DurableMemoryUse,
    DURABLE_MEMORY_BINDING_SCHEMA_VERSION, DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1,
    DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2, DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION,
    DURABLE_MEMORY_RETRIEVAL_PROFILE_V1, DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1,
    DURABLE_MEMORY_SEMANTIC_FUSION_PROFILE_V1,
    DURABLE_MEMORY_SEMANTIC_REFRESH_CHECKPOINT_SCHEMA_V1,
    DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1,
};
pub use dynamic_workflow::{
    dynamic_workflow_execution_plan, dynamic_workflow_step_identity, dynamic_workflow_store_path,
    register_dynamic_workflow_with_scheduler, DynamicWorkflowAdmissionStats,
    DynamicWorkflowRuntime, DynamicWorkflowScriptLimits, DynamicWorkflowTool,
    DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH,
};
pub use embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingError, EmbeddingExecution,
    EmbeddingExecutor, EmbeddingExecutorConfig, EmbeddingFailureKind, EmbeddingInput,
    EmbeddingNormalization, EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError,
    EmbeddingResult, EmbeddingVector,
};
pub use error::SessionBuildResource;
pub use error::{CodeError, Result};
pub use evaluation::{
    digest_bytes, digest_json, validate_digest, AuxiliaryCapabilityProfileV1, AuxiliaryExecutor,
    AuxiliaryModeV1, AuxiliaryRunContextV1, AuxiliaryRunError, AuxiliaryRunHandle,
    AuxiliaryRunOutputV1, AuxiliaryRunService, AuxiliaryRunSnapshotV1, AuxiliaryRunSpecV1,
    AuxiliaryRunStateV1, EvaluationBoundaryV1, EvaluationDispatch, EvaluationDispatchClaimOutcome,
    EvaluationDispatchLedger, EvaluationDispatchLedgerError, EvaluationDispatchOutcome,
    EvaluationPlanV1, EvaluationPolicy, EvaluationProtocolError, EvaluationRecordV1,
    EvaluationResultSink, EvaluationResultV1, EvaluationStoreError, EvaluationSupervisor,
    EvaluationWireEnvelopeV1, EvaluationWireKindDescriptorV1, EvaluationWireKindV1,
    EvaluationWireTypeV1, EvaluationWriteOutcomeV1, EventCursorV1, EvidenceArtifactV1,
    EvidenceContentModeV1, EvidenceError, EvidenceEventV1, EvidenceLimitsV1, EvidenceReadRequestV1,
    EvidenceReader, EvidenceRunStateV1, EvidenceSnapshotV1, ExecutionFactInputV1,
    ExecutionFactJournal, ExecutionFactKindV1, ExecutionFactPageV1, ExecutionFactRecorder,
    ExecutionFactSnapshotV1, ExecutionFactV1, ExecutionFrameV1, ExecutionTargetV1,
    FactAppendOutcomeV1, FileEvaluationDispatchLedger, FileEvaluationResultStore, IdentityError,
    InMemoryAuxiliaryRunService, InMemoryEvaluationDispatchLedger, InMemoryEvaluationResultStore,
    InMemoryExecutionFactJournal, JournalError, RunEvidenceReader, StructuredAuxiliaryExecutor,
    SupervisorError, AUXILIARY_MAX_OUTPUT_BYTES, AUXILIARY_MAX_STEPS, AUXILIARY_OUTPUT_SCHEMA_V1,
    AUXILIARY_RUN_SCHEMA_V1, AUXILIARY_SNAPSHOT_SCHEMA_V1, EVALUATION_DISPATCH_LEASE_GRACE_MS,
    EVALUATION_DISPATCH_LEDGER_DEFAULT_MAX_RECORDS, EVALUATION_DISPATCH_LEDGER_MAX_BYTES,
    EVALUATION_DISPATCH_LEDGER_SCHEMA_V1, EVALUATION_DISPATCH_MIN_LEASE_MS,
    EVALUATION_MAX_COOLDOWN_MS, EVALUATION_MAX_ID_BYTES, EVALUATION_MAX_PENDING,
    EVALUATION_PLAN_SCHEMA_V1, EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES,
    EVALUATION_PROTOCOL_SCHEMA_V1, EVALUATION_PROTOCOL_VERSION_V1, EVALUATION_RECORD_SCHEMA_V1,
    EVALUATION_RESULT_SCHEMA_V1, EVALUATION_RESULT_STORE_DEFAULT_MAX_RECORDS,
    EVALUATION_RESULT_STORE_MAX_BYTES, EVALUATION_RESULT_STORE_SCHEMA_V1,
    EVALUATION_WIRE_KIND_DESCRIPTORS_V1, EVIDENCE_MAX_ARTIFACTS, EVIDENCE_MAX_ARTIFACT_BYTES,
    EVIDENCE_MAX_EVENTS, EVIDENCE_MAX_EVENT_BYTES, EVIDENCE_MAX_PROMPT_BYTES,
    EVIDENCE_MAX_RESULT_BYTES, EVIDENCE_SNAPSHOT_SCHEMA_V1, EXECUTION_FACT_SCHEMA_V1,
    EXECUTION_FRAME_SCHEMA_V1, EXECUTION_TARGET_SCHEMA_V1,
};
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
    HarnessEvidenceError, ModelInputKindV1, ModelInputSnapshotV1, ModelPresentationApplicationV1,
    ModelPresentationSnapshotV1, ModelUsageSnapshotV1, RunCapabilitySnapshotV1,
    RunPolicyCeilingSnapshotV1, ToolRequestOriginV1, ToolRequestSnapshotV1,
    ToolResultContextUsageV1, WorkspaceCapabilitySnapshotV1,
    WorkspaceRetrievalCapabilitySnapshotV1, MODEL_INPUT_SNAPSHOT_V1_SCHEMA,
    MODEL_PRESENTATION_SNAPSHOT_V1_SCHEMA, MODEL_USAGE_SNAPSHOT_V1_SCHEMA,
    RUN_CAPABILITY_SNAPSHOT_V1_SCHEMA, TOOL_REQUEST_SNAPSHOT_V1_SCHEMA,
};
pub use llm::{
    clear_http_metrics_callback, set_http_metrics_callback, AnthropicClient, Attachment,
    ContentBlock, HttpMetricsCallback, HttpMetricsRecord, ImageSource, LlmClient, LlmResponse,
    Message, ModelGenerationAdmission, ModelGenerationAdmissionError, ModelGenerationConcurrency,
    ModelGenerationPermit, OpenAiClient, TokenUsage,
};
#[cfg(feature = "headless-search")]
pub use moli_runtime::{
    default_moli_version, ensure_moli, moli_runtime_info, packaged_moli, MoliRuntimeInfo,
    MOLI_RUNTIME_INFO_SCHEMA_V1,
};
pub use orchestration::{
    execute_loop, execute_pipeline, execute_steps_parallel, execute_steps_parallel_resumable,
    workflow_step_execution_identity, workflow_step_result_receipt, AgentExecutor, AgentStepSpec,
    BudgetSnapshot, LoopDecision, PipelineStage, StepOutcome, Workflow, WorkflowBudget,
    WorkflowBuilder, WorkflowCheckpoint, WorkflowEvent, WorkflowStepRecord,
    WORKFLOW_CHECKPOINT_SCHEMA_VERSION,
};
pub use prompts::{AgentStyle, DetectionConfidence, PlanningMode, SystemPromptSlots};
pub use research::{
    ResearchArtifactKindV1, ResearchContractError, ResearchEventV1, ResearchEvidenceFactKindV1,
    ResearchEvidenceFactV1, ResearchProvenanceReceiptV1, ResearchReproducibilityV1,
    ResearchReviewCategoryV1, ResearchReviewFindingV1, ResearchReviewLocationV1,
    ResearchReviewSeverityV1, ResearchReviewStatusV1, ResearchRunStatusV1, ResearchRunV1,
    RESEARCH_ARTIFACT_KINDS, RESEARCH_EVENT_SCHEMA_V1, RESEARCH_EVIDENCE_FACT_SCHEMA_V1,
    RESEARCH_PROVENANCE_RECEIPT_SCHEMA_V1, RESEARCH_REVIEW_FINDING_SCHEMA_V1,
    RESEARCH_RUN_SCHEMA_V1,
};
pub use rl_trajectory::{RlTrajectoryConfig, RlTrajectoryMode, RlTrajectoryRecorder};
pub use run::{
    ActiveToolSnapshot, InMemoryRunStore, RunEventRecord, RunHandle, RunRecord, RunReservation,
    RunSnapshot, RunStatus, RunWorkspaceChangeSet, RunWorkspaceChangeSetError,
};
pub use run_control::{
    InterruptRequest, RunControlCommand, RunControlError, RunControlErrorInfo, RunControlOperation,
    RunControlReceipt, RunControlReceiptState, RunControlRequest, RunControlSnapshot, SteerRequest,
    RUN_CONTROL_MAX_ID_BYTES, RUN_CONTROL_MAX_INPUT_BYTES, RUN_CONTROL_MAX_QUEUE,
    RUN_CONTROL_MAX_REASON_BYTES, RUN_CONTROL_MAX_SEEN_REQUESTS, RUN_CONTROL_RECEIPT_SCHEMA_V1,
    RUN_CONTROL_REQUEST_SCHEMA_V1,
};
pub use sdk_capabilities::{
    sdk_capabilities, sdk_capabilities_schema, SdkCapability, SDK_CAPABILITIES_SCHEMA_V1,
};
pub use session_checkpoint::{
    SessionCheckpointDescriptorV1, SessionCheckpointError, SessionCheckpointExportSink,
    SessionCheckpointExportV1, SessionCheckpointPayloadV1, SessionLogicalResumeEvidenceV1,
    SessionSnapshotEvidenceV1, SESSION_CHECKPOINT_DESCRIPTOR_SCHEMA_V1,
    SESSION_CHECKPOINT_ENCODING_V1, SESSION_CHECKPOINT_FORMAT_V1,
    SESSION_CHECKPOINT_LOGICAL_RESUME_SEMANTICS_V1, SESSION_CHECKPOINT_MAX_CONTENT_BYTES,
    SESSION_CHECKPOINT_MEDIA_TYPE_V1, SESSION_CHECKPOINT_PAYLOAD_SCHEMA_V1,
    SESSION_LOGICAL_RESUME_EVIDENCE_SCHEMA_V1, SESSION_SNAPSHOT_EVIDENCE_SCHEMA_V1,
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
    TaskLease, TaskPriority, TaskPriorityCounts, TaskScheduler, TaskSchedulerConfig,
    TaskSchedulerError, TaskSchedulerStats,
};
pub use tools::{
    ImmutableContentAdapter, ImmutableContentAdapterBindingV1, ImmutableContentAdapterSession,
    ImmutableContentDescriptorV1, ImmutableContentError, ImmutableContentKindV1,
    ImmutableContentReferenceV1, ImmutableContentResult, ImmutableContentWriteRequestV1,
    ToolPresentationError, ToolPresentationModeV1, ToolPresentationProfileV1,
    IMMUTABLE_CONTENT_ADAPTER_BINDING_SCHEMA_V1, IMMUTABLE_CONTENT_DESCRIPTOR_SCHEMA_V1,
    IMMUTABLE_CONTENT_REFERENCE_SCHEMA_V1, TOOL_PRESENTATION_PROFILE_V1_SCHEMA,
    TOOL_RESULT_CONTENT_MEDIA_TYPE,
};
pub use tools::{ToolCapabilities, ToolErrorKind, ToolOutputKind};
pub use use_runtime_tasks::{
    UsePlanScope, UsePlanScopeKind, UseProjectedLifecycleIdentity, UseRuntimeTaskDispatcher,
    UseRuntimeTaskError, UseRuntimeTaskExecutionV1, UseRuntimeTaskProjectionAdapter,
    UseRuntimeTaskProjectionV1, UseRuntimeTaskRequestV1, UseRuntimeTaskResult,
    MAX_USE_RUNTIME_TASK_ARGUMENTS, MAX_USE_RUNTIME_TASK_ARGUMENT_BYTES,
    MAX_USE_RUNTIME_TASK_OUTPUT_BYTES, MAX_USE_RUNTIME_TASK_TIMEOUT_MS,
    USE_RUNTIME_TASK_REQUEST_SCHEMA, USE_RUNTIME_TASK_RESULT_SCHEMA,
};
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
    WorkspaceHybridSearchResult, WorkspaceIndexError, WorkspaceLexicalEngine, WorkspacePath,
    WorkspacePathResolver, WorkspacePersistentIndex, WorkspacePersistentIndexPhase,
    WorkspacePersistentIndexStatus, WorkspaceRef, WorkspaceRerankAlgorithm,
    WorkspaceRerankFallbackReason, WorkspaceRerankMode, WorkspaceRerankOptions,
    WorkspaceRerankStatus, WorkspaceResult, WorkspaceRetrievalChannel, WorkspaceRetrievalError,
    WorkspaceRetrievalOptions, WorkspaceRetrievalPhase, WorkspaceRetrievalResult,
    WorkspaceRetrievalRuntime, WorkspaceRetrievalStatus, WorkspaceSearch,
    WorkspaceSemanticFallbackReason, WorkspaceSemanticIndexLimits, WorkspaceSemanticSearchHit,
    WorkspaceSemanticSearchRequest, WorkspaceSemanticSearchResult, WorkspaceServices,
    WorkspaceServicesBuilder, WorkspaceTextRange, WorkspaceTextReader, WorkspaceVersionConflict,
    WorkspaceWriteOutcome,
};
#[cfg(feature = "s3")]
pub use workspace::{S3BackendConfig, S3WorkspaceBackend};
