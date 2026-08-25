use super::*;

#[derive(Clone)]
struct HandlerGate {
    state: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl HandlerGate {
    fn new() -> Self {
        Self {
            state: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        }
    }

    fn wait(&self) {
        let (released, wake) = self.state.as_ref();
        let mut released = released.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
    }

    fn release(&self) {
        let (released, wake) = self.state.as_ref();
        *released.lock().unwrap() = true;
        wake.notify_all();
    }
}

struct ReleaseOnDrop(HandlerGate);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct VersionedHookHandler {
    version: &'static str,
    executions: Arc<Mutex<Vec<&'static str>>>,
    entered: Option<Arc<Semaphore>>,
    gate: Option<HandlerGate>,
    block: bool,
}

struct MutationGateCheckingDropHandler {
    session: std::sync::Weak<AgentSession>,
    gate_was_available: Arc<std::sync::atomic::AtomicBool>,
}

impl crate::hooks::HookHandler for MutationGateCheckingDropHandler {
    fn handle(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResponse {
        crate::hooks::HookResponse::continue_()
    }
}

impl Drop for MutationGateCheckingDropHandler {
    fn drop(&mut self) {
        let available = self.session.upgrade().is_some_and(|session| {
            session
                .close_handle
                .immediate_extension_mutation
                .try_lock()
                .is_ok()
        });
        self.gate_was_available
            .store(available, std::sync::atomic::Ordering::SeqCst);
    }
}

impl crate::hooks::HookHandler for VersionedHookHandler {
    fn handle(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResponse {
        self.executions.lock().unwrap().push(self.version);
        if let Some(entered) = &self.entered {
            entered.add_permits(1);
        }
        if let Some(gate) = &self.gate {
            gate.wait();
        }
        if self.block {
            crate::hooks::HookResponse::block(self.version)
        } else {
            crate::hooks::HookResponse::continue_()
        }
    }
}

fn hook_binding(
    id: &str,
    event_type: crate::hooks::HookEventType,
    version: &'static str,
    executions: &Arc<Mutex<Vec<&'static str>>>,
) -> Arc<crate::hooks::HookBinding> {
    Arc::new(crate::hooks::HookBinding::new(
        crate::hooks::Hook::new(id, event_type),
        Arc::new(VersionedHookHandler {
            version,
            executions: Arc::clone(executions),
            entered: None,
            gate: None,
            block: true,
        }),
    ))
}

#[derive(Clone)]
struct FinalTextClient;

#[async_trait]
impl LlmClient for FinalTextClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(CutoverClient::final_text("hook observation complete"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the Hook projection tests")
    }
}

#[derive(Debug, Default)]
struct RecordingExternalHookExecutor {
    events: Mutex<Vec<crate::hooks::HookEventType>>,
}

#[async_trait]
impl crate::hooks::HookExecutor for RecordingExternalHookExecutor {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        self.events.lock().unwrap().push(event.event_type());
        crate::hooks::HookResult::continue_()
    }
}

#[derive(Debug)]
struct SkippingExternalHookExecutor;

#[async_trait]
impl crate::hooks::HookExecutor for SkippingExternalHookExecutor {
    async fn fire(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::skip()
    }
}

async fn test_session_with_external_hook(
    name: &str,
    hook: Arc<dyn crate::hooks::HookExecutor>,
) -> AgentSession {
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    agent
        .build_session(
            format!("/tmp/{name}"),
            Arc::new(NoopClient),
            &SessionOptions::new()
                .with_session_id(name)
                .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_hook_executor(hook),
        )
        .unwrap()
}

#[tokio::test]
async fn atomic_registration_drops_replaced_sdk_handler_outside_the_session_gate() {
    let session = Arc::new(test_session("capability-hook-drop-boundary").await);
    let gate_was_available = Arc::new(std::sync::atomic::AtomicBool::new(false));
    session
        .register_hook_registration(
            crate::hooks::Hook::new("drop-boundary", crate::hooks::HookEventType::PrePrompt),
            Some(Arc::new(MutationGateCheckingDropHandler {
                session: Arc::downgrade(&session),
                gate_was_available: Arc::clone(&gate_was_available),
            })),
        )
        .unwrap();

    session
        .register_hook_registration(
            crate::hooks::Hook::new("drop-boundary", crate::hooks::HookEventType::PrePrompt),
            None,
        )
        .unwrap();

    assert!(gate_was_available.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn hook_name_conflicts_fail_before_and_after_publication() {
    let compatibility_session = test_session("capability-hook-name-conflict").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    compatibility_session
        .register_hook(crate::hooks::Hook::new(
            "projected-hook",
            crate::hooks::HookEventType::PrePrompt,
        ))
        .unwrap();
    let before = compatibility_session.capability_catalog_stamp();
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-hook", 'b')],
    );
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-hook"].clone(),
            CapabilityValue::Hook(hook_binding(
                "projected-hook",
                crate::hooks::HookEventType::PrePrompt,
                "projected",
                &executions,
            )),
        )
        .unwrap();
    assert!(matches!(
        compatibility_session
            .apply_capability_batch(batch, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Hook,
            ..
        })
    ));
    assert_eq!(compatibility_session.capability_catalog_stamp(), before);

    let projected_session = test_session("capability-hook-post-publication-conflict").await;
    let upstream = use_generation(1, 'c');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-hook", 'd')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-hook"].clone(),
            CapabilityValue::Hook(hook_binding(
                "projected-hook",
                crate::hooks::HookEventType::PrePrompt,
                "published",
                &executions,
            )),
        )
        .unwrap();
    projected_session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let stamp = projected_session.capability_catalog_stamp();

    assert!(matches!(
        projected_session.register_hook(crate::hooks::Hook::new(
            "projected-hook",
            crate::hooks::HookEventType::PrePrompt,
        )),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Hook,
                ..
            }
        ))
    ));
    assert!(matches!(
        projected_session.register_hook_handler(
            "projected-hook",
            Arc::new(VersionedHookHandler {
                version: "compatibility",
                executions: Arc::clone(&executions),
                entered: None,
                gate: None,
                block: true,
            }),
        ),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Hook,
                ..
            }
        ))
    ));
    assert_eq!(projected_session.capability_catalog_stamp(), stamp);
}

#[tokio::test]
async fn projected_hook_rejects_events_outside_the_run_scope_before_publication() {
    for (index, event_type) in [
        crate::hooks::HookEventType::SessionStart,
        crate::hooks::HookEventType::SessionEnd,
        crate::hooks::HookEventType::SkillLoad,
        crate::hooks::HookEventType::SkillUnload,
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("capability-hook-invalid-scope-{index}");
        let session = test_session(&name).await;
        let before = session.capability_catalog_stamp();
        let executions = Arc::new(Mutex::new(Vec::new()));
        let upstream = use_generation(1, ['a', 'b', 'c', 'd'][index]);
        let (set, ids) = use_kind_set(
            1,
            upstream.clone(),
            CapabilityKind::Hook,
            &[("projected-hook", ['e', 'a', 'b', 'c'][index])],
        );
        let acquired = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut batch = SessionCapabilityBatch::from_use_projection(
            set,
            provider(upstream, &acquired, &dropped),
        )
        .unwrap();
        batch
            .stage_value(
                ids["projected-hook"].clone(),
                CapabilityValue::Hook(hook_binding(
                    "projected-hook",
                    event_type,
                    "invalid-scope",
                    &executions,
                )),
            )
            .unwrap();

        assert!(matches!(
            session
                .apply_capability_batch(batch, CancellationToken::new())
                .await,
            Err(CapabilityRuntimeError::RuntimeValueInvalid {
                kind: CapabilityKind::Hook,
                ..
            })
        ));
        assert_eq!(session.capability_catalog_stamp(), before);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn active_run_keeps_n_hook_handler_and_use_lease_across_cutover() {
    let session = Arc::new(test_session("capability-hook-cutover").await);
    let executions = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Semaphore::new(0));
    let gate = HandlerGate::new();
    let _release_on_drop = ReleaseOnDrop(gate.clone());
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_kind_set(
        1,
        first_upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-hook", 'b')],
    );
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["projected-hook"].clone(),
            CapabilityValue::Hook(Arc::new(crate::hooks::HookBinding::new(
                crate::hooks::Hook::new("projected-hook", crate::hooks::HookEventType::PrePrompt),
                Arc::new(VersionedHookHandler {
                    version: "generation-one",
                    executions: Arc::clone(&executions),
                    entered: Some(Arc::clone(&entered)),
                    gate: Some(gate.clone()),
                    block: true,
                }),
            ))),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("execute generation one", None).await }
    });
    entered.acquire().await.unwrap().forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) = use_kind_set(
        2,
        second_upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-hook", 'd')],
    );
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage_value(
            second_ids["projected-hook"].clone(),
            CapabilityValue::Hook(hook_binding(
                "projected-hook",
                crate::hooks::HookEventType::PrePrompt,
                "generation-two",
                &executions,
            )),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);

    gate.release();
    let old_error = old_run.await.unwrap().unwrap_err();
    assert!(old_error.to_string().contains("generation-one"));
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new_error = session
        .send("execute generation two", None)
        .await
        .unwrap_err();
    assert!(new_error.to_string().contains("generation-two"));
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*executions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn observational_hook_is_supervised_until_handler_and_use_lease_settle() {
    let session = Arc::new(
        test_session_with_client(
            "capability-hook-observer",
            Arc::new(FinalTextClient) as Arc<dyn LlmClient>,
        )
        .await,
    );
    let executions = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Semaphore::new(0));
    let gate = HandlerGate::new();
    let _release_on_drop = ReleaseOnDrop(gate.clone());
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-observer", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-observer"].clone(),
            CapabilityValue::Hook(Arc::new(crate::hooks::HookBinding::new(
                crate::hooks::Hook::new(
                    "projected-observer",
                    crate::hooks::HookEventType::PostResponse,
                ),
                Arc::new(VersionedHookHandler {
                    version: "observed",
                    executions: Arc::clone(&executions),
                    entered: Some(Arc::clone(&entered)),
                    gate: Some(gate.clone()),
                    block: false,
                }),
            ))),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("complete normally", None).await }
    });
    entered.acquire().await.unwrap().forget();
    tokio::task::yield_now().await;
    assert!(!run.is_finished());
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    gate.release();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.text, "hook observation complete");
    assert_eq!(&*executions.lock().unwrap(), &["observed"]);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn async_hook_config_registers_detached_handler_with_the_run_supervisor() {
    let session = Arc::new(
        test_session_with_client(
            "capability-hook-async-handler",
            Arc::new(FinalTextClient) as Arc<dyn LlmClient>,
        )
        .await,
    );
    let executions = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Semaphore::new(0));
    let gate = HandlerGate::new();
    let _release_on_drop = ReleaseOnDrop(gate.clone());
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-async-observer", 'b')],
    );
    let hook = crate::hooks::Hook::new(
        "projected-async-observer",
        crate::hooks::HookEventType::GenerateEnd,
    )
    .with_config(crate::hooks::HookConfig {
        async_execution: true,
        ..Default::default()
    });
    let binding = Arc::new(crate::hooks::HookBinding::new(
        hook,
        Arc::new(VersionedHookHandler {
            version: "async-observed",
            executions: Arc::clone(&executions),
            entered: Some(Arc::clone(&entered)),
            gate: Some(gate.clone()),
            block: false,
        }),
    ));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-async-observer"].clone(),
            CapabilityValue::Hook(binding),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("complete with async observation", None).await }
    });
    entered.acquire().await.unwrap().forget();
    tokio::task::yield_now().await;
    assert!(!run.is_finished());
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    gate.release();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.text, "hook observation complete");
    assert_eq!(&*executions.lock().unwrap(), &["async-observed"]);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn timed_out_handler_settles_under_the_run_supervisor_before_use_lease_release() {
    let session = Arc::new(test_session("capability-hook-timeout-settle").await);
    let executions = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Semaphore::new(0));
    let gate = HandlerGate::new();
    let _release_on_drop = ReleaseOnDrop(gate.clone());
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-timeout", 'b')],
    );
    let hook = crate::hooks::Hook::new("projected-timeout", crate::hooks::HookEventType::PrePrompt)
        .with_config(crate::hooks::HookConfig {
            timeout_ms: 5,
            ..Default::default()
        });
    let binding = Arc::new(crate::hooks::HookBinding::new(
        hook,
        Arc::new(VersionedHookHandler {
            version: "timed-out",
            executions: Arc::clone(&executions),
            entered: Some(Arc::clone(&entered)),
            gate: Some(gate.clone()),
            block: true,
        }),
    ));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-timeout"].clone(),
            CapabilityValue::Hook(binding),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("trigger timeout", None).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .expect("timed-out hook handler did not start within the test bound")
        .unwrap()
        .forget();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!run.is_finished());
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    gate.release();
    let error = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("timed-out hook run did not settle after releasing the handler")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert_eq!(&*executions.lock().unwrap(), &["timed-out"]);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn timed_out_observational_handler_settles_inside_its_supervised_task() {
    let session = Arc::new(
        test_session_with_client(
            "capability-hook-observer-timeout-settle",
            Arc::new(FinalTextClient) as Arc<dyn LlmClient>,
        )
        .await,
    );
    let executions = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Semaphore::new(0));
    let gate = HandlerGate::new();
    let _release_on_drop = ReleaseOnDrop(gate.clone());
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-observer-timeout", 'b')],
    );
    let hook = crate::hooks::Hook::new(
        "projected-observer-timeout",
        crate::hooks::HookEventType::PostResponse,
    )
    .with_config(crate::hooks::HookConfig {
        timeout_ms: 5,
        ..Default::default()
    });
    let binding = Arc::new(crate::hooks::HookBinding::new(
        hook,
        Arc::new(VersionedHookHandler {
            version: "timed-out-observer",
            executions: Arc::clone(&executions),
            entered: Some(Arc::clone(&entered)),
            gate: Some(gate.clone()),
            block: false,
        }),
    ));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-observer-timeout"].clone(),
            CapabilityValue::Hook(binding),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let run = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            session
                .send("complete before observation settles", None)
                .await
        }
    });
    entered.acquire().await.unwrap().forget();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!run.is_finished());
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    gate.release();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.text, "hook observation complete");
    assert_eq!(&*executions.lock().unwrap(), &["timed-out-observer"]);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn projected_hook_composes_with_a_session_static_external_executor() {
    let external = Arc::new(RecordingExternalHookExecutor::default());
    let session = test_session_with_external_hook(
        "capability-hook-external-executor",
        Arc::clone(&external) as Arc<dyn crate::hooks::HookExecutor>,
    )
    .await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-hook", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-hook"].clone(),
            CapabilityValue::Hook(hook_binding(
                "projected-hook",
                crate::hooks::HookEventType::PrePrompt,
                "projected-policy",
                &executions,
            )),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let error = session.send("must be checked", None).await.unwrap_err();
    assert!(error.to_string().contains("projected-policy"));
    assert!(external
        .events
        .lock()
        .unwrap()
        .contains(&crate::hooks::HookEventType::PrePrompt));
    assert_eq!(&*executions.lock().unwrap(), &["projected-policy"]);
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn external_skip_cannot_bypass_projected_hook_policy() {
    let session = test_session_with_external_hook(
        "capability-hook-external-skip",
        Arc::new(SkippingExternalHookExecutor) as Arc<dyn crate::hooks::HookExecutor>,
    )
    .await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Hook,
        &[("projected-hook", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-hook"].clone(),
            CapabilityValue::Hook(hook_binding(
                "projected-hook",
                crate::hooks::HookEventType::PrePrompt,
                "projected-policy-after-skip",
                &executions,
            )),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let error = session
        .send("must still be checked", None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("projected-policy-after-skip"));
    assert_eq!(
        &*executions.lock().unwrap(),
        &["projected-policy-after-skip"]
    );
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
