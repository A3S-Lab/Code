//! Presentation policy for the model-facing `model` argument of the task tool.

use std::sync::Arc;

use crate::types::definition::ToolDefinition;
use crate::types::template_renderer::TemplateRenderer;

/// Canonical name of the model argument, before any client parameter remapping.
pub const MODEL_PARAM: &str = "model";

/// Whether the model may pick a child model explicitly or every child inherits the defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskModelSelection {
    #[default]
    Selectable,
    Inherited,
}

/// Task tool configuration, stored as `Params<TaskParams>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskParams {
    #[serde(default)]
    pub model_selection: TaskModelSelection,
    /// Fresh spawns that omit a type use this when general-purpose is not spawnable
    /// and the parent allowlist names exactly one other type. Not a model argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit_subagent_type: Option<String>,
}

crate::register_resource!("grok_build", "TaskParams", TaskParams);

/// Finalize validated the params before merging, so a parse failure here only logs.
fn selection_from_params(effective_params: &serde_json::Value) -> TaskModelSelection {
    match serde_json::from_value::<TaskParams>(effective_params.clone()) {
        Ok(params) => params.model_selection,
        Err(error) => {
            tracing::warn!(%error, "task params did not parse; keeping explicit model selection");
            TaskModelSelection::default()
        }
    }
}

/// Runs before parameter remapping, so the canonical name is the right key.
fn exported_input_schema(
    input_schema: &serde_json::Value,
    selection: TaskModelSelection,
) -> serde_json::Value {
    let mut schema = input_schema.clone();
    if selection == TaskModelSelection::Selectable {
        return schema;
    }
    if let Some(obj) = schema.as_object_mut() {
        if let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
            props.remove(MODEL_PARAM);
        }
        if let Some(required) = obj.get_mut("required").and_then(|r| r.as_array_mut()) {
            required.retain(|name| name.as_str() != Some(MODEL_PARAM));
        }
    }
    schema
}

/// `ToolMetadata::versioned_definition` for a task tool, over the schema filtered by `TaskParams`.
pub fn task_versioned_definition(
    client_name: &str,
    description_override: Option<&str>,
    description_template: &str,
    renderer: &TemplateRenderer,
    param_map: &std::collections::HashMap<String, String>,
    input_schema: &serde_json::Value,
    effective_params: &serde_json::Value,
) -> ToolDefinition {
    let raw_desc = description_override.unwrap_or(description_template);
    let description = renderer.render(raw_desc).unwrap_or_else(|e| {
        crate::types::template_renderer::strip_markers_on_render_failure(raw_desc, &e)
    });
    let exported = exported_input_schema(input_schema, selection_from_params(effective_params));
    let remapped = if param_map.is_empty() {
        exported
    } else {
        crate::util::remap::remap_schema_properties(&exported, param_map)
    };
    ToolDefinition::function(client_name, Some(&description), remapped)
}

/// `param_name` is the client-facing spelling; the catalog is never enumerated.
pub fn hidden_selection_message(param_name: &str) -> String {
    format!(
        "Explicit subagent model selection is unavailable for this catalog. Retry without the \
         {param_name} argument; configured defaults will apply."
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskModelRejection {
    HiddenSelection,
}

/// Host-injected observer; this crate emits no telemetry itself.
#[derive(Clone)]
pub struct TaskModelRejectionSink(Arc<dyn Fn(TaskModelRejection) + Send + Sync>);

impl TaskModelRejectionSink {
    pub fn new(observe: impl Fn(TaskModelRejection) + Send + Sync + 'static) -> Self {
        Self(Arc::new(observe))
    }

    pub fn notify(&self, rejection: TaskModelRejection) {
        (self.0)(rejection)
    }
}

impl std::fmt::Debug for TaskModelRejectionSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskModelRejectionSink").finish()
    }
}

crate::register_resource!(
    "grok_build",
    "TaskModelRejectionSink",
    TaskModelRejectionSink
);

#[cfg(test)]
#[path = "model_policy_tests.rs"]
mod tests;
