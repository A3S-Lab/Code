use super::*;

impl WorkspaceRuntime {
    pub(crate) async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.lifetime.cancel();
        let (runtimes, starts) = {
            let _status_update = self.status_updates.lock().await;
            let mut runtimes = Vec::new();
            let mut starts = Vec::new();
            for slot in &self.slots {
                let mut state = slot.state.lock().await;
                match std::mem::replace(&mut *state, SlotState::Dormant) {
                    SlotState::Ready(runtime) => runtimes.push(runtime),
                    SlotState::Starting(start) => starts.push(start),
                    SlotState::Failed(failure) => {
                        if let Some(runtime) = failure.retained_runtime {
                            runtimes.push(runtime);
                        }
                    }
                    SlotState::Dormant => {}
                }
            }
            self.status.send_replace(CodeIntelligenceStatus {
                state: CodeIntelligenceState::Unavailable,
                message: Some("Code Intelligence runtime is shut down".to_owned()),
                ..CodeIntelligenceStatus::default()
            });
            (runtimes, starts)
        };

        self.stop_generations(&runtimes, &starts, "workspace shutdown")
            .await;
    }

    pub(super) async fn stop_generations(
        &self,
        runtimes: &[Arc<LanguageRuntime>],
        starts: &[RuntimeStart],
        operation: &'static str,
    ) {
        for start in starts {
            start.cancellation.cancel();
        }
        let cleanup = async {
            let runtime_cleanup = async {
                let mut shutdowns = FuturesUnordered::new();
                for runtime in runtimes {
                    shutdowns.push(runtime.shutdown());
                }
                let mut complete = true;
                while let Some(result) = shutdowns.next().await {
                    if let Err(error) = result {
                        complete = false;
                        tracing::warn!(
                            error = %error,
                            operation,
                            "Code Intelligence runtime cleanup failed"
                        );
                    }
                }
                complete
            };
            let start_cleanup = async {
                let mut completions = FuturesUnordered::new();
                for start in starts {
                    completions.push(Self::wait_for_start_completion(start.outcome.clone()));
                }
                while completions.next().await.is_some() {}
            };
            let (runtime_complete, ()) = tokio::join!(runtime_cleanup, start_cleanup);
            runtime_complete
        };
        let graceful = tokio::time::timeout(self.timeout.min(SHUTDOWN_GRACE), cleanup).await;
        if !matches!(graceful, Ok(true)) {
            tracing::warn!(
                timeout = ?self.timeout.min(SHUTDOWN_GRACE),
                operation,
                "Code Intelligence cleanup exceeded its shutdown bound; forcing remaining processes"
            );
            for start in starts {
                start.abort.abort();
            }
            for runtime in runtimes {
                runtime.force_kill();
            }
            let settle = async {
                let runtime_settle = async {
                    let mut shutdowns = FuturesUnordered::new();
                    for runtime in runtimes {
                        shutdowns.push(runtime.shutdown());
                    }
                    while shutdowns.next().await.is_some() {}
                };
                let start_settle = async {
                    let mut completions = FuturesUnordered::new();
                    for start in starts {
                        completions.push(Self::wait_for_start_completion(start.outcome.clone()));
                    }
                    while completions.next().await.is_some() {}
                };
                tokio::join!(runtime_settle, start_settle);
            };
            let _ = tokio::time::timeout(SHUTDOWN_ABORT_SETTLE, settle).await;
        }
    }

    pub(super) async fn ensure_runtime(
        &self,
        slot: &LanguageSlot,
        cancellation: &CancellationToken,
    ) -> CodeIntelligenceResult<Arc<LanguageRuntime>> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(CodeIntelligenceError::Unavailable {
                message: "the workspace runtime is shutting down".to_owned(),
            });
        }
        let state = Arc::clone(&slot.state);
        let mut state = tokio::select! {
            _ = cancellation.cancelled() => return Err(CodeIntelligenceError::Cancelled),
            state = state.lock_owned() => state,
        };
        match &*state {
            SlotState::Starting(start) => {
                let outcome = start.outcome.clone();
                drop(state);
                return Self::wait_for_runtime_start(outcome, cancellation).await;
            }
            SlotState::Ready(runtime) => {
                if runtime.unavailable_message().is_none() {
                    return Ok(Arc::clone(runtime));
                }
            }
            SlotState::Failed(failure) if failure.at.elapsed() < START_RETRY_DELAY => {
                return Err(CodeIntelligenceError::Unavailable {
                    message: failure.message.clone(),
                });
            }
            SlotState::Dormant | SlotState::Failed(_) => {}
        }
        if cancellation.is_cancelled() {
            return Err(CodeIntelligenceError::Cancelled);
        }
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(CodeIntelligenceError::Unavailable {
                message: "the workspace runtime is shutting down".to_owned(),
            });
        }

        let retiring = match std::mem::replace(&mut *state, SlotState::Dormant) {
            SlotState::Ready(runtime) => {
                let message = runtime.unavailable_message().unwrap_or_else(|| {
                    "the previous language runtime is no longer usable".to_owned()
                });
                Some((runtime, message))
            }
            SlotState::Failed(failure) => failure
                .retained_runtime
                .map(|runtime| (runtime, failure.message)),
            SlotState::Dormant => None,
            SlotState::Starting(start) => {
                let outcome = start.outcome.clone();
                *state = SlotState::Starting(start);
                drop(state);
                return Self::wait_for_runtime_start(outcome, cancellation).await;
            }
        };
        let generation = slot.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let start_cancellation = self.lifetime.child_token();
        let (outcome_tx, outcome_rx) = watch::channel(None);
        let (begin_tx, begin_rx) = oneshot::channel();
        let task = self.spawn_runtime_start(
            slot,
            generation,
            start_cancellation.clone(),
            retiring,
            begin_rx,
            outcome_tx,
        );
        *state = SlotState::Starting(RuntimeStart {
            generation,
            cancellation: start_cancellation,
            outcome: outcome_rx.clone(),
            abort: task.abort_handle(),
        });
        drop(state);
        let _ = begin_tx.send(());
        Self::wait_for_runtime_start(outcome_rx, cancellation).await
    }

    fn spawn_runtime_start(
        &self,
        slot: &LanguageSlot,
        generation: u64,
        cancellation: CancellationToken,
        retiring: Option<(Arc<LanguageRuntime>, String)>,
        begin: oneshot::Receiver<()>,
        outcome: watch::Sender<Option<RuntimeStartOutcome>>,
    ) -> tokio::task::JoinHandle<()> {
        let profile = slot.profile.clone();
        let profile_id = profile.id();
        let canonical_root = self.canonical_root.clone();
        let layout = self.layout.clone();
        let documents = Arc::clone(&slot.documents);
        let diagnostics = Arc::clone(&self.diagnostics);
        let timeout = self.timeout;
        let slot_state = Arc::clone(&slot.state);
        let status = self.status.clone();
        let status_updates = Arc::clone(&self.status_updates);
        let lifetime = self.lifetime.clone();

        tokio::spawn(async move {
            if begin.await.is_err() {
                return;
            }
            {
                let _status_update = status_updates.lock().await;
                let state = slot_state.lock().await;
                let is_current = matches!(
                    &*state,
                    SlotState::Starting(start) if start.generation == generation
                );
                if is_current && !lifetime.is_cancelled() && !cancellation.is_cancelled() {
                    publish_language_status(
                        &status,
                        profile_language(profile_id),
                        CodeIntelligenceState::Starting,
                        CodeIntelligenceCapabilities::default(),
                        Some("language runtime is starting".to_owned()),
                        "one or more language runtimes are unavailable",
                    );
                }
            }
            let attempt = AssertUnwindSafe(async {
                if let Some((runtime, message)) = retiring {
                    tracing::warn!(
                        language = %profile_language(profile_id),
                        message,
                        "Code Intelligence will restart an exited language runtime"
                    );
                    let retired = tokio::select! {
                        _ = cancellation.cancelled() => {
                            return Err(StartAttemptFailure {
                                public: CodeIntelligenceError::Unavailable {
                                    message: "language runtime start was cancelled".to_owned(),
                                },
                                message: "language runtime start was cancelled".to_owned(),
                                retained_runtime: Some(runtime),
                            });
                        }
                        result = runtime.shutdown() => result,
                    };
                    if let Err(error) = retired {
                        tracing::warn!(
                            language = %profile_language(profile_id),
                            error = %error,
                            "Code Intelligence could not fully retire an exited language runtime"
                        );
                        let message = error.to_string();
                        return Err(StartAttemptFailure {
                            public: map_language_error(profile_id, error),
                            message,
                            retained_runtime: Some(runtime),
                        });
                    }
                }
                if cancellation.is_cancelled() {
                    return Err(StartAttemptFailure {
                        public: CodeIntelligenceError::Unavailable {
                            message: "language runtime start was cancelled".to_owned(),
                        },
                        message: "language runtime start was cancelled".to_owned(),
                        retained_runtime: None,
                    });
                }
                LanguageRuntime::start(
                    profile,
                    canonical_root,
                    layout,
                    documents,
                    diagnostics,
                    cancellation.clone(),
                    timeout,
                )
                .await
                .map(Arc::new)
                .map_err(|error| {
                    let message = error.to_string();
                    StartAttemptFailure {
                        public: map_language_error(profile_id, error),
                        message,
                        retained_runtime: None,
                    }
                })
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|payload| {
                let message = panic_payload_to_string(payload);
                Err(StartAttemptFailure {
                    public: CodeIntelligenceError::Unavailable {
                        message: format!("language runtime start task panicked: {message}"),
                    },
                    message: format!("language runtime start task panicked: {message}"),
                    retained_runtime: None,
                })
            });

            let mut cleanup_runtime = None;
            let mut monitor_runtime = None;
            let public_outcome;
            {
                let _status_update = status_updates.lock().await;
                let mut state = slot_state.lock().await;
                let is_current = matches!(
                    &*state,
                    SlotState::Starting(start) if start.generation == generation
                );
                if is_current && !lifetime.is_cancelled() && !cancellation.is_cancelled() {
                    match attempt {
                        Ok(runtime) => {
                            let capabilities = runtime.capabilities();
                            *state = SlotState::Ready(Arc::clone(&runtime));
                            publish_language_status(
                                &status,
                                profile_language(profile_id),
                                CodeIntelligenceState::Ready,
                                capabilities,
                                None,
                                "one or more language runtimes are unavailable",
                            );
                            public_outcome = Ok(Arc::clone(&runtime));
                            monitor_runtime = Some(runtime);
                        }
                        Err(failure) => {
                            public_outcome = Err(failure.public.clone());
                            publish_language_status(
                                &status,
                                profile_language(profile_id),
                                CodeIntelligenceState::Unavailable,
                                CodeIntelligenceCapabilities::default(),
                                Some(failure.message.clone()),
                                "one or more language runtimes are unavailable",
                            );
                            *state = SlotState::Failed(StartFailure {
                                at: Instant::now(),
                                message: failure.message,
                                retained_runtime: failure.retained_runtime,
                            });
                        }
                    }
                } else {
                    cleanup_runtime = match attempt {
                        Ok(runtime) => Some(runtime),
                        Err(failure) => failure.retained_runtime,
                    };
                    public_outcome = Err(CodeIntelligenceError::Unavailable {
                        message: "language runtime start is no longer current".to_owned(),
                    });
                }
            }

            if let Some(runtime) = cleanup_runtime {
                if let Err(error) = runtime.shutdown().await {
                    tracing::warn!(
                        language = %profile_language(profile_id),
                        error = %error,
                        "Code Intelligence could not retire a cancelled language runtime start"
                    );
                }
            }
            if let Some(runtime) = monitor_runtime {
                Self::spawn_runtime_monitor(
                    slot_state,
                    status,
                    status_updates,
                    lifetime,
                    profile_id,
                    runtime,
                );
            }
            outcome.send_replace(Some(public_outcome));
        })
    }

    async fn wait_for_runtime_start(
        mut outcome: watch::Receiver<Option<RuntimeStartOutcome>>,
        cancellation: &CancellationToken,
    ) -> RuntimeStartOutcome {
        loop {
            if cancellation.is_cancelled() {
                return Err(CodeIntelligenceError::Cancelled);
            }
            if let Some(result) = outcome.borrow().clone() {
                return result;
            }
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(CodeIntelligenceError::Cancelled),
                changed = outcome.changed() => {
                    if changed.is_err() {
                        return Err(CodeIntelligenceError::Unavailable {
                            message: "language runtime start task ended before publishing an outcome".to_owned(),
                        });
                    }
                }
            }
        }
    }

    async fn wait_for_start_completion(mut outcome: watch::Receiver<Option<RuntimeStartOutcome>>) {
        loop {
            if outcome.borrow().is_some() {
                return;
            }
            if outcome.changed().await.is_err() {
                return;
            }
        }
    }

    fn spawn_runtime_monitor(
        state: Arc<Mutex<SlotState>>,
        status: watch::Sender<CodeIntelligenceStatus>,
        status_updates: Arc<Mutex<()>>,
        lifetime: CancellationToken,
        profile: ProjectLanguageProfile,
        runtime: Arc<LanguageRuntime>,
    ) {
        let language = profile_language(profile);
        let mut process_state = runtime.subscribe_process_state();
        tokio::spawn(async move {
            loop {
                if !matches!(
                    *process_state.borrow(),
                    super::super::lsp::process::LspProcessState::Running
                ) {
                    break;
                }
                tokio::select! {
                    _ = lifetime.cancelled() => return,
                    changed = process_state.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
            }
            if lifetime.is_cancelled() {
                return;
            }
            let message = runtime
                .unavailable_message()
                .unwrap_or_else(|| "the language runtime stopped unexpectedly".to_owned());
            {
                let _status_update = status_updates.lock().await;
                let mut slot_state = state.lock().await;
                let is_current = matches!(
                    &*slot_state,
                    SlotState::Ready(current) if Arc::ptr_eq(current, &runtime)
                );
                if !is_current || lifetime.is_cancelled() {
                    return;
                }
                *slot_state = SlotState::Failed(StartFailure {
                    at: Instant::now(),
                    message: message.clone(),
                    retained_runtime: Some(Arc::clone(&runtime)),
                });
                publish_stopped_language_status(&status, language.clone(), message.clone());
            }

            let cleanup = runtime.shutdown().await;
            let _status_update = status_updates.lock().await;
            let mut slot_state = state.lock().await;
            let is_current = matches!(
                &*slot_state,
                SlotState::Failed(failure)
                    if failure.retained_runtime.as_ref().is_some_and(|current| Arc::ptr_eq(current, &runtime))
            );
            if !is_current {
                return;
            }
            match cleanup {
                Ok(()) => {
                    *slot_state = SlotState::Dormant;
                    publish_stopped_language_status(&status, language, message);
                }
                Err(error) => {
                    let message = format!("{message}; cleanup failed: {error}");
                    tracing::warn!(
                        language = %language,
                        error = %error,
                        "Code Intelligence could not clean up a stopped language runtime"
                    );
                    *slot_state = SlotState::Failed(StartFailure {
                        at: Instant::now(),
                        message: message.clone(),
                        retained_runtime: Some(runtime),
                    });
                    publish_stopped_language_status(&status, language, message);
                }
            }
        });
    }
}

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_owned()
}
