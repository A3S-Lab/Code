//! Programmatic tool calling primitives.
//!
//! A program is a named, deterministic chain of tool invocations executed by
//! the harness instead of expanded turn-by-turn through the model loop.

use crate::tools::{ToolContext, ToolRegistry};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

pub const PROGRAM_TRACE_SCHEMA: &str = "a3s.program_trace.v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub name: String,
    pub description: String,
    pub steps: Vec<ProgramStep>,
}

impl Program {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            steps: Vec::new(),
        }
    }

    pub fn with_step(mut self, step: ProgramStep) -> Self {
        self.steps.push(step);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramTemplate {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ProgramParameter>,
    pub steps: Vec<ProgramStepTemplate>,
}

impl ProgramTemplate {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: Vec::new(),
            steps: Vec::new(),
        }
    }

    pub fn with_parameter(mut self, parameter: ProgramParameter) -> Self {
        self.parameters.push(parameter);
        self
    }

    pub fn with_step(mut self, step: ProgramStepTemplate) -> Self {
        self.steps.push(step);
        self
    }

    pub fn validate(&self) -> ProgramTemplateValidation {
        ProgramTemplateValidation::validate(self)
    }

    pub fn ensure_valid(&self) -> Result<()> {
        let validation = self.validate();
        if validation.is_valid() {
            Ok(())
        } else {
            bail!("{}", validation.summary());
        }
    }

    pub fn instantiate(&self, inputs: &serde_json::Value) -> Result<Program> {
        self.ensure_valid()?;
        let input_object = inputs.as_object();
        let mut bindings = serde_json::Map::new();

        for parameter in &self.parameters {
            let value = input_object.and_then(|object| object.get(&parameter.name));
            match (value, &parameter.default, parameter.required) {
                (Some(value), _, _) => {
                    bindings.insert(parameter.name.clone(), value.clone());
                }
                (None, Some(default), _) => {
                    bindings.insert(parameter.name.clone(), default.clone());
                }
                (None, None, true) => {
                    bail!("Missing required program parameter: {}", parameter.name);
                }
                (None, None, false) => {}
            }
        }

        let bindings = serde_json::Value::Object(bindings);
        let mut program = Program::new(self.name.clone(), self.description.clone());
        for step in &self.steps {
            program = program.with_step(ProgramStep {
                tool_name: step.tool_name.clone(),
                args: render_template_value(&step.args, &bindings),
                label: step.label.clone(),
            });
        }
        Ok(program)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramTemplateValidation {
    pub template_name: String,
    pub issues: Vec<ProgramTemplateIssue>,
}

impl ProgramTemplateValidation {
    pub fn validate(template: &ProgramTemplate) -> Self {
        let mut issues = Vec::new();
        validate_program_template(template, &mut issues);
        Self {
            template_name: template.name.clone(),
            issues,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.is_valid() {
            return format!("Program template '{}' is valid", self.template_name);
        }

        let issues = self
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.path, issue.message))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Program template '{}' is invalid: {}",
            self.template_name, issues
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramTemplateIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl ProgramTemplateIssue {
    fn new(code: impl Into<String>, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            message: message.into(),
        }
    }
}

fn validate_program_template(template: &ProgramTemplate, issues: &mut Vec<ProgramTemplateIssue>) {
    if template.name.trim().is_empty() {
        issues.push(ProgramTemplateIssue::new(
            "empty_name",
            "name",
            "program template name is required",
        ));
    } else if !is_program_identifier(&template.name) {
        issues.push(ProgramTemplateIssue::new(
            "invalid_name",
            "name",
            "program template name must contain only ASCII letters, numbers, '_' or '-'",
        ));
    }

    if template.description.trim().is_empty() {
        issues.push(ProgramTemplateIssue::new(
            "empty_description",
            "description",
            "program template description is required",
        ));
    }

    let mut parameter_names = HashSet::new();
    for (index, parameter) in template.parameters.iter().enumerate() {
        let path = format!("parameters[{index}].name");
        if parameter.name.trim().is_empty() {
            issues.push(ProgramTemplateIssue::new(
                "empty_parameter_name",
                path,
                "program parameter name is required",
            ));
        } else if !is_program_identifier(&parameter.name) {
            issues.push(ProgramTemplateIssue::new(
                "invalid_parameter_name",
                path,
                "program parameter name must contain only ASCII letters, numbers, '_' or '-'",
            ));
        } else if !parameter_names.insert(parameter.name.clone()) {
            issues.push(ProgramTemplateIssue::new(
                "duplicate_parameter",
                path,
                format!("duplicate program parameter '{}'", parameter.name),
            ));
        }

        if parameter.required && parameter.default.is_some() {
            issues.push(ProgramTemplateIssue::new(
                "required_parameter_with_default",
                format!("parameters[{index}].default"),
                "required program parameters must not define defaults",
            ));
        }
    }

    if template.steps.is_empty() {
        issues.push(ProgramTemplateIssue::new(
            "empty_steps",
            "steps",
            "program template must contain at least one step",
        ));
    }

    let mut labels = HashSet::new();
    for (index, step) in template.steps.iter().enumerate() {
        if step.tool_name.trim().is_empty() {
            issues.push(ProgramTemplateIssue::new(
                "empty_tool_name",
                format!("steps[{index}].tool_name"),
                "program step tool_name is required",
            ));
        }

        if let Some(label) = &step.label {
            if label.trim().is_empty() {
                issues.push(ProgramTemplateIssue::new(
                    "empty_step_label",
                    format!("steps[{index}].label"),
                    "program step label must not be empty",
                ));
            } else if !labels.insert(label.clone()) {
                issues.push(ProgramTemplateIssue::new(
                    "duplicate_step_label",
                    format!("steps[{index}].label"),
                    format!("duplicate program step label '{label}'"),
                ));
            }
        }

        validate_template_value(
            &step.args,
            &format!("steps[{index}].args"),
            &parameter_names,
            issues,
        );
    }
}

fn validate_template_value(
    value: &serde_json::Value,
    path: &str,
    parameter_names: &HashSet<String>,
    issues: &mut Vec<ProgramTemplateIssue>,
) {
    match value {
        serde_json::Value::String(text) => {
            for placeholder in template_placeholders(text) {
                match placeholder {
                    Ok(name) if !is_program_identifier(&name) => {
                        issues.push(ProgramTemplateIssue::new(
                            "invalid_placeholder",
                            path,
                            format!("invalid placeholder '{{{{{name}}}}}'"),
                        ));
                    }
                    Ok(name) if !parameter_names.contains(&name) => {
                        issues.push(ProgramTemplateIssue::new(
                            "unknown_placeholder",
                            path,
                            format!("unknown program parameter placeholder '{{{{{name}}}}}'"),
                        ));
                    }
                    Ok(_) => {}
                    Err(message) => {
                        issues.push(ProgramTemplateIssue::new(
                            "malformed_placeholder",
                            path,
                            message,
                        ));
                    }
                }
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_template_value(item, &format!("{path}[{index}]"), parameter_names, issues);
            }
        }
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                validate_template_value(value, &format!("{path}.{key}"), parameter_names, issues);
            }
        }
        _ => {}
    }
}

fn is_program_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn template_placeholders(text: &str) -> Vec<std::result::Result<String, String>> {
    let mut placeholders = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            placeholders.push(Err(
                "malformed placeholder: missing closing '}}'".to_string()
            ));
            return placeholders;
        };
        let name = after_start[..end].trim();
        if name.is_empty() {
            placeholders.push(Err("malformed placeholder: empty name".to_string()));
        } else {
            placeholders.push(Ok(name.to_string()));
        }
        rest = &after_start[end + 2..];
    }

    if rest.contains("}}") {
        placeholders.push(Err(
            "malformed placeholder: missing opening '{{'".to_string()
        ));
    }

    placeholders
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramParameter {
    pub name: String,
    pub description: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

impl ProgramParameter {
    pub fn required(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            required: true,
            default: None,
        }
    }

    pub fn optional(
        name: impl Into<String>,
        description: impl Into<String>,
        default: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            required: false,
            default: Some(default),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramStep {
    pub tool_name: String,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ProgramStep {
    pub fn new(tool_name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            args,
            label: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramStepTemplate {
    pub tool_name: String,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ProgramStepTemplate {
    pub fn new(tool_name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            args,
            label: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramResult {
    pub program_name: String,
    pub success: bool,
    pub summary: String,
    pub steps: Vec<ProgramStepResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramStepResult {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub success: bool,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramTrace {
    pub schema: String,
    #[serde(rename = "type")]
    pub trace_type: String,
    pub program_name: String,
    pub success: bool,
    pub summary: String,
    pub step_count: usize,
    pub failed_steps: usize,
    pub steps: Vec<ProgramTraceStep>,
}

impl ProgramTrace {
    pub fn from_result(result: &ProgramResult, steps: Vec<ProgramTraceStep>) -> Self {
        Self {
            schema: PROGRAM_TRACE_SCHEMA.to_string(),
            trace_type: "program_execution".to_string(),
            program_name: result.program_name.clone(),
            success: result.success,
            summary: result.summary.clone(),
            step_count: steps.len(),
            failed_steps: steps.iter().filter(|step| !step.success).count(),
            steps,
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            serde_json::json!({
                "schema": PROGRAM_TRACE_SCHEMA,
                "type": "program_execution",
                "program_name": self.program_name,
                "success": self.success,
                "summary": self.summary,
                "step_count": self.step_count,
                "failed_steps": self.failed_steps,
                "steps": [],
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramTraceStep {
    pub index: usize,
    pub label: String,
    pub tool_name: String,
    pub success: bool,
    pub output_bytes: usize,
    pub compacted: bool,
    #[serde(default)]
    pub artifact: Option<ProgramTraceArtifact>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

impl ProgramTraceStep {
    pub fn from_result(
        index: usize,
        step: &ProgramStepResult,
        compacted: bool,
        artifact: Option<ProgramTraceArtifact>,
    ) -> Self {
        Self {
            index,
            label: step.label.clone().unwrap_or_else(|| step.tool_name.clone()),
            tool_name: step.tool_name.clone(),
            success: step.success,
            output_bytes: step.output.len(),
            compacted,
            artifact,
            metadata: step.metadata.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramTraceArtifact {
    pub artifact_id: String,
    pub artifact_uri: String,
    pub original_bytes: usize,
    pub shown_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramVerificationHint {
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_uris: Vec<String>,
}

impl ProgramVerificationHint {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            required: false,
            suggested_tools: Vec::new(),
            evidence_uris: Vec::new(),
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn with_suggested_tools(
        mut self,
        tools: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.suggested_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_evidence_uris(mut self, uris: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.evidence_uris = uris.into_iter().map(Into::into).collect();
        self
    }

    pub fn to_values(hints: &[Self]) -> Vec<serde_json::Value> {
        hints
            .iter()
            .map(|hint| serde_json::to_value(hint).unwrap_or_else(|_| serde_json::json!({})))
            .collect()
    }
}

pub fn program_verification_hints(
    result: &ProgramResult,
    trace: Option<&ProgramTrace>,
) -> Vec<ProgramVerificationHint> {
    let mut hints = match result.program_name.as_str() {
        "program_code_search" => vec![ProgramVerificationHint::new(
            "inspect_matches",
            "Review matched files before editing or drawing conclusions.",
        )
        .required()
        .with_suggested_tools(["read", "grep"])],
        "program_repo_map" => vec![ProgramVerificationHint::new(
            "inspect_project_files",
            "Use detected project files to choose build, test, and lint commands.",
        )
        .required()
        .with_suggested_tools(["read", "glob"])],
        _ => Vec::new(),
    };

    if !result.success {
        let failed_steps = result
            .steps
            .iter()
            .filter(|step| !step.success)
            .map(|step| step.label.as_deref().unwrap_or(&step.tool_name))
            .collect::<Vec<_>>();
        let message = if failed_steps.is_empty() {
            "Investigate the failed program execution before relying on its result.".to_string()
        } else {
            format!(
                "Investigate failed program step(s): {}.",
                failed_steps.join(", ")
            )
        };
        hints.push(
            ProgramVerificationHint::new("investigate_failed_steps", message)
                .required()
                .with_suggested_tools(["read", "grep"]),
        );
    }

    if let Some(trace) = trace {
        let evidence_uris = trace
            .steps
            .iter()
            .filter_map(|step| step.artifact.as_ref())
            .map(|artifact| artifact.artifact_uri.clone())
            .collect::<Vec<_>>();

        if !evidence_uris.is_empty() {
            hints.push(
                ProgramVerificationHint::new(
                    "inspect_artifacts",
                    "Inspect compacted program artifacts before treating summarized output as complete evidence.",
                )
                .required()
                .with_evidence_uris(evidence_uris),
            );
        }
    }

    hints
}

pub struct ProgramExecutor {
    registry: Arc<ToolRegistry>,
    context: ToolContext,
}

#[derive(Debug, Clone, Default)]
pub struct ProgramCatalog {
    templates: Vec<ProgramTemplate>,
}

impl ProgramCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin_programs() -> Self {
        let mut catalog = Self::new();
        for template in builtin_program_templates() {
            catalog.register(template);
        }
        catalog
    }

    pub fn register(&mut self, template: ProgramTemplate) {
        self.insert(template);
    }

    pub fn try_register(&mut self, template: ProgramTemplate) -> Result<()> {
        template.ensure_valid()?;
        self.insert(template);
        Ok(())
    }

    fn insert(&mut self, template: ProgramTemplate) {
        if let Some(existing) = self
            .templates
            .iter_mut()
            .find(|existing| existing.name == template.name)
        {
            *existing = template;
        } else {
            self.templates.push(template);
        }
    }

    pub fn get(&self, name: &str) -> Option<&ProgramTemplate> {
        self.templates.iter().find(|template| template.name == name)
    }

    pub fn list(&self) -> &[ProgramTemplate] {
        &self.templates
    }

    pub fn instantiate(&self, name: &str, inputs: &serde_json::Value) -> Result<Program> {
        let template = self
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown program: {}", name))?;
        template.instantiate(inputs)
    }
}

impl ProgramExecutor {
    pub fn new(registry: Arc<ToolRegistry>, context: ToolContext) -> Self {
        Self { registry, context }
    }

    pub async fn execute(&self, program: &Program) -> Result<ProgramResult> {
        let mut steps = Vec::with_capacity(program.steps.len());
        let mut success = true;

        for step in &program.steps {
            let result = self
                .registry
                .execute_with_context(&step.tool_name, &step.args, &self.context)
                .await?;

            let step_success = result.exit_code == 0;
            success &= step_success;
            steps.push(ProgramStepResult {
                tool_name: step.tool_name.clone(),
                label: step.label.clone(),
                success: step_success,
                output: result.output,
                metadata: result.metadata,
            });

            if !step_success {
                break;
            }
        }

        Ok(ProgramResult {
            program_name: program.name.clone(),
            success,
            summary: summarize_program_result(program, success, steps.len()),
            steps,
        })
    }
}

pub fn builtin_program_templates() -> Vec<ProgramTemplate> {
    vec![program_code_search(), program_repo_map()]
}

pub fn program_code_search() -> ProgramTemplate {
    ProgramTemplate::new(
        "program_code_search",
        "Search code with a bounded grep pass and return file/line matches.",
    )
    .with_parameter(ProgramParameter::required(
        "query",
        "Regex or literal pattern to search for.",
    ))
    .with_parameter(ProgramParameter::optional(
        "path",
        "Workspace-relative path to search.",
        serde_json::json!("."),
    ))
    .with_parameter(ProgramParameter::optional(
        "glob",
        "Optional file glob filter.",
        serde_json::json!("*"),
    ))
    .with_step(
        ProgramStepTemplate::new(
            "grep",
            serde_json::json!({
                "pattern": "{{query}}",
                "path": "{{path}}",
                "glob": "{{glob}}",
                "context": 2
            }),
        )
        .with_label("search_code"),
    )
}

pub fn program_repo_map() -> ProgramTemplate {
    let mut template = ProgramTemplate::new(
        "program_repo_map",
        "Map the repository shape with root listing and key project files.",
    )
    .with_parameter(ProgramParameter::optional(
        "path",
        "Workspace-relative path to map.",
        serde_json::json!("."),
    ))
    .with_step(
        ProgramStepTemplate::new("ls", serde_json::json!({ "path": "{{path}}" }))
            .with_label("list_root"),
    );

    for pattern in [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "README.md",
        "AGENTS.md",
    ] {
        template = template.with_step(
            ProgramStepTemplate::new(
                "glob",
                serde_json::json!({
                    "path": "{{path}}",
                    "pattern": pattern
                }),
            )
            .with_label(format!("find_{pattern}")),
        );
    }

    template
}

fn summarize_program_result(program: &Program, success: bool, completed_steps: usize) -> String {
    let status = if success { "completed" } else { "stopped" };
    format!(
        "Program '{}' {} after {}/{} steps.",
        program.name,
        status,
        completed_steps,
        program.steps.len()
    )
}

fn render_template_value(
    value: &serde_json::Value,
    bindings: &serde_json::Value,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => render_template_string(text, bindings),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| render_template_value(item, bindings))
                .collect(),
        ),
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), render_template_value(value, bindings)))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn render_template_string(text: &str, bindings: &serde_json::Value) -> serde_json::Value {
    if let Some(name) = exact_placeholder_name(text) {
        return bindings.get(name).cloned().unwrap_or_default();
    }

    let mut rendered = text.to_string();
    if let Some(object) = bindings.as_object() {
        for (key, value) in object {
            let replacement = value
                .as_str()
                .map(ToString::to_string)
                .unwrap_or_else(|| value.to_string());
            rendered = rendered.replace(&format!("{{{{{key}}}}}"), &replacement);
        }
    }
    serde_json::Value::String(rendered)
}

fn exact_placeholder_name(text: &str) -> Option<&str> {
    text.strip_prefix("{{")?.strip_suffix("}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolOutput};
    use anyhow::Result;
    use async_trait::async_trait;
    use std::path::PathBuf;

    #[test]
    fn program_template_instantiates_step_args() {
        let template = ProgramTemplate::new("search", "Search")
            .with_parameter(ProgramParameter::required("query", "Search query"))
            .with_parameter(ProgramParameter::optional(
                "path",
                "Search path",
                serde_json::json!("."),
            ))
            .with_step(ProgramStepTemplate::new(
                "grep",
                serde_json::json!({
                    "pattern": "{{query}}",
                    "path": "{{path}}",
                    "message": "query={{query}}"
                }),
            ));

        let program = template
            .instantiate(&serde_json::json!({ "query": "AgentLoop" }))
            .unwrap();

        assert_eq!(program.name, "search");
        assert_eq!(program.steps.len(), 1);
        assert_eq!(program.steps[0].args["pattern"], "AgentLoop");
        assert_eq!(program.steps[0].args["path"], ".");
        assert_eq!(program.steps[0].args["message"], "query=AgentLoop");
    }

    #[test]
    fn program_template_requires_declared_inputs() {
        let template = ProgramTemplate::new("search", "Search")
            .with_parameter(ProgramParameter::required("query", "Search query"))
            .with_step(ProgramStepTemplate::new(
                "grep",
                serde_json::json!({ "pattern": "{{query}}" }),
            ));

        let err = template.instantiate(&serde_json::json!({})).unwrap_err();

        assert!(err
            .to_string()
            .contains("Missing required program parameter"));
    }

    #[test]
    fn builtin_program_catalog_contains_first_ptc_programs() {
        let catalog = ProgramCatalog::with_builtin_programs();

        assert!(catalog.get("program_code_search").is_some());
        assert!(catalog.get("program_repo_map").is_some());
        assert_eq!(catalog.list().len(), 2);
    }

    #[test]
    fn code_search_program_uses_query_path_and_glob() {
        let catalog = ProgramCatalog::with_builtin_programs();
        let program = catalog
            .instantiate(
                "program_code_search",
                &serde_json::json!({
                    "query": "ContextAssembler",
                    "path": "core/src",
                    "glob": "*.rs"
                }),
            )
            .unwrap();

        assert_eq!(program.steps.len(), 1);
        assert_eq!(program.steps[0].tool_name, "grep");
        assert_eq!(program.steps[0].label.as_deref(), Some("search_code"));
        assert_eq!(program.steps[0].args["pattern"], "ContextAssembler");
        assert_eq!(program.steps[0].args["path"], "core/src");
        assert_eq!(program.steps[0].args["glob"], "*.rs");
    }

    #[test]
    fn repo_map_program_uses_bounded_root_steps() {
        let catalog = ProgramCatalog::with_builtin_programs();
        let program = catalog
            .instantiate("program_repo_map", &serde_json::json!({ "path": "." }))
            .unwrap();

        assert_eq!(program.steps.len(), 7);
        assert_eq!(program.steps[0].tool_name, "ls");
        assert_eq!(program.steps[0].label.as_deref(), Some("list_root"));
        assert!(program.steps[1..]
            .iter()
            .all(|step| step.tool_name == "glob"));
        assert_eq!(program.steps[1].label.as_deref(), Some("find_Cargo.toml"));
        assert_eq!(program.steps[1].args["pattern"], "Cargo.toml");
        assert_eq!(program.steps[6].args["pattern"], "AGENTS.md");
    }

    #[test]
    fn program_template_validation_accepts_builtin_templates() {
        for template in builtin_program_templates() {
            let validation = template.validate();
            assert!(
                validation.is_valid(),
                "unexpected validation errors: {}",
                validation.summary()
            );
        }
    }

    #[test]
    fn program_template_validation_reports_asset_issues() {
        let template = ProgramTemplate::new("bad name", "")
            .with_parameter(ProgramParameter::required("query", "Query"))
            .with_parameter(ProgramParameter::required("query", "Duplicate query"))
            .with_step(
                ProgramStepTemplate::new(
                    "",
                    serde_json::json!({
                        "pattern": "{{missing}}",
                        "dangling": "{{query"
                    }),
                )
                .with_label("scan"),
            )
            .with_step(
                ProgramStepTemplate::new("grep", serde_json::json!({ "pattern": "{{query}}" }))
                    .with_label("scan"),
            );

        let validation = template.validate();
        let codes = validation
            .issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<Vec<_>>();

        assert!(!validation.is_valid());
        assert!(codes.contains(&"invalid_name"));
        assert!(codes.contains(&"empty_description"));
        assert!(codes.contains(&"duplicate_parameter"));
        assert!(codes.contains(&"empty_tool_name"));
        assert!(codes.contains(&"unknown_placeholder"));
        assert!(codes.contains(&"malformed_placeholder"));
        assert!(codes.contains(&"duplicate_step_label"));
    }

    #[test]
    fn program_catalog_try_register_rejects_invalid_template() {
        let mut catalog = ProgramCatalog::new();
        let template = ProgramTemplate::new("empty_steps", "Missing steps");

        let err = catalog.try_register(template).unwrap_err();

        assert!(err.to_string().contains("empty_steps"));
        assert!(catalog.list().is_empty());
    }

    #[test]
    fn program_trace_serializes_with_stable_schema() {
        let result = ProgramResult {
            program_name: "program_code_search".to_string(),
            success: true,
            summary: "done".to_string(),
            steps: vec![ProgramStepResult {
                tool_name: "grep".to_string(),
                label: Some("search_code".to_string()),
                success: true,
                output: "match".to_string(),
                metadata: Some(serde_json::json!({ "exit_code": 0 })),
            }],
        };

        let step_trace = ProgramTraceStep::from_result(
            0,
            &result.steps[0],
            true,
            Some(ProgramTraceArtifact {
                artifact_id: "artifact-1".to_string(),
                artifact_uri: "artifact://tool-output/artifact-1".to_string(),
                original_bytes: 100,
                shown_bytes: 10,
            }),
        );
        let trace = ProgramTrace::from_result(&result, vec![step_trace]);
        let value = trace.to_value();

        assert_eq!(value["schema"], PROGRAM_TRACE_SCHEMA);
        assert_eq!(value["type"], "program_execution");
        assert_eq!(value["program_name"], "program_code_search");
        assert_eq!(value["step_count"], 1);
        assert_eq!(value["failed_steps"], 0);
        assert_eq!(value["steps"][0]["label"], "search_code");
        assert_eq!(value["steps"][0]["output_bytes"], 5);
        assert_eq!(value["steps"][0]["metadata"]["exit_code"], 0);
        assert_eq!(
            value["steps"][0]["artifact"]["artifact_uri"],
            "artifact://tool-output/artifact-1"
        );
    }

    #[test]
    fn program_verification_hints_include_program_contract() {
        let result = ProgramResult {
            program_name: "program_repo_map".to_string(),
            success: true,
            summary: "done".to_string(),
            steps: vec![],
        };

        let hints = program_verification_hints(&result, None);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].kind, "inspect_project_files");
        assert!(hints[0].required);
        assert_eq!(hints[0].suggested_tools, vec!["read", "glob"]);
    }

    #[test]
    fn program_verification_hints_include_failures_and_artifacts() {
        let result = ProgramResult {
            program_name: "custom_program".to_string(),
            success: false,
            summary: "stopped".to_string(),
            steps: vec![ProgramStepResult {
                tool_name: "grep".to_string(),
                label: Some("scan".to_string()),
                success: false,
                output: "failed".to_string(),
                metadata: None,
            }],
        };
        let trace = ProgramTrace::from_result(
            &result,
            vec![ProgramTraceStep {
                index: 0,
                label: "scan".to_string(),
                tool_name: "grep".to_string(),
                success: false,
                output_bytes: 6,
                compacted: true,
                artifact: Some(ProgramTraceArtifact {
                    artifact_id: "artifact-1".to_string(),
                    artifact_uri: "artifact://tool-output/artifact-1".to_string(),
                    original_bytes: 100,
                    shown_bytes: 6,
                }),
                metadata: None,
            }],
        );

        let hints = program_verification_hints(&result, Some(&trace));

        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].kind, "investigate_failed_steps");
        assert!(hints[0].message.contains("scan"));
        assert_eq!(hints[1].kind, "inspect_artifacts");
        assert_eq!(
            hints[1].evidence_uris,
            vec!["artifact://tool-output/artifact-1"]
        );
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes the message argument"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success(
                args["message"].as_str().unwrap_or_default(),
            ))
        }
    }

    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }

        fn description(&self) -> &str {
            "Always fails"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {},
                "required": []
            })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::error("failed"))
        }
    }

    #[tokio::test]
    async fn program_executor_runs_steps_in_order() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoTool));
        let executor = ProgramExecutor::new(
            Arc::clone(&registry),
            ToolContext::new(PathBuf::from("/tmp")),
        );
        let program = Program::new("two_echoes", "Run two echo steps")
            .with_step(ProgramStep::new(
                "echo",
                serde_json::json!({ "message": "one" }),
            ))
            .with_step(ProgramStep::new(
                "echo",
                serde_json::json!({ "message": "two" }),
            ));

        let result = executor.execute(&program).await.unwrap();

        assert!(result.success);
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].output, "one");
        assert_eq!(result.steps[1].output, "two");
        assert_eq!(result.steps[0].label, None);
        assert_eq!(
            result.summary,
            "Program 'two_echoes' completed after 2/2 steps."
        );
    }

    #[tokio::test]
    async fn program_executor_stops_after_failed_step() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailTool));
        let executor = ProgramExecutor::new(
            Arc::clone(&registry),
            ToolContext::new(PathBuf::from("/tmp")),
        );
        let program = Program::new("fail_fast", "Stop after a failed step")
            .with_step(ProgramStep::new(
                "echo",
                serde_json::json!({ "message": "before" }),
            ))
            .with_step(ProgramStep::new("fail", serde_json::json!({})))
            .with_step(ProgramStep::new(
                "echo",
                serde_json::json!({ "message": "after" }),
            ));

        let result = executor.execute(&program).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].output, "before");
        assert_eq!(result.steps[1].output, "failed");
        assert_eq!(
            result.summary,
            "Program 'fail_fast' stopped after 2/3 steps."
        );
    }
}
