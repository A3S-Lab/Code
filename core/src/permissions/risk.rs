//! Structured risk vocabulary for interactive tool guardrails.
//!
//! Risk is deliberately not represented by a single opaque score. Each
//! assessment records the tool type, operation target, blast radius,
//! reversibility, and environment sensitivity, then assigns one of four levels.
//! Interactive hosts use the level-to-action matrix in `InteractiveApprovalMode`.

use serde::{Deserialize, Serialize};

/// Coarse risk level for one tool invocation.
///
/// Levels are ordered from least to most restrictive so composite tools can
/// inherit the highest-risk child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevel {
    /// Read-only, bounded, and understood well enough to run without friction.
    Routine,
    /// A reversible mutation confined to the current workspace.
    Bounded,
    /// Context-dependent, unbounded, external, or otherwise ambiguous execution.
    High,
    /// A deterministic safety-boundary violation that no mode or model may bypass.
    Critical,
}

/// Routing action selected from a risk assessment and interactive mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskAction {
    /// Execute without a confirmation prompt.
    Allow,
    /// A deterministic policy requires explicit human approval.
    RequireConfirmation,
    /// Ask a constrained LLM reviewer whether to allow, escalate, or deny.
    ReviewByLlm,
    /// Reject by deterministic rule; an LLM review cannot override this action.
    RuleDeny,
}

/// Primary capability exercised by a tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskType {
    ReadOnly,
    WorkspaceMutation,
    CommandExecution,
    VersionControl,
    NetworkRead,
    Delegation,
    DynamicExecution,
    ExternalIntegration,
    Composite,
    Unknown,
}

/// Object or boundary targeted by an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationTarget {
    Workspace,
    OutsideWorkspace,
    PublicNetwork,
    ExternalService,
    HostEnvironment,
    PrivilegedSystem,
    Multiple,
    Unknown,
}

/// Maximum expected blast radius if the invocation succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactScope {
    Observation,
    SingleResource,
    Workspace,
    ExternalService,
    Host,
    SystemWide,
    Unknown,
}

/// How readily the direct effects can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    NotApplicable,
    Easy,
    Conditional,
    Difficult,
    Irreversible,
    Unknown,
}

/// Ambient execution environment that can affect the operation's safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentSensitivity {
    None,
    Workspace,
    Network,
    Host,
    Credentials,
    Privileged,
    Mixed,
    Unknown,
}

/// Stable, non-secret reason codes explaining a classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskReason {
    KnownReadOnly,
    BoundedWorkspaceMutation,
    BoundedVersionControlMutation,
    UnboundedCommandExecution,
    DynamicOrDelegatedExecution,
    ExternalOrUnknownIntegration,
    EnvironmentSensitiveOperation,
    ForceOrDestructiveOperation,
    MalformedOrUnknownOperation,
    WorkspaceBoundaryEscape,
    SymlinkBoundaryEscape,
    CatastrophicOperation,
    CompositeInvocation,
}

/// The five independent dimensions used to explain a risk level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRiskDimensions {
    pub tool_type: ToolRiskType,
    pub operation_target: OperationTarget,
    pub impact_scope: ImpactScope,
    pub reversibility: Reversibility,
    pub environment_sensitivity: EnvironmentSensitivity,
}

impl ToolRiskDimensions {
    pub const fn new(
        tool_type: ToolRiskType,
        operation_target: OperationTarget,
        impact_scope: ImpactScope,
        reversibility: Reversibility,
        environment_sensitivity: EnvironmentSensitivity,
    ) -> Self {
        Self {
            tool_type,
            operation_target,
            impact_scope,
            reversibility,
            environment_sensitivity,
        }
    }
}

/// Explainable risk assessment produced before tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRiskAssessment {
    pub level: ToolRiskLevel,
    pub dimensions: ToolRiskDimensions,
    pub reasons: Vec<ToolRiskReason>,
}

impl ToolRiskAssessment {
    pub fn new(
        level: ToolRiskLevel,
        dimensions: ToolRiskDimensions,
        reasons: impl IntoIterator<Item = ToolRiskReason>,
    ) -> Self {
        let mut deduplicated = Vec::new();
        for reason in reasons {
            if !deduplicated.contains(&reason) {
                deduplicated.push(reason);
            }
        }
        Self {
            level,
            dimensions,
            reasons: deduplicated,
        }
    }

    /// Conservative action before a host applies mode-specific streamlining.
    ///
    /// Auto mode may promote only `Bounded` to `Allow`. `High` always remains
    /// an LLM-review candidate and `Critical` always remains a rule denial.
    pub const fn baseline_action(&self) -> ToolRiskAction {
        match self.level {
            ToolRiskLevel::Routine => ToolRiskAction::Allow,
            ToolRiskLevel::Bounded => ToolRiskAction::RequireConfirmation,
            ToolRiskLevel::High => ToolRiskAction::ReviewByLlm,
            ToolRiskLevel::Critical => ToolRiskAction::RuleDeny,
        }
    }
}
