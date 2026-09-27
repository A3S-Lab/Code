//! Presentation policy for the subagent `model` argument, latched per agent construction.

use std::sync::Arc;

use a3s_code_agent::prompt::context::PromptAudience;
use a3s_code_telemetry::events::{
    SubagentModelCatalogKind, SubagentModelOverrideRejected, SubagentModelPresentationApplied,
    SubagentModelRejectionReason, SubagentModelSelectionKind, SubagentOwnerKind,
    SubagentPresentationAudience,
};
use a3s_code_tools::implementations::grok_build::task::model_policy::{
    TaskModelRejection, TaskModelRejectionSink,
};

use crate::agent::config::Resolved;
use crate::agent::remote_config::ModelsManager;
pub(crate) use a3s_code_tools::implementations::grok_build::task::model_policy::TaskModelSelection;

const FIRST_PARTY_FAMILY: &str = "a3s";

/// One entry the picker would offer (`ModelInfo::is_picker_eligible`); `None` family is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EligibleTaskModel {
    pub id: String,
    pub model_family: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskModelCatalogSnapshot {
    pub eligible: Vec<EligibleTaskModel>,
    pub authority: CatalogAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogAuthority {
    Complete,
    /// Remote discovery is expected but has not produced a catalog yet.
    Provisional,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskModelPolicyInputs {
    pub inheritance: Resolved<bool>,
    pub remote_fetch_enabled: bool,
    /// A verbatim fork mirrors its parent's schema, so it advertises the parent's selection.
    pub forked_selection: Option<TaskModelSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskCatalogClassification {
    Provisional,
    Empty,
    UnknownFamily,
    FirstPartyOnly,
    ThirdPartyOnly,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskModelPresentation {
    pub selection: TaskModelSelection,
    pub model_slugs: Vec<String>,
    pub classification: TaskCatalogClassification,
}

fn classify(snapshot: &TaskModelCatalogSnapshot) -> TaskCatalogClassification {
    if snapshot.authority == CatalogAuthority::Provisional {
        return TaskCatalogClassification::Provisional;
    }
    if snapshot.eligible.is_empty() {
        return TaskCatalogClassification::Empty;
    }
    let families: Vec<Option<&str>> = snapshot
        .eligible
        .iter()
        .map(|entry| entry.model_family.as_deref().map(str::trim))
        .collect();
    if families
        .iter()
        .any(|family| family.is_none_or(str::is_empty))
    {
        return TaskCatalogClassification::UnknownFamily;
    }
    let first_party = families
        .iter()
        .filter(|family| **family == Some(FIRST_PARTY_FAMILY))
        .count();
    if first_party == snapshot.eligible.len() {
        TaskCatalogClassification::FirstPartyOnly
    } else if first_party == 0 {
        TaskCatalogClassification::ThirdPartyOnly
    } else {
        TaskCatalogClassification::Mixed
    }
}

pub(crate) fn resolve_presentation(
    inheritance_enabled: bool,
    snapshot: TaskModelCatalogSnapshot,
) -> TaskModelPresentation {
    let classification = classify(&snapshot);
    let selection =
        if inheritance_enabled && classification == TaskCatalogClassification::FirstPartyOnly {
            TaskModelSelection::Inherited
        } else {
            TaskModelSelection::Selectable
        };
    TaskModelPresentation {
        selection,
        model_slugs: snapshot
            .eligible
            .into_iter()
            .map(|entry| entry.id)
            .collect(),
        classification,
    }
}

/// Only an enabled, unpinned construction waits for the first remote catalog.
pub(crate) async fn latch_task_model_presentation(
    models_manager: &ModelsManager,
    inputs: &TaskModelPolicyInputs,
) -> TaskModelPresentation {
    if inputs.inheritance.value && inputs.forked_selection.is_none() {
        models_manager
            .wait_for_first_catalog(inputs.remote_fetch_enabled)
            .await;
    }
    let snapshot = models_manager.task_model_catalog_snapshot(inputs.remote_fetch_enabled);
    let mut presentation = resolve_presentation(inputs.inheritance.value, snapshot);
    if let Some(selection) = inputs.forked_selection {
        presentation.selection = selection;
    }
    presentation
}

pub(crate) fn presentation_applied_event(
    presentation: &TaskModelPresentation,
    inputs: &TaskModelPolicyInputs,
    audience: PromptAudience,
) -> SubagentModelPresentationApplied {
    SubagentModelPresentationApplied {
        selection: selection_telemetry_kind(presentation.selection),
        classification: presentation.classification.into(),
        eligible_count: u32::try_from(presentation.model_slugs.len()).unwrap_or(u32::MAX),
        inheritance_enabled: inputs.inheritance.value,
        feature_source: inputs.inheritance.source.to_string(),
        audience: match audience {
            PromptAudience::Primary => SubagentPresentationAudience::Primary,
            PromptAudience::Subagent => SubagentPresentationAudience::Subagent,
        },
    }
}

pub(crate) fn rejection_sink(parent_session_id: String) -> TaskModelRejectionSink {
    TaskModelRejectionSink::new(move |rejection| {
        a3s_code_telemetry::session_ctx::log_event(SubagentModelOverrideRejected {
            parent_session_id: parent_session_id.clone(),
            owner: SubagentOwnerKind::Task,
            reason: match rejection {
                TaskModelRejection::HiddenSelection => {
                    SubagentModelRejectionReason::HiddenSelection
                }
            },
        });
    })
}

/// The live agent's selection, shared so fork snapshots and workflow spawns never reclassify.
#[derive(Debug, Clone, Default)]
pub(crate) struct LatchedTaskModelSelection(Arc<parking_lot::Mutex<TaskModelSelection>>);

impl LatchedTaskModelSelection {
    pub(crate) fn get(&self) -> TaskModelSelection {
        *self.0.lock()
    }

    pub(crate) fn set(&self, selection: TaskModelSelection) {
        *self.0.lock() = selection;
    }
}

/// Both types are foreign here, so this cannot be a `From` impl.
pub(crate) fn selection_telemetry_kind(
    selection: TaskModelSelection,
) -> SubagentModelSelectionKind {
    match selection {
        TaskModelSelection::Selectable => SubagentModelSelectionKind::Selectable,
        TaskModelSelection::Inherited => SubagentModelSelectionKind::Inherited,
    }
}

impl From<TaskCatalogClassification> for SubagentModelCatalogKind {
    fn from(classification: TaskCatalogClassification) -> Self {
        match classification {
            TaskCatalogClassification::Provisional => Self::Provisional,
            TaskCatalogClassification::Empty => Self::Empty,
            TaskCatalogClassification::UnknownFamily => Self::UnknownFamily,
            TaskCatalogClassification::FirstPartyOnly => Self::FirstPartyOnly,
            TaskCatalogClassification::ThirdPartyOnly => Self::ThirdPartyOnly,
            TaskCatalogClassification::Mixed => Self::Mixed,
        }
    }
}

#[cfg(test)]
#[path = "task_model_policy_tests.rs"]
mod tests;
