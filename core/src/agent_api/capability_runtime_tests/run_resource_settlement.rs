#![cfg(test)]

use super::*;
use std::path::PathBuf;

struct Resource {
    path: PathBuf,
    closing: Semaphore,
    release: Semaphore,
    fail: bool,
    context: Mutex<Option<ToolContext>>,
}

struct RunEffect(Arc<Resource>);
#[async_trait]
impl CapabilityEffect for RunEffect {
    fn name(&self) -> &str {
        "plugin.document.run"
    }
    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.0.closing.add_permits(1);
        self.0
            .release
            .acquire()
            .await
            .map_err(|error| CapabilityEffectError::new(error.to_string()))?
            .forget();
        if self.0.fail {
            return Err(CapabilityEffectError::new(
                "native resource close was not acknowledged",
            ));
        }
        tokio::fs::remove_file(&self.0.path)
            .await
            .map_err(|error| CapabilityEffectError::new(error.to_string()))
    }
}

struct ResourceTool(Arc<Resource>);
#[async_trait]
impl Tool for ResourceTool {
    fn name(&self) -> &str {
        "projected"
    }
    fn description(&self) -> &str {
        "Own a real resource through the current Run"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    async fn execute(
        &self,
        _: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        ctx.workspace_operation(crate::capability::WorkspaceOperation::Write)?
            .check()?;
        ctx.register_run_capability_effect(RunEffect(self.0.clone()))?;
        tokio::fs::write(&self.0.path, "live resource").await?;
        *self.0.context.lock().unwrap() = Some(ctx.clone());
        Ok(ToolOutput::success("resource acquired"))
    }
}

struct Model {
    calls: AtomicUsize,
    resource: Arc<Resource>,
    response: Option<&'static str>,
}
#[async_trait]
impl LlmClient for Model {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if !tools.iter().any(|tool| tool.name == "projected") {
            return Ok(CutoverClient::final_text("auxiliary"));
        }
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(CutoverClient::tool_call("resource"));
        }
        anyhow::ensure!(
            self.resource.path.exists(),
            "Run resource was retired at the shorter Turn boundary"
        );
        let context = self
            .resource
            .context
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("resource invocation missing"))?;
        anyhow::ensure!(
            context
                .workspace_operation(crate::capability::WorkspaceOperation::Write)
                .is_err(),
            "actual invoking Turn still authorizes operations after it closed"
        );
        anyhow::ensure!(
            context
                .register_run_capability_effect(RunEffect(self.resource.clone()))
                .is_err(),
            "a stale Turn promoted new Run-owned work"
        );
        self.response
            .map(CutoverClient::final_text)
            .ok_or_else(|| anyhow::anyhow!("primary inference failed"))
    }
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.complete(messages, system, tools).await?;
        let (tx, rx) = mpsc::channel(1);
        tx.send(StreamEvent::Done(response)).await?;
        Ok(rx)
    }
}

async fn setup(
    label: &str,
    fail: bool,
    ready: bool,
) -> (Arc<AgentSession>, Arc<Resource>, Arc<AtomicUsize>) {
    setup_with_response(label, fail, ready, Some("model completed")).await
}

async fn setup_with_response(
    label: &str,
    fail: bool,
    ready: bool,
    response: Option<&'static str>,
) -> (Arc<AgentSession>, Arc<Resource>, Arc<AtomicUsize>) {
    let name = format!("run-resource-{label}-{}", uuid::Uuid::new_v4());
    let base = PathBuf::from("/tmp").join(&name);
    let workspace = base.join("workspace");
    // The Run owns this temporary host lease; it is not a product file in
    // the workspace whose mutation would require separate verification.
    let path = base.join("owned-document-lease");
    std::fs::create_dir_all(&workspace).unwrap();
    let resource = Arc::new(Resource {
        path: path.clone(),
        closing: Semaphore::new(0),
        release: Semaphore::new(usize::from(ready)),
        fail,
        context: Mutex::new(None),
    });
    let client = Arc::new(Model {
        calls: AtomicUsize::new(0),
        resource: resource.clone(),
        response,
    });
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    let options = SessionOptions::new()
        .with_session_store(Arc::new(crate::store::MemorySessionStore::new()))
        .with_session_id(&name)
        .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
        .with_planning_mode(crate::prompts::PlanningMode::Disabled)
        .with_harness(
            crate::HarnessComposeOptions::compose(
                ["system", "tools", "budget", "compact", "infer"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                None,
                None,
                vec![],
            )
            .unwrap(),
        );
    let session = Arc::new(
        agent
            .build_session(workspace.to_string_lossy().into_owned(), client, &options)
            .unwrap(),
    );
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
            CapabilityValue::Tool(Arc::new(ResourceTool(resource.clone()))),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    (session, resource, dropped)
}

async fn closing(resource: &Resource) {
    tokio::time::timeout(Duration::from_secs(3), resource.closing.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

async fn assert_not_completed(session: &AgentSession) {
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert!(
        !runs[0].status.is_terminal(),
        "Run published a terminal outcome while its native resource was still live: {:?}",
        runs[0].status
    );
}

async fn cleanup(session: &AgentSession, resource: &Resource) {
    session.close().await;
    let _ = std::fs::remove_dir_all(resource.path.parent().unwrap());
}

#[tokio::test]
async fn blocking_send_does_not_publish_success_until_run_effect_is_closed() {
    let (session, resource, dropped) = setup("blocking", false, false).await;
    let send = {
        let session = session.clone();
        tokio::spawn(async move { session.send("own resource", None).await })
    };
    closing(&resource).await;
    assert!(resource.path.exists());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_not_completed(&session).await;
    assert!(!send.is_finished());
    resource.release.add_permits(1);
    let result = tokio::time::timeout(Duration::from_secs(3), send)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.text, "model completed");
    assert!(!resource.path.exists());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        session.runs().await[0].status,
        crate::run::RunStatus::Completed
    );
    cleanup(&session, &resource).await;
}

#[tokio::test]
async fn streaming_end_waits_for_run_resource_and_retained_generation_close() {
    let (session, resource, dropped) = setup("streaming", false, false).await;
    let (mut events, worker) = session.stream("own resource", None).await.unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collect = {
        let seen = seen.clone();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                seen.lock().unwrap().push(event);
            }
        })
    };
    closing(&resource).await;
    assert_not_completed(&session).await;
    assert!(!seen
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(event, crate::AgentEvent::End { .. })));
    assert!(
        !collect.is_finished(),
        "host channel closed before resource acknowledgement"
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    resource.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    collect.await.unwrap();
    assert!(!resource.path.exists());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, crate::AgentEvent::End { .. }))
            .count(),
        1
    );
    cleanup(&session, &resource).await;
}

#[tokio::test]
async fn detached_run_remains_active_until_effect_cleanup_acknowledges() {
    let (session, resource, dropped) = setup("detached", false, false).await;
    let spawned = session
        .spawn_run_with_id("resource-detached", "own resource")
        .await
        .unwrap();
    let worker = match spawned {
        AgentRunSpawn::Started { worker, snapshot } => {
            assert!(
                snapshot.capability_binding.is_some(),
                "detached admission must bind the frozen generation before returning"
            );
            worker
        }
        _ => panic!("new Run was replayed"),
    };
    closing(&resource).await;
    assert_not_completed(&session).await;
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    resource.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(!resource.path.exists());
    assert_eq!(
        session
            .run_snapshot("resource-detached")
            .await
            .unwrap()
            .status,
        crate::run::RunStatus::Completed
    );
    let facts = crate::fact_control::read_workspace_facts(
        &session.workspace,
        &crate::fact_control::thread_for_session(session.id()),
    )
    .unwrap();
    let prepared = facts
        .iter()
        .find(|fact| fact.kind == "tool.prepared")
        .unwrap();
    let prepared: a3s_effect::ToolPreparedV1 =
        serde_json::from_value(prepared.payload["prepared"].clone()).unwrap();
    assert_eq!(
        prepared.identity.admission.unwrap().run_id,
        "resource-detached"
    );
    cleanup(&session, &resource).await;
}

#[tokio::test]
async fn failed_cleanup_refuses_success_on_send_stream_and_detached_run() {
    for mode in ["send", "stream", "detached"] {
        let (session, resource, dropped) = setup(mode, true, true).await;
        let public_worker = match mode {
            "send" => {
                let error = session.send("own resource", None).await.unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("Capability Run close was incomplete"),
                    "{error}"
                );
                None
            }
            "stream" => {
                let (mut events, worker) = session.stream("own resource", None).await.unwrap();
                let mut failures = 0;
                tokio::time::timeout(Duration::from_secs(5), async {
                    while let Some(event) = events.recv().await {
                        match event {
                            crate::AgentEvent::End { .. } => panic!("failed cleanup emitted End"),
                            crate::AgentEvent::Error { message } => {
                                assert!(
                                    message.contains("Capability Run close was incomplete"),
                                    "{message}"
                                );
                                failures += 1;
                            }
                            _ => {}
                        }
                    }
                })
                .await
                .expect("failed execution must publish Error and close its event lane");
                assert_eq!(failures, 1);
                Some(worker)
            }
            "detached" => {
                let spawned = session
                    .spawn_run_with_id("resource-failed", "own resource")
                    .await
                    .unwrap();
                match spawned {
                    AgentRunSpawn::Started { worker, .. } => Some(worker),
                    _ => panic!("new Run was replayed"),
                }
            }
            _ => unreachable!(),
        };
        let original = session.run_admission.current_original().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while original.observed_worker_join().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the actual execution must return its cleanup failure");
        assert_eq!(
            original.observed_worker_join(),
            Some(super::super::run_admission::ActualJoinObservation::Completed)
        );
        // The public observer cannot finish while its guardian retains the
        // original TaskLease for failed close proof. Error/Failed is not release.
        if let Some(mut worker) = public_worker {
            assert!(matches!(
                futures::poll!(&mut worker),
                std::task::Poll::Pending
            ));
        }
        assert!(matches!(
            session.send("must remain busy", None).await,
            Err(crate::error::CodeError::SessionBusy { .. })
        ));
        let health = session.task_scheduler.health().await.unwrap();
        assert_eq!((health.active, health.released), (1, 0));
        assert_eq!(
            session.runs().await[0].status,
            crate::run::RunStatus::Failed
        );
        assert!(
            resource.path.exists(),
            "test did not exercise an unacknowledged resource"
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        cleanup(&session, &resource).await;
        assert!(Arc::ptr_eq(
            &original,
            &session.run_admission.current_original().unwrap()
        ));
        let health = session.task_scheduler.health().await.unwrap();
        assert_eq!((health.active, health.released), (1, 0));
    }
}

#[tokio::test]
async fn timed_out_cleanup_refuses_completion_and_preserves_resource_failure() {
    let (session, resource, _) = setup("timeout", false, false).await;
    let error = tokio::time::timeout(Duration::from_secs(8), session.send("own resource", None))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Capability Run close was incomplete"),
        "{error}"
    );
    assert_eq!(
        session.runs().await[0].status,
        crate::run::RunStatus::Failed
    );
    assert!(resource.path.exists());
    cleanup(&session, &resource).await;
}

#[tokio::test]
async fn cancellation_during_teardown_cannot_publish_the_earlier_model_success() {
    for stream in [false, true] {
        let (session, resource, _) = setup_with_response(
            if stream {
                "cancel-stream"
            } else {
                "cancel-send"
            },
            false,
            false,
            Some("model completed without being interrupted"),
        )
        .await;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let send = if stream {
            let (mut events, worker) = session.stream("own resource", None).await.unwrap();
            let observed = observed.clone();
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    observed.lock().unwrap().push(event);
                }
                worker.await.unwrap();
                None
            })
        } else {
            let session = session.clone();
            tokio::spawn(async move { Some(session.send("own resource", None).await) })
        };
        closing(&resource).await;
        assert!(
            session.cancel().await,
            "Run lost its active cancellation slot before cleanup"
        );
        resource.release.add_permits(1);
        let result = tokio::time::timeout(Duration::from_secs(3), send)
            .await
            .unwrap()
            .unwrap();
        if let Some(result) = result {
            assert!(result.is_err(), "cancelled teardown returned model success");
        }
        {
            let events = observed.lock().unwrap();
            assert!(!events
                .iter()
                .any(|event| matches!(event, crate::AgentEvent::End { .. })));
            if stream {
                assert!(events
                    .iter()
                    .any(|event| matches!(event, crate::AgentEvent::Error { .. })));
            }
        }
        assert_eq!(
            session.runs().await[0].status,
            crate::run::RunStatus::Cancelled
        );
        assert!(!resource.path.exists());
        cleanup(&session, &resource).await;
    }
}

#[tokio::test]
async fn plain_host_context_cannot_promote_effects_to_an_unowned_run() {
    let path = PathBuf::from("/tmp/a3s-unowned-run-resource");
    let resource = Arc::new(Resource {
        path: path.clone(),
        closing: Semaphore::new(0),
        release: Semaphore::new(1),
        fail: false,
        context: Mutex::new(None),
    });
    let context = ToolContext::new(path);
    assert_eq!(
        context.register_run_capability_effect(RunEffect(resource)),
        Err(crate::capability::CapabilityScopeError::AgentTurnScopeUnavailable)
    );
}

#[tokio::test]
async fn primary_inference_failure_is_preserved_when_resource_teardown_also_fails() {
    let (session, resource, _) = setup_with_response("double-failure", true, true, None).await;
    let error = session.send("own resource", None).await.unwrap_err();
    let message = error.to_string();
    assert!(message.contains("primary inference failed"), "{message}");
    assert!(
        message.contains("Capability Run close was incomplete"),
        "{message}"
    );
    assert_eq!(
        session.runs().await[0].status,
        crate::run::RunStatus::Failed
    );
    assert!(resource.path.exists());
    cleanup(&session, &resource).await;
}

async fn seed_resume(session: &AgentSession, checkpoint: bool) {
    if checkpoint {
        let saved = crate::loop_checkpoint::LoopCheckpoint {
            schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
            run_id: "resource-checkpoint".into(),
            session_id: session.id().into(),
            capability_binding: None,
            standing_context_digest: None,
            durable_source: None,
            turn: 1,
            messages: vec![Message::user("own resource")],
            total_usage: TokenUsage::default(),
            tool_calls_count: 3,
            verification_reports: vec![],
            convergence: Default::default(),
            checkpoint_ms: 1,
        };
        session
            .session_store
            .as_ref()
            .unwrap()
            .save_loop_checkpoint("resource-checkpoint", &saved)
            .await
            .unwrap();
    } else {
        use a3s_effect::LogStore;
        a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
            &session.workspace,
            session.id(),
        ))
        .unwrap()
        .append(
            &crate::fact_control::thread_for_session(session.id()),
            &[a3s_effect::message_fact("resume-resource", "own resource")],
            None,
        )
        .unwrap();
    }
}

#[tokio::test]
async fn both_resume_paths_refuse_success_after_unacknowledged_resource_cleanup() {
    for checkpoint in [false, true] {
        let (session, resource, dropped) = setup("resume-failure", true, true).await;
        seed_resume(&session, checkpoint).await;
        let error = session.resume_run("resource-checkpoint").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Capability Run close was incomplete"),
            "{error}"
        );
        assert!(resource.path.exists());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        if checkpoint {
            assert_eq!(
                session.runs().await[0].status,
                crate::run::RunStatus::Failed
            );
        }
        cleanup(&session, &resource).await;
    }
}

#[tokio::test]
async fn resumed_inference_failure_still_awaits_resource_cleanup() {
    let (session, resource, dropped) =
        setup_with_response("resume-model-failure", false, false, None).await;
    seed_resume(&session, true).await;
    let send = {
        let session = session.clone();
        tokio::spawn(async move { session.resume_run("resource-checkpoint").await })
    };
    closing(&resource).await;
    assert!(
        !send.is_finished(),
        "resume returned its primary error before resource cleanup acknowledged"
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(resource.path.exists());
    resource.release.add_permits(1);
    let error = tokio::time::timeout(Duration::from_secs(3), send)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error.to_string().contains("primary inference failed"),
        "{error}"
    );
    assert!(!resource.path.exists());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    cleanup(&session, &resource).await;
}

#[tokio::test]
async fn checkpoint_resume_retains_live_run_and_usage_until_teardown_acknowledges() {
    let (session, resource, dropped) = setup("resume-blocking", false, false).await;
    seed_resume(&session, true).await;
    let send = {
        let session = session.clone();
        tokio::spawn(async move { session.resume_run("resource-checkpoint").await })
    };
    closing(&resource).await;
    assert_not_completed(&session).await;
    assert!(!send.is_finished());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    resource.release.add_permits(1);
    let result = tokio::time::timeout(Duration::from_secs(3), send)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.text, "model completed");
    assert!(
        result.tool_calls_count >= 3,
        "resume lost seeded cumulative usage"
    );
    assert!(!resource.path.exists());
    assert_eq!(
        session.runs().await[0].status,
        crate::run::RunStatus::Completed
    );
    cleanup(&session, &resource).await;
}
