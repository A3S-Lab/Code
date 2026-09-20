use super::*;

const HOST_RESULT_TOKEN: &str = "HOST-RESULT-91";

struct HostProbeTool;

#[async_trait::async_trait]
impl crate::tools::Tool for HostProbeTool {
    fn name(&self) -> &str {
        "host_probe"
    }

    fn description(&self) -> &str {
        "Return a fixed host token."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {}
        })
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        Ok(crate::tools::ToolOutput::success(HOST_RESULT_TOKEN))
    }
}

struct RecordingHostClient {
    turns: Arc<std::sync::Mutex<Vec<String>>>,
    model_turns: std::sync::atomic::AtomicUsize,
}

impl RecordingHostClient {
    fn new(turns: Arc<std::sync::Mutex<Vec<String>>>) -> Self {
        Self {
            turns,
            model_turns: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn next_response(
        &self,
        messages: &[crate::llm::Message],
        system: Option<&str>,
    ) -> anyhow::Result<LlmResponse> {
        if system.is_some_and(|value| value.contains(crate::prompts::PRE_ANALYSIS_SYSTEM)) {
            let prompt = messages
                .last()
                .map(crate::llm::Message::text)
                .unwrap_or_default();
            return Ok(scripted_text_response(
                &serde_json::json!({
                    "intent": "GeneralPurpose",
                    "requires_planning": false,
                    "goal": { "description": prompt, "success_criteria": [] },
                    "execution_plan": {
                        "complexity": "Simple",
                        "steps": [{
                            "id": "s1",
                            "description": prompt,
                            "dependencies": [],
                            "success_criteria": "Complete the request"
                        }]
                    },
                    "optimized_input": prompt
                })
                .to_string(),
            ));
        }
        self.turns
            .lock()
            .expect("host adapter turn log")
            .push(serde_json::to_string(messages).unwrap_or_default());
        let turn = self
            .model_turns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if turn == 0 {
            Ok(scripted_tool_call_response(
                "probe-1",
                "host_probe",
                serde_json::json!({}),
            ))
        } else {
            Ok(scripted_text_response("done"))
        }
    }
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for RecordingHostClient {
    async fn complete(
        &self,
        messages: &[crate::llm::Message],
        system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.next_response(messages, system)
    }

    async fn complete_streaming(
        &self,
        messages: &[crate::llm::Message],
        system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let response = self.next_response(messages, system)?;
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let text = response.text();
            if !text.is_empty() {
                let _ = tx.send(StreamEvent::TextDelta(text)).await;
            }
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn custom_host_adapter_receives_the_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let turns = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let session = agent
        .session_async(
            dir.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_llm_client(Arc::new(RecordingHostClient::new(Arc::clone(&turns))))
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_continuation(false)
                    .with_confirmation_manager(Arc::new(crate::hitl::AutoApproveConfirmation)),
            ),
        )
        .await
        .unwrap();
    session
        .register_dynamic_tool(Arc::new(HostProbeTool))
        .unwrap();

    let result = session.send("call host_probe", None).await.unwrap();
    assert_eq!(result.text, "done");

    let recorded = turns.lock().expect("host adapter turn log").clone();
    assert!(
        recorded.len() >= 2,
        "host adapter saw {} model turns, expected the tool follow-up",
        recorded.len()
    );
    assert!(
        !recorded[0].contains(HOST_RESULT_TOKEN),
        "tool result leaked into the request that asked for the tool"
    );
    assert!(
        recorded[1..]
            .iter()
            .any(|turn| turn.contains(HOST_RESULT_TOKEN)),
        "host adapter follow-up omitted the tool result: {recorded:?}"
    );
}
