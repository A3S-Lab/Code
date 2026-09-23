//! Structured question. Parks on a fact, without granting permission or steering.

use crate::agent::AgentEvent;
use crate::ask_user::{self, AskUserError};
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

pub struct AskUserTool;

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Pause for a structured user answer. This does not write files, grant a permission, or steer the run."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "question": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" }, "maxItems": 8 },
                "allow_free_text": { "type": "boolean" }
            },
            "required": ["question"]
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
        let Some(question) = args.get("question").and_then(|value| value.as_str()) else {
            return Ok(ToolOutput::error("question parameter is required"));
        };
        let options = args
            .get("options")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let allow_free_text = args
            .get("allow_free_text")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let question_id = format!(
            "ask-{}-{}",
            ctx.session_id.as_deref().unwrap_or("run"),
            ctx.run_id().unwrap_or("0")
        );
        let run_id = ctx.session_id.as_deref().unwrap_or("run");
        let event = match ask_user::begin(run_id, &question_id, question, &options, allow_free_text)
        {
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
            });
        }
        let thread = crate::fact_control::thread_for_session(run_id);
        let payload = serde_json::to_value(a3s_effect::ModelDecision::Question {
            question_id: event.question_id.clone(),
            question: event.question.clone(),
            allow_free_text: event.allow_free_text,
            options: event.options.clone(),
        })
        .unwrap_or_else(|error| serde_json::json!({ "kind": "text", "text": error.to_string() }));
        let log = match a3s_effect::FileLog::open(ctx.workspace.join(".a3s").join("effect-log")) {
            Ok(log) => log,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        if let Err(error) = a3s_effect::LogStore::append(
            &log,
            &thread,
            &[a3s_effect::NewFact {
                kind: "model.turn".into(),
                key: format!("ask:{}", event.question_id),
                payload,
            }],
            None,
        ) {
            return Ok(ToolOutput::error(error.to_string()));
        }
        let content = serde_json::json!({
            "schema": ask_user::USER_QUESTION_SCHEMA,
            "question_id": event.question_id,
            "status": "parked",
            "allow_free_text": event.allow_free_text,
            "options": event.options,
            "permission_grant": false,
        })
        .to_string();
        Ok(
            ToolOutput::success(content).with_metadata(serde_json::json!({
                "schema": ask_user::USER_QUESTION_SCHEMA,
                "permission_grant": false,
                "wrote_files": false,
                "parked_on_log": true,
            })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, ToolResultContentField};

    #[tokio::test]
    async fn answer_is_on_the_next_model_tool_result_and_is_not_a_grant() {
        let root = tempfile::tempdir().unwrap();
        let session = format!(
            "ask-{}",
            root.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        let (event_tx, mut events) = tokio::sync::broadcast::channel(4);
        let tool = AskUserTool;
        let ctx = ToolContext::new(root.path().to_path_buf())
            .with_session_id(&session)
            .with_agent_event_tx(event_tx);
        let args = serde_json::json!({
            "question": "Which name?",
            "options": ["left", "right"]
        });
        let output = tool.execute(&args, &ctx).await.unwrap();
        let event = events.recv().await.unwrap();
        match event {
            crate::agent::AgentEvent::UserQuestion {
                options,
                allow_free_text,
                ..
            } => {
                assert_eq!(options, vec!["left".to_string(), "right".to_string()]);
                assert!(!allow_free_text);
            }
            other => panic!("expected a question event, got {other:?}"),
        }
        assert!(output.success);
        assert!(output.content.contains("\"status\":\"parked\""));
        let metadata = output.metadata.expect("answer metadata");
        assert_eq!(metadata["permission_grant"], false);
        assert_eq!(metadata["wrote_files"], false);
        let thread = crate::fact_control::thread_for_session(&session);
        let facts = crate::fact_control::read_workspace_facts(root.path(), &thread).unwrap();
        let turn = facts
            .iter()
            .find(|fact| fact.kind == "model.turn")
            .expect("question fact");
        assert_eq!(turn.payload["allow_free_text"], false);
        assert_eq!(turn.payload["options"][0], "left");
        assert_eq!(turn.payload["options"][1], "right");

        let message = crate::llm::Message::tool_result("ask-1", &output.content, false);
        let visible = message.content.iter().find_map(|block| match block {
            ContentBlock::ToolResult {
                content: ToolResultContentField::Text(text),
                ..
            } => Some(text.as_str()),
            _ => None,
        });
        assert!(
            visible.is_some_and(|text| text.contains("\"status\":\"parked\"") && text.contains("left")),
            "the parked question, including its options, is the tool result"
        );
    }
}
