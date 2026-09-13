//! Structured question. Parks without writing, granting permission, or steering.

use crate::agent::AgentEvent;
use crate::ask_user::{self, AskUserError, AskUserResume};
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;

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
        let (event, rx) =
            match ask_user::begin(run_id, &question_id, question, &options, allow_free_text) {
                Ok(pair) => pair,
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
            });
        }
        let question_id = event.question_id.clone();
        let resume = match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(resume)) => resume,
            _ => {
                ask_user::cancel(&question_id);
                AskUserResume::Unanswered
            }
        };
        let content = ask_user::resume_message(&question_id, &resume);
        Ok(
            ToolOutput::success(content).with_metadata(serde_json::json!({
                "schema": ask_user::USER_ANSWER_SCHEMA,
                "permission_grant": false,
                "wrote_files": false,
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
            .with_session_id(session)
            .with_agent_event_tx(event_tx);
        let args = serde_json::json!({
            "question": "Which name?",
            "options": ["left", "right"]
        });
        let execute = tokio::spawn(async move { tool.execute(&args, &ctx).await });
        let answered = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match events.recv().await {
                    Ok(crate::agent::AgentEvent::UserQuestion { question_id, .. })
                        if ask_user::answer(&question_id, "left") =>
                    {
                        return;
                    }
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        })
        .await;
        assert!(answered.is_ok(), "the parked question was not answerable");
        let output = execute.await.unwrap().unwrap();
        assert!(output.success);
        assert!(output.content.contains("\"answer\":\"left\"") || output.content.contains("left"));
        let metadata = output.metadata.expect("answer metadata");
        assert_eq!(metadata["permission_grant"], false);
        assert_eq!(metadata["wrote_files"], false);
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());

        let message = crate::llm::Message::tool_result("ask-1", &output.content, false);
        let visible = message.content.iter().find_map(|block| match block {
            ContentBlock::ToolResult {
                content: ToolResultContentField::Text(text),
                ..
            } => Some(text.as_str()),
            _ => None,
        });
        assert!(
            visible.is_some_and(|text| text.contains("left")),
            "the answer must be the next model-visible tool result"
        );
    }
}
