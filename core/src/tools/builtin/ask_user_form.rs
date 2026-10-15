//! Structured form question. The client renders OpenUI Lang instead of
//! options; the call stays open until the member submits, and the tool result
//! carries the submitted field values as JSON.

use crate::agent::AgentEvent;
use crate::ask_user::{self, AskUserError};
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

pub const USER_FORM_SCHEMA: &str = "a3s.code.user-form.v1";
const MAX_FORM_BYTES: usize = 65_536;

pub struct AskUserFormTool;

#[async_trait]
impl Tool for AskUserFormTool {
    fn name(&self) -> &str {
        "ask_user_form"
    }

    fn description(&self) -> &str {
        "Pause for structured user input rendered as a form. Use whenever a tool result needs \
         more information from the user (a platform card asking for missing fields, a plain-text \
         follow-up question, or a confirmation before an action) instead of asking in prose. \
         Build OpenUI Lang: one statement per line, `root = Component(...)`, arguments in \
         catalog prop order. Use Card, Stack, Text, Input, Textarea, Select, Checkbox, \
         Slider, Metric, BarChart and one Button(\"submit\", \"label\"). No Markdown fence, \
         imports, DOM tags or JavaScript calls. Write the card title, field labels, \
         placeholders, options and button label in the language the user writes in; keep \
         field names as ASCII identifiers. The submitted named field values arrive \
         as the tool result JSON and the turn continues with them."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "form": {
                    "type": "string",
                    "description": "OpenUI Lang source. The first statement is root = Component(...). No Markdown fence."
                },
                "prompt": {
                    "type": "string",
                    "description": "One-line context shown with the form"
                }
            },
            "required": ["form"]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> ToolCapabilities {
        ToolCapabilities {
            read_only: true,
            idempotent: false,
            resumable: false,
            cancellation_safe: true,
            supports_pagination: false,
            max_parallelism: 1,
            output_kind: crate::tools::ToolOutputKind::Structured,
        }
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(form) = args.get("form").and_then(|value| value.as_str()) else {
            return Ok(ToolOutput::error("form parameter is required"));
        };
        let prompt = args
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or("请填写以下表单");
        if let Err(message) = check_form_shape(form) {
            return Ok(ToolOutput::error(message));
        }
        let question_id = format!(
            "ask-form-{}-{}",
            ctx.session_id.as_deref().unwrap_or("run"),
            ctx.run_id().unwrap_or("0")
        );
        let run_id = ctx.session_id.as_deref().unwrap_or("run");
        let event = match ask_user::begin(run_id, &question_id, prompt, &[], true) {
            Ok(event) => event,
            Err(AskUserError::CapExceeded) => {
                return Ok(ToolOutput::error("ask_user question cap exceeded"));
            }
            Err(AskUserError::Invalid(message)) => return Ok(ToolOutput::error(message)),
        };
        if let Some(tx) = &ctx.agent_event_tx {
            let _ = tx.send(AgentEvent::UserQuestion {
                question_id: event.question_id.clone(),
                question: event.question.clone(),
                options: event.options.clone(),
                allow_free_text: event.allow_free_text,
                form: Some(form.to_owned()),
            });
        }
        let cancel = ctx.cancellation_token();
        let Some(answer) = ask_user::wait_for_answer(&event.question_id, &cancel).await else {
            return Ok(ToolOutput::error(
                "form was cancelled before it was submitted",
            ));
        };
        let Some(values) = submitted_values(&answer) else {
            return Ok(ToolOutput::error("the client returned no form values"));
        };
        let content = serde_json::json!({
            "schema": USER_FORM_SCHEMA,
            "question_id": event.question_id,
            "status": "answered",
            "values": values,
            "permission_grant": false,
        })
        .to_string();
        Ok(
            ToolOutput::success(content).with_metadata(serde_json::json!({
                "schema": USER_FORM_SCHEMA,
                "permission_grant": false,
                "wrote_files": false,
            })),
        )
    }
}

/// The host answers with the submitted values object; JSON `null` means the
/// client could not render the form or the member left it unsubmitted.
fn submitted_values(answer: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(answer)
        .ok()
        .filter(serde_json::Value::is_object)
}

/// Source envelope guard. The session renderer validates every component name.
fn check_form_shape(form: &str) -> Result<(), &'static str> {
    let form = form.trim();
    if form.is_empty() {
        return Err("form must not be empty");
    }
    if form.len() > MAX_FORM_BYTES {
        return Err("form exceeds its size bound");
    }
    if !form
        .lines()
        .any(|line| line.trim_start().starts_with("root = "))
    {
        return Err("form must be OpenUI Lang and assign root");
    }
    Ok(())
}

#[cfg(test)]
#[path = "ask_user_form_tests.rs"]
mod tests;
