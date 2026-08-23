//! Typed, deterministic model-facing Tool presentation.
//!
//! A presentation profile owns no [`Tool`](super::Tool) values and performs no
//! execution. It can only select definitions from a caller-supplied governed
//! source and, for the built-in code profile, rephrase the existing `program`
//! definition. Names and parameter schemas remain byte-for-byte equivalent to
//! their source definitions.

use super::selector;
use crate::llm::{estimate_prompt_tokens, Message, ToolDefinition};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;

pub const TOOL_PRESENTATION_PROFILE_V1_SCHEMA: &str = "a3s.code.tool-presentation-profile.v1";

const MAX_PRESENTATION_TOOLS: usize = 4_096;
const MAX_CODE_CATALOG_BYTES: usize = 64 * 1024;
const MAX_CODE_CATALOG_DESCRIPTION_BYTES: usize = 192;

/// Closed presentation modes supported by the version-1 profile contract.
///
/// These modes change only the provider-facing definition list. They do not
/// install Tools, alter permissions, or select an A3S Use generation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPresentationModeV1 {
    /// Preserve the historical prompt-sensitive Tool selector.
    #[default]
    Adaptive,
    /// Present every Tool definition admitted by the run-owned permission
    /// visibility boundary.
    Direct,
    /// Present the existing `program` Tool as a compact code gateway over the
    /// same governed executor.
    Code,
    /// Present no Tools. Host-direct and other governed execution APIs remain
    /// unchanged because this is a presentation choice, not authorization.
    Disabled,
}

impl ToolPresentationModeV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Adaptive => "adaptive",
            Self::Direct => "direct",
            Self::Code => "code",
            Self::Disabled => "disabled",
        }
    }
}

/// Versioned, serializable Tool-presentation policy frozen by a Session and
/// copied into each admitted Run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPresentationProfileV1 {
    schema: String,
    mode: ToolPresentationModeV1,
}

impl ToolPresentationProfileV1 {
    pub fn adaptive() -> Self {
        Self::new(ToolPresentationModeV1::Adaptive)
    }

    pub fn direct() -> Self {
        Self::new(ToolPresentationModeV1::Direct)
    }

    pub fn code() -> Self {
        Self::new(ToolPresentationModeV1::Code)
    }

    pub fn disabled() -> Self {
        Self::new(ToolPresentationModeV1::Disabled)
    }

    fn new(mode: ToolPresentationModeV1) -> Self {
        Self {
            schema: TOOL_PRESENTATION_PROFILE_V1_SCHEMA.to_owned(),
            mode,
        }
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub const fn mode(&self) -> ToolPresentationModeV1 {
        self.mode
    }

    pub fn validate(&self) -> Result<(), ToolPresentationError> {
        if self.schema != TOOL_PRESENTATION_PROFILE_V1_SCHEMA {
            return Err(ToolPresentationError::UnsupportedSchema);
        }
        Ok(())
    }

    /// Reject a child mode that could expose a shape outside its parent.
    ///
    /// `direct` is the widest root mode. `adaptive` and `code` are intentionally
    /// incomparable because each can expose a definition the other omits for a
    /// given prompt. `disabled` is always a valid child.
    pub fn ensure_within(&self, parent: &Self) -> Result<(), ToolPresentationError> {
        self.validate()?;
        parent.validate()?;
        let within = self.mode == ToolPresentationModeV1::Disabled
            || self.mode == parent.mode
            || parent.mode == ToolPresentationModeV1::Direct;
        if within {
            Ok(())
        } else {
            Err(ToolPresentationError::ProfileExpansion {
                parent: parent.mode.as_str(),
                child: self.mode.as_str(),
            })
        }
    }

    /// Project the exact Tool definitions presented for one message context.
    ///
    /// Callers must first remove definitions hidden by run-owned permission
    /// policy. The returned vector is canonical by Tool name and is validated
    /// as a definition-only projection of that source.
    pub fn present_for_messages(
        &self,
        source: &[ToolDefinition],
        messages: &[Message],
    ) -> Result<Vec<ToolDefinition>, ToolPresentationError> {
        self.validate()?;
        let source = canonical_source(source)?;
        let projected = match self.mode {
            ToolPresentationModeV1::Adaptive => {
                selector::select_tools_for_messages(&source, messages)
            }
            ToolPresentationModeV1::Direct => source.clone(),
            ToolPresentationModeV1::Code => code_projection(&source),
            ToolPresentationModeV1::Disabled => Vec::new(),
        };
        validate_projection(&source, &projected)?;
        Ok(projected)
    }

    /// Project definitions from a plain prompt without constructing a full
    /// conversation history.
    pub fn present_for_prompt(
        &self,
        source: &[ToolDefinition],
        prompt: &str,
    ) -> Result<Vec<ToolDefinition>, ToolPresentationError> {
        self.present_for_messages(source, &[Message::user(prompt)])
    }
}

impl Default for ToolPresentationProfileV1 {
    fn default() -> Self {
        Self::adaptive()
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ToolPresentationError {
    #[error("Tool presentation profile uses an unsupported schema")]
    UnsupportedSchema,
    #[error("Tool presentation source exceeds the {max} definition limit")]
    ToolLimitExceeded { max: usize },
    #[error("Tool presentation source contains duplicate name '{name}'")]
    DuplicateToolName { name: String },
    #[error("Tool presentation projected unknown Tool '{name}'")]
    UnknownProjectedTool { name: String },
    #[error("Tool presentation changed the parameter schema for Tool '{name}'")]
    ParameterSchemaChanged { name: String },
    #[error("Tool presentation submitted an unexpected description for Tool '{name}'")]
    DescriptionChanged { name: String },
    #[error("Tool presentation output is not in canonical Tool-name order")]
    NonCanonicalOrder,
    #[error("Tool presentation child mode '{child}' broadens parent mode '{parent}'")]
    ProfileExpansion {
        parent: &'static str,
        child: &'static str,
    },
}

pub(crate) fn estimated_definition_tokens(tools: &[ToolDefinition]) -> usize {
    estimate_prompt_tokens(&[], None, tools)
}

pub(crate) fn canonical_source(
    source: &[ToolDefinition],
) -> Result<Vec<ToolDefinition>, ToolPresentationError> {
    if source.len() > MAX_PRESENTATION_TOOLS {
        return Err(ToolPresentationError::ToolLimitExceeded {
            max: MAX_PRESENTATION_TOOLS,
        });
    }
    let mut canonical = source.to_vec();
    canonical.sort_by(|left, right| left.name.cmp(&right.name));
    for pair in canonical.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(ToolPresentationError::DuplicateToolName {
                name: pair[0].name.clone(),
            });
        }
    }
    Ok(canonical)
}

pub(crate) fn is_definition_subset(
    source: &[ToolDefinition],
    projected: &[ToolDefinition],
) -> Result<bool, ToolPresentationError> {
    validate_projection(source, projected)?;
    let source = source
        .iter()
        .map(|definition| (definition.name.as_str(), definition))
        .collect::<BTreeMap<_, _>>();
    for definition in projected {
        let Some(original) = source.get(definition.name.as_str()) else {
            return Err(ToolPresentationError::UnknownProjectedTool {
                name: definition.name.clone(),
            });
        };
        if definition.description != original.description {
            return Err(ToolPresentationError::DescriptionChanged {
                name: definition.name.clone(),
            });
        }
    }
    Ok(true)
}

fn validate_projection(
    source: &[ToolDefinition],
    projected: &[ToolDefinition],
) -> Result<(), ToolPresentationError> {
    let source = source
        .iter()
        .map(|definition| (definition.name.as_str(), definition))
        .collect::<BTreeMap<_, _>>();
    let mut prior_name: Option<&str> = None;
    let mut seen = HashSet::with_capacity(projected.len());
    for definition in projected {
        if prior_name.is_some_and(|prior| prior >= definition.name.as_str()) {
            return Err(ToolPresentationError::NonCanonicalOrder);
        }
        prior_name = Some(&definition.name);
        if !seen.insert(definition.name.as_str()) {
            return Err(ToolPresentationError::DuplicateToolName {
                name: definition.name.clone(),
            });
        }
        let Some(original) = source.get(definition.name.as_str()) else {
            return Err(ToolPresentationError::UnknownProjectedTool {
                name: definition.name.clone(),
            });
        };
        if definition.parameters != original.parameters {
            return Err(ToolPresentationError::ParameterSchemaChanged {
                name: definition.name.clone(),
            });
        }
    }
    Ok(())
}

fn code_projection(source: &[ToolDefinition]) -> Vec<ToolDefinition> {
    let Some(program) = source
        .iter()
        .find(|definition| definition.name == "program")
    else {
        // Permission policy may hide the gateway. Failing closed to an empty
        // presentation keeps that policy authoritative without blocking an
        // otherwise tool-free model call.
        return Vec::new();
    };
    let mut program = program.clone();
    let catalog = compact_code_catalog(source);
    program.description = format!(
        "Run a sandboxed JavaScript program through the governed Tool executor. Define async function run(ctx, inputs), call await ctx.tool(name, args), and set allowed_tools to the smallest required subset. Every nested call keeps the Run permission, confirmation, cancellation, and audit boundaries. Available Tool signatures: {catalog}"
    );
    vec![program]
}

fn compact_code_catalog(source: &[ToolDefinition]) -> String {
    let mut output = String::new();
    let mut omitted = 0usize;
    for definition in source
        .iter()
        .filter(|definition| definition.name != "program")
    {
        let signature = compact_signature(definition);
        let separator = if output.is_empty() { "" } else { "; " };
        if output
            .len()
            .saturating_add(separator.len())
            .saturating_add(signature.len())
            > MAX_CODE_CATALOG_BYTES
        {
            omitted = omitted.saturating_add(1);
            continue;
        }
        output.push_str(separator);
        output.push_str(&signature);
    }
    if omitted > 0 {
        output.push_str(&format!("; ... {omitted} additional Tools omitted"));
    }
    if output.is_empty() {
        "none".to_owned()
    } else {
        output
    }
}

fn compact_signature(definition: &ToolDefinition) -> String {
    let required = definition
        .parameters
        .get("required")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let mut description = definition.description.lines().next().unwrap_or("").trim();
    if description.len() > MAX_CODE_CATALOG_DESCRIPTION_BYTES {
        let mut boundary = MAX_CODE_CATALOG_DESCRIPTION_BYTES;
        while !description.is_char_boundary(boundary) {
            boundary -= 1;
        }
        description = &description[..boundary];
    }
    format!("{}({required}) {description}", definition.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition(name: &str, description: &str, required: &[&str]) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters: json!({
                "type": "object",
                "properties": required
                    .iter()
                    .map(|name| ((*name).to_owned(), json!({"type": "string"})))
                    .collect::<serde_json::Map<_, _>>(),
                "required": required,
            }),
        }
    }

    #[test]
    fn profiles_are_closed_serializable_values() {
        for (profile, mode) in [
            (ToolPresentationProfileV1::adaptive(), "adaptive"),
            (ToolPresentationProfileV1::direct(), "direct"),
            (ToolPresentationProfileV1::code(), "code"),
            (ToolPresentationProfileV1::disabled(), "disabled"),
        ] {
            profile.validate().unwrap();
            let value = serde_json::to_value(&profile).unwrap();
            assert_eq!(value["schema"], TOOL_PRESENTATION_PROFILE_V1_SCHEMA);
            assert_eq!(value["mode"], mode);
            assert_eq!(
                serde_json::from_value::<ToolPresentationProfileV1>(value).unwrap(),
                profile
            );
        }
    }

    #[test]
    fn projection_is_canonical_and_can_only_rephrase_existing_definitions() {
        let source = vec![
            definition("write", "Write a file", &["file_path", "content"]),
            definition("program", "Original program description", &["type"]),
            definition("read", "Read a file", &["file_path"]),
        ];
        let projected = ToolPresentationProfileV1::code()
            .present_for_prompt(&source, "change the file")
            .unwrap();

        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].name, "program");
        assert_eq!(projected[0].parameters, source[1].parameters);
        assert_ne!(projected[0].description, source[1].description);
        assert!(projected[0].description.contains("read(file_path)"));
        assert!(projected[0]
            .description
            .contains("write(file_path,content)"));
    }

    #[test]
    fn direct_and_adaptive_outputs_have_deterministic_order() {
        let source = vec![
            definition("write", "Write", &[]),
            definition("bash", "Execute", &[]),
            definition("read", "Read", &[]),
        ];
        for profile in [
            ToolPresentationProfileV1::direct(),
            ToolPresentationProfileV1::adaptive(),
        ] {
            let names = profile
                .present_for_prompt(&source, "inspect and update the project")
                .unwrap()
                .into_iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>();
            assert_eq!(names, vec!["bash", "read", "write"]);
        }
    }

    #[test]
    fn code_profile_reduces_large_direct_definition_cost() {
        let mut source = vec![definition("program", "Program", &["type"])];
        for index in 0..40 {
            source.push(definition(
                &format!("tool_{index:02}"),
                &"long model-facing description ".repeat(20),
                &["input", "path", "mode"],
            ));
        }
        let direct = ToolPresentationProfileV1::direct()
            .present_for_prompt(&source, "work")
            .unwrap();
        let code = ToolPresentationProfileV1::code()
            .present_for_prompt(&source, "work")
            .unwrap();

        assert_eq!(code.len(), 1);
        assert!(estimated_definition_tokens(&code) < estimated_definition_tokens(&direct));
    }

    #[test]
    fn child_profile_partial_order_rejects_cross_mode_broadening() {
        let direct = ToolPresentationProfileV1::direct();
        let adaptive = ToolPresentationProfileV1::adaptive();
        let code = ToolPresentationProfileV1::code();
        let disabled = ToolPresentationProfileV1::disabled();

        adaptive.ensure_within(&direct).unwrap();
        code.ensure_within(&direct).unwrap();
        disabled.ensure_within(&adaptive).unwrap();
        adaptive.ensure_within(&adaptive).unwrap();
        assert!(matches!(
            code.ensure_within(&adaptive),
            Err(ToolPresentationError::ProfileExpansion { .. })
        ));
        assert!(matches!(
            adaptive.ensure_within(&code),
            Err(ToolPresentationError::ProfileExpansion { .. })
        ));
    }
}
