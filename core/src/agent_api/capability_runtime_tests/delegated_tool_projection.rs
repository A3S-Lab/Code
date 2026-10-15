#![cfg(test)]

use super::*;

struct DelegatingClient {
    parent_calls: AtomicUsize,
    child_calls: AtomicUsize,
    allowed: bool,
}

#[async_trait]
impl LlmClient for DelegatingClient {
    async fn complete(
        &self,
        messages: &[Message],
        _: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if tools.iter().any(|tool| tool.name == "task") {
            if self.parent_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut response = CutoverClient::tool_call("delegate-plugin");
                response.message.content = vec![ContentBlock::ToolUse {
                    id: "delegate-plugin".to_owned(),
                    name: "task".to_owned(),
                    input: serde_json::json!({"tasks": [{"agent": "general", "description": "Invoke projected plugin", "prompt": "Invoke projected exactly once."}]}),
                }];
                return Ok(response);
            }
            let output = messages
                .iter()
                .rev()
                .flat_map(|message| message.content.iter().rev())
                .find_map(|block| {
                    if let ContentBlock::ToolResult { content, .. } = block {
                        Some(content.as_text())
                    } else {
                        None
                    }
                })
                .ok_or_else(|| anyhow::anyhow!("no Task result"))?;
            anyhow::ensure!(
                output.contains(if self.allowed {
                    "child accepted"
                } else {
                    "child refused"
                }),
                "delegated plugin outcome is missing: {output}"
            );
            return Ok(CutoverClient::final_text("delegation settled"));
        }
        let call = self.child_calls.load(Ordering::SeqCst);
        if call == 0 {
            anyhow::ensure!(
                tools.iter().any(|tool| tool.name == "projected") == self.allowed,
                "child Tool surface differs from accepted projected authority"
            );
            self.child_calls.fetch_add(1, Ordering::SeqCst);
            return Ok(CutoverClient::tool_call("delegated"));
        }
        let result = messages
            .iter()
            .rev()
            .flat_map(|message| message.content.iter().rev())
            .find_map(|block| {
                if let ContentBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    Some((content.as_text(), *is_error))
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow::anyhow!("no child Tool result"))?;
        if self.allowed {
            anyhow::ensure!(
                result.0.contains("accepted") && result.1 != Some(true),
                "child did not execute accepted Tool: {result:?}"
            );
        } else {
            anyhow::ensure!(
                result.1 == Some(true),
                "child requested denied Tool without an error"
            );
        }
        Ok(CutoverClient::final_text(if self.allowed {
            "child accepted"
        } else {
            "child refused"
        }))
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is outside this contract")
    }
}

async fn delegated_access(name: &str, allowed: bool) {
    let client = Arc::new(DelegatingClient {
        parent_calls: AtomicUsize::new(0),
        child_calls: AtomicUsize::new(0),
        allowed,
    });
    let session = test_session_with_client(name, client.clone()).await;
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let counter = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &counter, &counter))
            .unwrap()
            .with_allowed_capabilities(if allowed {
                vec![ids["projected"].clone()]
            } else {
                vec![]
            })
            .unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_owned(),
                version: "accepted",
                executions: executions.clone(),
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        session.send("Delegate the plugin call.", None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.text, "delegation settled");
    assert!(client.child_calls.load(Ordering::SeqCst) > 0);
    assert_eq!(
        &*executions.lock().unwrap(),
        if allowed { &["accepted"][..] } else { &[][..] }
    );
    session.close().await;
}

#[tokio::test]
async fn actual_task_inherits_the_accepted_projected_tool() {
    delegated_access("delegated-plugin-tool-allowed", true).await;
}

#[tokio::test]
async fn actual_task_cannot_obtain_a_denied_projected_tool_by_name() {
    delegated_access("delegated-plugin-tool-denied", false).await;
}
