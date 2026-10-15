#![cfg(test)]

use super::*;
use crate::capability::{WorkspaceOperation, WorkspaceOperationAccess};
use crate::workspace::{WorkspaceCapabilities, WorkspaceServices, WorkspaceServicesBuilder};

async fn restricted_session(
    name: &str,
    client: Arc<dyn LlmClient>,
    flags: WorkspaceCapabilities,
) -> AgentSession {
    let workspace = format!("/tmp/{name}");
    crate::fact_control::reset_session_fact_log(&workspace);
    std::fs::create_dir_all(&workspace).unwrap();
    let base = if flags.code_intelligence {
        WorkspaceServices::local_with_code_intelligence(&workspace, name)
            .await
            .unwrap()
    } else {
        WorkspaceServices::local(std::path::Path::new(&workspace))
    };
    let mut builder =
        WorkspaceServicesBuilder::new(base.workspace_ref().clone(), base.fs()).capabilities(flags);
    if flags.exec {
        builder = builder.command_runner(base.command_runner().unwrap());
    }
    if flags.search {
        builder = builder.search(base.search().unwrap());
    }
    if flags.git {
        builder = builder.git(base.git().unwrap());
    }
    if flags.code_intelligence {
        builder = builder.code_intelligence(base.code_intelligence().unwrap());
    }
    let services = builder.build();
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    agent
        .build_session(
            workspace,
            client,
            &SessionOptions::new()
                .with_session_id(name)
                .with_workspace_backend(services)
                .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap()
}

#[tokio::test]
async fn actual_run_ceiling_uses_each_backend_flag_and_context_replacement_cannot_expand_it() {
    let operations = [
        WorkspaceOperation::Read,
        WorkspaceOperation::Write,
        WorkspaceOperation::Execute,
        WorkspaceOperation::Search,
        WorkspaceOperation::Git,
        WorkspaceOperation::CodeIntelligence,
    ];
    for (index, allowed) in operations.iter().enumerate() {
        let flags = WorkspaceCapabilities {
            read: index == 0,
            write: index == 1,
            exec: index == 2,
            search: index == 3,
            git: index == 4,
            code_intelligence: index == 5,
        };
        let session = restricted_session(
            &format!("workspace-access-{index}"),
            Arc::new(NoopClient),
            flags,
        )
        .await;
        let run = session.admit_capability_run().await.unwrap();
        let turn = crate::capability::AgentCapabilityRuntime::from_run(run.run_scope())
            .begin_turn(1)
            .unwrap();
        let ctx = ToolContext::new(session.workspace.clone())
            .with_capability_context(turn.tool_context());
        for operation in operations {
            let result = ctx.workspace_operation(operation);
            assert_eq!(
                result.is_ok(),
                operation == *allowed,
                "{}",
                operation.as_str()
            );
        }
        let access = ctx.workspace_operation(*allowed).unwrap();
        assert_eq!(
            access.check().unwrap().catalog_digest(),
            run.run_scope().catalog_digest()
        );
        let cancellation = access.cancellation().unwrap();
        turn.close().await.unwrap();
        assert!(cancellation.is_cancelled());
        assert!(access.check().is_err());
        assert!(ctx.workspace_operation(*allowed).is_err());
        run.close().await.unwrap();
        session.close().await;
    }
}

struct ResourceTool {
    access: Arc<Mutex<Option<WorkspaceOperationAccess>>>,
    closed: Arc<AtomicUsize>,
}

struct ResourceEffect(Arc<AtomicUsize>);

#[async_trait]
impl CapabilityEffect for ResourceEffect {
    fn name(&self) -> &str {
        "resource.read"
    }
    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl Tool for ResourceTool {
    fn name(&self) -> &str {
        "projected"
    }
    fn description(&self) -> &str {
        "Read an owned resource"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::parallel_safe_read(1)
    }
    async fn execute(
        &self,
        _: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let access = ctx.workspace_operation(WorkspaceOperation::Read)?;
        let accepted = access.check()?;
        anyhow::ensure!(accepted.scope_id().as_str() == ctx.capability_scope_id().unwrap());
        anyhow::ensure!(
            accepted.use_generation().is_some(),
            "actual Use identity missing"
        );
        anyhow::ensure!(ctx.workspace_operation(WorkspaceOperation::Write).is_err());
        // Caller-controlled service metadata cannot manufacture scope rights.
        let replaced = ctx
            .clone()
            .with_workspace_services(WorkspaceServices::local(&ctx.workspace));
        anyhow::ensure!(replaced
            .workspace_operation(WorkspaceOperation::Execute)
            .is_err());
        ctx.register_capability_effect(ResourceEffect(self.closed.clone()))?;
        *self.access.lock().unwrap() = Some(access);
        Ok(ToolOutput::success("owned read"))
    }
}

struct ResourceClient {
    calls: AtomicUsize,
    access: Arc<Mutex<Option<WorkspaceOperationAccess>>>,
    closed: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for ResourceClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(CutoverClient::tool_call("resource-proof"));
        }
        let access = self
            .access
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no actual resource call"))?;
        anyhow::ensure!(
            self.closed.load(Ordering::SeqCst) == 1,
            "resource has not settled"
        );
        anyhow::ensure!(
            access.check().is_err(),
            "closed Turn still authorizes resource operations"
        );
        Ok(CutoverClient::final_text("resource closed"))
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("not a streaming contract")
    }
}

#[tokio::test]
async fn actual_fact_harness_resource_access_is_revoked_before_next_inference() {
    let access = Arc::new(Mutex::new(None));
    let closed = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(ResourceClient {
        calls: AtomicUsize::new(0),
        access: access.clone(),
        closed: closed.clone(),
    });
    let session = restricted_session(
        "workspace-access-harness",
        client,
        WorkspaceCapabilities {
            read: true,
            write: false,
            exec: false,
            search: false,
            git: false,
            code_intelligence: false,
        },
    )
    .await;
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(ResourceTool {
                access: access.clone(),
                closed: closed.clone(),
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let result = session.send("Read the resource.", None).await.unwrap();
    assert_eq!(result.text, "resource closed");
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "saved access retained the Use lease"
    );
    session.close().await;
}
