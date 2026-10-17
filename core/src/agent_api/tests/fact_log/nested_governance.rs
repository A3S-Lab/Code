#![cfg(test)]

//! Calls an orchestrator dispatches inside a fact run cross the same gates
//! as the call the model made directly.

use super::*;

struct SideEffect {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl crate::tools::Tool for SideEffect {
    fn name(&self) -> &str {
        "side_effect"
    }

    fn description(&self) -> &str {
        "Count one side effect"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(crate::tools::ToolOutput::success("side-effect-ran"))
    }
}

async fn run_orchestrated(session_id: &str, call: LlmResponse) -> usize {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let session = session_for(
        &agent,
        workspace.path(),
        session_id,
        Arc::new(ScriptedClient {
            seen: Arc::new(std::sync::Mutex::new(Vec::new())),
            responses: std::sync::Mutex::new(vec![call, text_response("done")]),
        }),
        &crate::SessionOptions::new().with_permission_policy(
            crate::permissions::PermissionPolicy::new()
                .allow("*")
                .deny("side_effect"),
        ),
    );
    session
        .register_dynamic_tool(Arc::new(SideEffect {
            calls: Arc::clone(&calls),
        }))
        .unwrap();
    session.send("orchestrate", None).await.unwrap();
    calls.load(Ordering::SeqCst)
}

#[tokio::test]
async fn fact_run_batch_nested_call_obeys_the_session_deny() {
    let ran = run_orchestrated(
        "nested-batch-deny",
        tool_response(
            "batch-1",
            "batch",
            serde_json::json!({"invocations": [{"tool": "side_effect", "args": {}}]}),
        ),
    )
    .await;
    assert_eq!(ran, 0, "a denied tool must not run through batch");
}

#[tokio::test]
async fn fact_run_program_nested_call_obeys_the_session_deny() {
    let ran = run_orchestrated(
        "nested-program-deny",
        tool_response(
            "program-1",
            "program",
            serde_json::json!({
                "type": "script",
                "language": "javascript",
                "source": "async function run(ctx) { return await ctx.tool('side_effect', {}); }",
                "allowed_tools": ["side_effect"]
            }),
        ),
    )
    .await;
    assert_eq!(ran, 0, "a denied tool must not run through program");
}
