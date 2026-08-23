use super::*;

struct VersionedCommand {
    version: &'static str,
    executions: Arc<Mutex<Vec<&'static str>>>,
    entered: Option<Arc<Semaphore>>,
    release: Option<Arc<(Mutex<bool>, std::sync::Condvar)>>,
}

impl crate::commands::SlashCommand for VersionedCommand {
    fn name(&self) -> &str {
        "projected-command"
    }

    fn description(&self) -> &str {
        self.version
    }

    fn execute(
        &self,
        _args: &str,
        _ctx: &crate::commands::CommandContext,
    ) -> crate::commands::CommandOutput {
        self.executions.lock().unwrap().push(self.version);
        if let Some(entered) = &self.entered {
            entered.add_permits(1);
        }
        if let Some(release) = &self.release {
            let (released, wake) = release.as_ref();
            let mut released = released.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
        }
        crate::commands::CommandOutput::text(self.version)
    }
}

fn command(
    version: &'static str,
    executions: &Arc<Mutex<Vec<&'static str>>>,
) -> Arc<dyn crate::commands::SlashCommand> {
    Arc::new(VersionedCommand {
        version,
        executions: Arc::clone(executions),
        entered: None,
        release: None,
    })
}

#[tokio::test]
async fn command_name_conflicts_fail_before_and_after_publication() {
    let compatibility_session = test_session("capability-command-name-conflict").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    compatibility_session
        .register_command(command("compatibility", &executions))
        .unwrap();
    let before = compatibility_session.capability_catalog_stamp();
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Command,
        &[("projected-command", 'b')],
    );
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-command"].clone(),
            CapabilityValue::Command(command("projected", &executions)),
        )
        .unwrap();
    assert!(matches!(
        compatibility_session
            .apply_capability_batch(batch, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Command,
            ..
        })
    ));
    assert_eq!(compatibility_session.capability_catalog_stamp(), before);

    let projected_session = test_session("capability-command-post-publication-conflict").await;
    let upstream = use_generation(1, 'c');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Command,
        &[("projected-command", 'd')],
    );
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-command"].clone(),
            CapabilityValue::Command(command("published", &executions)),
        )
        .unwrap();
    projected_session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let stamp = projected_session.capability_catalog_stamp();
    assert!(matches!(
        projected_session.register_command(command("compatibility", &executions)),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Command,
                ..
            }
        ))
    ));

    // The legacy registry guard remains readable for SDK compatibility. Its
    // direct mutation path must also refuse a published projected name.
    projected_session
        .command_registry()
        .register(command("legacy-guard", &executions));
    assert!(!projected_session
        .command_registry()
        .list_full()
        .iter()
        .any(|(name, _, _)| name == "projected-command"));
    assert_eq!(projected_session.capability_catalog_stamp(), stamp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn command_dispatch_keeps_n_registry_and_use_lease_across_cutover() {
    let session = Arc::new(test_session("capability-command-cutover").await);
    let executions = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_kind_set(
        1,
        first_upstream.clone(),
        CapabilityKind::Command,
        &[("projected-command", 'b')],
    );
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["projected-command"].clone(),
            CapabilityValue::Command(Arc::new(VersionedCommand {
                version: "generation-one",
                executions: Arc::clone(&executions),
                entered: Some(Arc::clone(&entered)),
                release: Some(Arc::clone(&release)),
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("/projected-command", None).await }
    });
    entered.acquire().await.unwrap().forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) = use_kind_set(
        2,
        second_upstream.clone(),
        CapabilityKind::Command,
        &[("projected-command", 'd')],
    );
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage_value(
            second_ids["projected-command"].clone(),
            CapabilityValue::Command(command("generation-two", &executions)),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);

    {
        let (released, wake) = release.as_ref();
        *released.lock().unwrap() = true;
        wake.notify_all();
    }
    let old_result = old_run.await.unwrap().unwrap();
    assert_eq!(old_result.text, "generation-one");
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let (mut events, worker) = session.stream("/projected-command", None).await.unwrap();
    let mut streamed = String::new();
    while let Some(event) = events.recv().await {
        if let crate::agent::AgentEvent::TextDelta { text } = event {
            streamed.push_str(&text);
        }
    }
    worker.await.unwrap();
    assert_eq!(streamed, "generation-two");
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*executions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
}
