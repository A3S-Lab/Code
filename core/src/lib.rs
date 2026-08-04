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
pub mod budget;
pub(crate) mod child_run;
pub mod code_intelligence;
pub mod commands;
pub(crate) mod compaction;
pub mod config;
pub mod context;
pub mod dynamic_workflow;
pub mod error;
pub mod event_protocol;
pub mod flow_graph;
pub(crate) mod git;
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
pub mod telemetry;
#[cfg(feature = "telemetry")]
pub mod telemetry_otel;
pub(crate) mod text;
pub(crate) mod tool_confirmation;
pub mod tools;
pub mod trace;
pub mod verification;
pub mod workspace;

// Re-export key types at crate root for ergonomic usage
pub use agent::{AgentEvent, AgentResult};
pub use agent_api::{
    Agent, AgentSession, ReadFileOptions, SessionBuilder, SessionOptions, ToolCallResult,
};
pub use agent_protocol::{
    AgentProtocolCommandActionV1, AgentProtocolCommandReceiptV1, AgentProtocolCommandV1,
    AgentProtocolError, AgentProtocolEventPageV1, AgentProtocolEventRecordV1,
    AgentProtocolRunCancelV1, AgentProtocolRunIdentityV1, AgentProtocolRunRecoverV1,
    AgentProtocolRunStartV1, AgentProtocolRunStateV1, AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE,
    AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES, AGENT_PROTOCOL_MAX_EVENT_PAGE_BYTES,
    AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES, AGENT_PROTOCOL_MAX_EVENT_TYPE_BYTES,
    AGENT_PROTOCOL_MAX_ID_BYTES, AGENT_PROTOCOL_MAX_PROMPT_BYTES, AGENT_PROTOCOL_MAX_REASON_BYTES,
    AGENT_PROTOCOL_V1,
};
pub use code_intelligence::{
    CodeDiagnostic, CodeDiagnosticSeverity, CodeIntelligenceCapabilities, CodeIntelligenceError,
    CodeIntelligenceLanguageStatus, CodeIntelligenceResult, CodeIntelligenceState,
    CodeIntelligenceStatus, CodeLocation, CodePosition, CodeQueryResult, CodeRange, CodeSymbolKind,
    DocumentRevision, DocumentSnapshot, DocumentSymbol, LanguageId, LocalCodeIntelligence,
    NavigationKind, SymbolInformation, WorkspaceCodeIntelligence,
};
pub use config::{
    AutoDelegationConfig, CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities,
    OsConfig, ProviderConfig,
};
pub use dynamic_workflow::{
    dynamic_workflow_store_path, DynamicWorkflowRuntime, DynamicWorkflowScriptLimits,
    DynamicWorkflowTool, DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH,
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
    ActiveToolSnapshot, InMemoryRunStore, RunEventRecord, RunHandle, RunRecord, RunSnapshot,
    RunStatus,
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
pub use tools::{ToolCapabilities, ToolErrorKind, ToolOutputKind};
pub use workspace::{
    CommandOutput, CommandOutputObserver, CommandOutputSummary, CommandRequest,
    LocalWorkspaceAccessPolicy, LocalWorkspaceBackend, LocalWorkspaceFile,
    LocalWorkspaceFileStatus, LocalWorkspaceManifest, LocalWorkspaceManifestSnapshot,
    ManifestWorkspaceBackend, RecentWorkspaceFile, RemoteGitBackend, RemoteGitBackendConfig,
    RemoteGitConflict, VirtualPathResolver, WorkspaceCapabilities, WorkspaceCommandRunner,
    WorkspaceDirEntry, WorkspaceError, WorkspaceFileChange, WorkspaceFileChangeKind,
    WorkspaceFileSystem, WorkspaceFileSystemExt, WorkspaceFileType, WorkspaceGit,
    WorkspaceGitBranch, WorkspaceGitCheckoutOutput, WorkspaceGitCheckoutRequest,
    WorkspaceGitCommit, WorkspaceGitCreateBranchRequest, WorkspaceGitCreateWorktreeRequest,
    WorkspaceGitDiffRequest, WorkspaceGitRemote, WorkspaceGitRemoveWorktreeRequest,
    WorkspaceGitStash, WorkspaceGitStashProvider, WorkspaceGitStashRequest, WorkspaceGitStatus,
    WorkspaceGitWorktree, WorkspaceGitWorktreeMutation, WorkspaceGitWorktreeProvider,
    WorkspaceGlobRequest, WorkspaceGlobResult, WorkspaceGrepOutcome, WorkspaceGrepRequest,
    WorkspaceGrepResult, WorkspacePath, WorkspacePathResolver, WorkspaceRef, WorkspaceResult,
    WorkspaceSearch, WorkspaceServices, WorkspaceServicesBuilder, WorkspaceTextRange,
    WorkspaceTextReader, WorkspaceVersionConflict, WorkspaceWriteOutcome,
};
#[cfg(feature = "s3")]
pub use workspace::{S3BackendConfig, S3WorkspaceBackend};
