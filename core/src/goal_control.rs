//! Durable Goal associations and bounded inputs; execution stays with the original Run.

use crate::execution_identity::{ExecutionIdentityError, ExecutionIdentityV1};
use crate::planning::{AgentGoal, ExecutionPlan, TaskStatus};
use crate::workspace::WorkspaceRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

// Keep wire spellings, serde and explicit parsing on the same closed catalog.
macro_rules! closed_set {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }

            pub fn parse(value: &str) -> Result<Self, GoalControlError> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    _ => Err(GoalControlError::UnknownClosedSet {
                        set: stringify!($name), value: value.to_owned()
                    }),
                }
            }
        }

        impl TryFrom<String> for $name {
            type Error = GoalControlError;
            fn try_from(value: String) -> Result<Self, Self::Error> { Self::parse(&value) }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self { value.as_str().to_owned() }
        }
    };
}

closed_set!(GoalState {
    Active => "active", Waiting => "waiting", Paused => "paused",
    Completed => "completed", Cancelled => "cancelled",
});

closed_set!(GoalTickDecisionV1 {
    Advance => "advance", Wait => "wait", NeedsInput => "needs_input",
    Paused => "paused", BudgetExhausted => "budget_exhausted",
    Reconcile => "reconcile", Complete => "complete",
});

closed_set!(GoalCommandKindV1 {
    Create => "create", Revise => "revise", ProposePlan => "propose_plan",
    AcceptPlan => "accept_plan", Pause => "pause", Resume => "resume",
    Cancel => "cancel", Tick => "tick", DeclareWait => "declare_wait",
    ObserveWait => "observe_wait",
});

closed_set!(GoalWaitKindV1 {
    QuestionAnswer => "question_answer", ConfirmationAnswer => "confirmation_answer",
    SourceRevision => "source_revision", UtcDue => "utc_due",
});

closed_set!(GoalRefusal {
    NotFound => "goal.not_found", Conflict => "goal.conflict",
    WriterUnavailable => "goal.writer_unavailable", Waiting => "goal.waiting",
    Paused => "goal.paused", BudgetExhausted => "goal.budget_exhausted",
    DeadlineExpired => "goal.deadline_expired", ReceiptUnsettled => "goal.receipt_unsettled",
    EvidenceStale => "goal.evidence_stale", ConditionUnsupported => "goal.condition_unsupported",
    LegacyAmbiguous => "goal.legacy_ambiguous", HostDriverConflict => "goal.host_driver_conflict",
});

mod continuation;
mod facts;
pub(crate) use continuation::preview_tick;
pub use continuation::GoalContinuationPolicyV1;
#[cfg(test)]
pub(crate) use continuation::MAX_GOAL_SLICES;
#[cfg(test)]
pub(crate) use facts::slice_unsettled;
pub(crate) use facts::{
    check_step_run, closes_goal, definition_from_facts, prepare_completion, prepare_control,
    prepare_definition, prepare_plan, prepare_run, prepare_settlement, prepare_step, prepare_wait,
    reconcile_slices, refuse_unbound_goal_execution, render_run_context, validate_step_append,
    ContinuationSource, PreparedDefinition,
};
pub use facts::{goal_fact_key, GoalFactKindV1, GOAL_FACT_SCHEMA_V1};

pub const GOAL_LEGACY_SOURCE_DOMAIN_V1: &str = "a3s.code.goal-legacy-source.v1";

/// Original source identity and pause posture; never an execution entitlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalImportSourceV1 {
    pub identity: ExecutionIdentityV1,
    pub paused: bool,
}

/// Explicit host input. Expected references are absent only for creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalDefinitionCommandV1 {
    pub kind: GoalCommandKindV1,
    pub operation_id: String,
    pub goal_id: String,
    pub session_id: String,
    pub workspace: WorkspaceRef,
    #[serde(deserialize_with = "deserialize_goal_definition")]
    pub goal: AgentGoal,
    pub non_goals: Vec<String>,
    pub scope_fact_sequences: Vec<u64>,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub work_policy_digest: Option<String>,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    pub expected_goal_revision: Option<u32>,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    pub expected_plan_revision: Option<u32>,
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    pub expected_goal_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<GoalImportSourceV1>,
}

/// A plan proposal carries original content; acceptance carries only its source sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPlanCommandV1 {
    pub kind: GoalCommandKindV1,
    pub operation_id: String,
    pub goal_id: String,
    pub session_id: String,
    pub workspace: WorkspaceRef,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub work_policy_digest: Option<String>,
    pub expected_goal_revision: u32,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    pub expected_plan_revision: Option<u32>,
    pub expected_goal_sequence: u64,
    #[serde(deserialize_with = "deserialize_plan")]
    pub plan: Option<ExecutionPlan>,
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    pub proposal_sequence: Option<u64>,
}

/// Explicit state control, referencing the original definition without copying its content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalControlCommandV1 {
    pub kind: GoalCommandKindV1,
    pub operation_id: String,
    pub goal_id: String,
    pub session_id: String,
    pub workspace: WorkspaceRef,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub work_policy_digest: Option<String>,
    pub expected_goal_revision: u32,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    pub expected_plan_revision: Option<u32>,
    pub expected_goal_sequence: u64,
}

/// One step-scoped condition. Readiness after it is observed never approves or accepts progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalWaitConditionV1 {
    QuestionAnswer {
        question: String,
    },
    ConfirmationAnswer {
        request: String,
    },
    /// Satisfied by a host observation of `subject` whose digest differs from `after_digest`.
    SourceRevision {
        subject: String,
        after_digest: String,
    },
    /// Satisfied only by the host clock; callers never supply time.
    UtcDue {
        due_unix_ms: std::num::NonZeroU64,
    },
}

impl GoalWaitConditionV1 {
    pub fn kind(&self) -> GoalWaitKindV1 {
        match self {
            Self::QuestionAnswer { .. } => GoalWaitKindV1::QuestionAnswer,
            Self::ConfirmationAnswer { .. } => GoalWaitKindV1::ConfirmationAnswer,
            Self::SourceRevision { .. } => GoalWaitKindV1::SourceRevision,
            Self::UtcDue { .. } => GoalWaitKindV1::UtcDue,
        }
    }

    fn validate(&self) -> Result<(), GoalControlError> {
        match self {
            Self::QuestionAnswer { question: text }
            | Self::ConfirmationAnswer { request: text } => validate_text("wait.text", text, 4096),
            Self::SourceRevision {
                subject,
                after_digest,
            } => {
                validate_id("wait.subject", subject)?;
                validate_id("wait.after_digest", after_digest)
            }
            Self::UtcDue { .. } => Ok(()),
        }
    }
}

/// Host evidence for one open wait. `utc_due` carries no value: Core reads its own clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalWaitEvidenceV1 {
    QuestionAnswer {
        answer: String,
    },
    ConfirmationAnswer {
        approved: bool,
    },
    SourceRevision {
        observation: crate::external_observation::ExternalObservationV1,
    },
    UtcDue {},
}

impl GoalWaitEvidenceV1 {
    pub fn kind(&self) -> GoalWaitKindV1 {
        match self {
            Self::QuestionAnswer { .. } => GoalWaitKindV1::QuestionAnswer,
            Self::ConfirmationAnswer { .. } => GoalWaitKindV1::ConfirmationAnswer,
            Self::SourceRevision { .. } => GoalWaitKindV1::SourceRevision,
            Self::UtcDue {} => GoalWaitKindV1::UtcDue,
        }
    }
}

/// Declare or observe one step-scoped wait under the original Goal references.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalWaitCommandV1 {
    pub kind: GoalCommandKindV1,
    pub operation_id: String,
    pub goal_id: String,
    pub session_id: String,
    pub workspace: WorkspaceRef,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub work_policy_digest: Option<String>,
    pub expected_goal_revision: u32,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    pub expected_plan_revision: Option<u32>,
    pub expected_goal_sequence: u64,
    pub step_id: String,
    #[serde(deserialize_with = "Option::<GoalWaitConditionV1>::deserialize")]
    pub condition: Option<GoalWaitConditionV1>,
    #[serde(deserialize_with = "Option::<GoalWaitEvidenceV1>::deserialize")]
    pub evidence: Option<GoalWaitEvidenceV1>,
}

/// A projected wait; `observed` is the single admitted observation, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalWaitV1 {
    pub step_id: String,
    pub declared_sequence: u64,
    pub condition: GoalWaitConditionV1,
    pub observed: Option<GoalWaitObservationV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalWaitObservationV1 {
    pub sequence: u64,
    pub evidence: GoalWaitEvidenceV1,
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    pub observed_unix_ms: Option<u64>,
}

/// `recorded=false` is a quiet observation: not due or unchanged; nothing was appended.
#[derive(Debug, Clone, Serialize)]
pub struct GoalWaitResultV1 {
    pub recorded: bool,
    pub definition: DurableGoalDefinitionV1,
}

impl DurableGoalDefinitionV1 {
    fn open_wait(&self, step_id: &str) -> Option<&GoalWaitV1> {
        self.waits
            .iter()
            .find(|wait| wait.step_id == step_id && wait.observed.is_none())
    }

    /// Original ready steps minus steps gated by an open wait.
    pub fn ready_steps(&self) -> Vec<&crate::planning::Task> {
        self.plan.as_ref().map_or_else(Vec::new, |plan| {
            plan.get_ready_steps()
                .into_iter()
                .filter(|step| self.open_wait(&step.id).is_none())
                .collect()
        })
    }
}

fn deserialize_plan<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ExecutionPlan>, D::Error> {
    let wire = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(wire) = wire else {
        return Ok(None);
    };
    let plan = ExecutionPlan::deserialize(&wire).map_err(serde::de::Error::custom)?;
    let mut fields = serde_json::to_value(&plan).map_err(serde::de::Error::custom)?;
    // Original serde omits empty optional fields; retain their names for boundary checking.
    fields["execution_context"] = serde_json::Value::Null;
    if let Some(steps) = fields["steps"].as_array_mut() {
        for step in steps {
            step["tool"] = serde_json::Value::Null;
            step["dependencies"] = serde_json::Value::Null;
            step["success_criteria"] = serde_json::Value::Null;
        }
    }
    let known_fields = |input: &serde_json::Value, known: &serde_json::Value| {
        input
            .as_object()
            .zip(known.as_object())
            .is_some_and(|(input, known)| input.keys().all(|key| known.contains_key(key)))
    };
    if !known_fields(&wire, &fields)
        || !wire["steps"]
            .as_array()
            .zip(fields["steps"].as_array())
            .is_some_and(|(input, known)| {
                input
                    .iter()
                    .zip(known)
                    .all(|(input, known)| known_fields(input, known))
            })
    {
        return Err(serde::de::Error::custom(
            "unknown durable plan or task field",
        ));
    }
    validate_goal_plan(&plan).map_err(serde::de::Error::custom)?;
    Ok(Some(plan))
}

/// A projection of original definition facts, never an execution or completion grant.
#[derive(Debug, Clone, Serialize)]
pub struct DurableGoalDefinitionV1 {
    pub binding: DurableGoalBindingV1,
    pub state: GoalState,
    pub import_source: Option<GoalImportSourceV1>,
    pub goal: AgentGoal,
    pub goal_sequence: u64,
    pub plan_revision: Option<u32>,
    pub plan: Option<ExecutionPlan>,
    pub plan_fact_sequence: Option<u64>,
    pub plan_accepted: bool,
    /// Waits for the current plan revision, in declaration order.
    pub waits: Vec<GoalWaitV1>,
}

fn deserialize_goal_definition<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<AgentGoal, D::Error> {
    let wire = serde_json::Value::deserialize(deserializer)?;
    let goal = AgentGoal::deserialize(&wire).map_err(serde::de::Error::custom)?;
    let canonical = serde_json::to_value(&goal).map_err(serde::de::Error::custom)?;
    let exact_fields = match (wire.as_object(), canonical.as_object()) {
        (Some(input), Some(expected)) => {
            input.len() == expected.len() && input.keys().all(|key| expected.contains_key(key))
        }
        _ => false,
    };
    if !exact_fields {
        return Err(serde::de::Error::custom("invalid durable Goal fields"));
    }
    validate_goal_definition(&goal).map_err(serde::de::Error::custom)?;
    Ok(goal)
}

/// References only. Neither this binding nor its source identity grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableGoalBindingV1 {
    pub goal_id: String,
    pub session_id: String,
    pub workspace: WorkspaceRef,
    pub goal_revision: u32,
    pub non_goals: Vec<String>,
    pub scope_fact_sequences: Vec<u64>,
    pub authorization_source: ExecutionIdentityV1,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub work_policy_digest: Option<String>,
}

impl DurableGoalBindingV1 {
    /// Structural validation; admission must also match actual source facts and authority.
    pub fn validate(&self) -> Result<(), GoalControlError> {
        validate_id("goal_id", &self.goal_id)?;
        validate_id("session_id", &self.session_id)?;
        validate_text("workspace.id", &self.workspace.id, 4096)?;
        validate_text("workspace.display_root", &self.workspace.display_root, 4096)?;
        if self.goal_revision == 0 {
            return Err(invalid("goal_revision", "must be greater than zero"));
        }
        if self.non_goals.len() > 32 || self.scope_fact_sequences.len() > 32 {
            return Err(invalid(
                "scope",
                "at most 32 non-goals and 32 source references",
            ));
        }
        for text in &self.non_goals {
            validate_text("non_goals", text, 1024)?;
        }
        let mut sources = BTreeSet::new();
        if self
            .scope_fact_sequences
            .iter()
            .any(|seq| *seq == 0 || !sources.insert(*seq))
        {
            return Err(invalid(
                "scope_fact_sequences",
                "must be distinct positive original fact sequences",
            ));
        }
        self.authorization_source.validate()?;
        if let Some(digest) = &self.work_policy_digest {
            crate::content_digest::validate_digest(digest)
                .map_err(|_| invalid("work_policy_digest", "must be a canonical SHA-256 digest"))?;
        }
        Ok(())
    }

    pub fn next_revision(&self) -> Result<u32, GoalControlError> {
        self.validate()?;
        self.goal_revision
            .checked_add(1)
            .ok_or_else(|| invalid("goal_revision", "revision overflow"))
    }
}

/// Validate original Goal content at the new persistent boundary, without accepting progress.
pub fn validate_goal_definition(goal: &AgentGoal) -> Result<(), GoalControlError> {
    validate_text("description", &goal.description, 4096)?;
    if goal.success_criteria.is_empty() || goal.success_criteria.len() > 32 {
        return Err(invalid(
            "success_criteria",
            "must contain 1–32 required criteria",
        ));
    }
    for criterion in &goal.success_criteria {
        validate_text("success_criteria", criterion, 1024)?;
    }
    if goal.progress != 0.0 || goal.achieved || goal.achieved_at.is_some() {
        return Err(invalid(
            "progress",
            "client progress cannot accept a durable Goal outcome",
        ));
    }
    Ok(())
}

/// Check a bounded definition through the original plan's dependency semantics.
pub fn validate_goal_plan(plan: &ExecutionPlan) -> Result<ExecutionIdentityV1, GoalControlError> {
    validate_text("plan.goal", &plan.goal, 4096)?;
    if let Some(context) = &plan.execution_context {
        validate_text("execution_context", context, 32768)?;
    }
    if plan.steps.is_empty() || plan.steps.len() > 64 || plan.required_tools.len() > 64 {
        return Err(invalid(
            "plan",
            "requires 1–64 steps and at most 64 required tools",
        ));
    }
    for tool in &plan.required_tools {
        validate_id("required_tools", tool)?;
    }
    let mut ids = BTreeSet::new();
    for step in &plan.steps {
        validate_id("step.id", &step.id)?;
        if !ids.insert(step.id.as_str()) {
            return Err(invalid("step.id", "duplicate step id"));
        }
        validate_text("step.content", &step.content, 4096)?;
        validate_text(
            "step.success_criteria",
            step.success_criteria.as_deref().unwrap_or(""),
            1024,
        )?;
        if let Some(tool) = &step.tool {
            validate_id("step.tool", tool)?;
        }
        if step.dependencies.len() > 64 {
            return Err(invalid("dependencies", "at most 64 dependencies per step"));
        }
    }
    for step in &plan.steps {
        let mut dependencies = BTreeSet::new();
        for dependency in &step.dependencies {
            if dependency == &step.id
                || !ids.contains(dependency.as_str())
                || !dependencies.insert(dependency)
            {
                return Err(invalid(
                    "dependencies",
                    "self, missing or duplicate dependency",
                ));
            }
        }
    }

    // Display statuses cannot hide a cycle or constitute accepted progress.
    let mut remaining = plan.clone();
    for step in &mut remaining.steps {
        step.status = TaskStatus::Pending;
    }
    while remaining.pending_count() > 0 {
        let ready = remaining
            .get_ready_steps()
            .iter()
            .map(|step| step.id.clone())
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(invalid("dependencies", "cyclic dependency graph"));
        }
        for id in ready {
            remaining.mark_status(&id, TaskStatus::Completed);
        }
    }
    Ok(plan.definition_identity()?)
}

fn validate_id(field: &'static str, value: &str) -> Result<(), GoalControlError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "must contain 1–128 UTF-8 bytes without control characters",
        ));
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), GoalControlError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(invalid(
            field,
            "must be nonblank, within its UTF-8 byte limit and without NUL",
        ));
    }
    Ok(())
}

fn invalid(field: &'static str, reason: &'static str) -> GoalControlError {
    GoalControlError::Invalid { field, reason }
}

#[derive(Debug, Error)]
pub enum GoalControlError {
    #[error("{}: {reason}", .refusal.as_str())]
    Refused {
        refusal: GoalRefusal,
        reason: &'static str,
    },
    #[error("goal.writer_unavailable: original log failed: {0}")]
    Log(#[from] a3s_effect::ActorError),
    #[error("goal.conflict: definition encoding failed: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("unknown {set} value {value:?}")]
    UnknownClosedSet { set: &'static str, value: String },
    #[error("invalid durable Goal field {field}: {reason}")]
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    #[error("invalid durable Goal source identity: {0}")]
    Identity(#[from] ExecutionIdentityError),
}

#[cfg(test)]
mod tests;
