use super::*;

/// Workspace-bound session. All LLM and tool operations happen here.
#[napi]
pub struct Session {
    pub(super) inner: Arc<RustAgentSession>,
}

#[napi]
impl Session {
    /// Send a prompt or request and wait for the complete response.
    ///
    /// `send("prompt")` is the compact prompt-first form. `send({ prompt,
    /// history, attachments })` is the compact object-shaped form for growth.
    #[napi(
        ts_args_type = "request: string | SessionRequestOptions, history?: Array<MessageObject> | null"
    )]
    pub async fn send(
        &self,
        request: Either<String, SessionRequestOptions>,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<AgentResult> {
        let (prompt, rust_history, rust_attachments) = session_request_parts(request, history)?;
        send_session_request(self.inner.clone(), prompt, rust_history, rust_attachments).await
    }

    /// Alias for `send(...)` with a name that matches run/replay terminology.
    #[napi(
        ts_args_type = "request: string | SessionRequestOptions, history?: Array<MessageObject> | null"
    )]
    pub async fn run(
        &self,
        request: Either<String, SessionRequestOptions>,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<AgentResult> {
        let (prompt, rust_history, rust_attachments) = session_request_parts(request, history)?;
        send_session_request(self.inner.clone(), prompt, rust_history, rust_attachments).await
    }

    /// Resume a previously-checkpointed run on this session.
    ///
    /// Loads the latest loop checkpoint stored under `checkpointRunId`
    /// from the configured `SessionStore` and replays the agent loop
    /// from that boundary. A new run id is allocated for the resumed
    /// work; the relationship between the old and new run is host
    /// metadata.
    ///
    /// Rejects when the session has no `sessionStore` configured, or
    /// when no checkpoint exists for `checkpointRunId`.
    #[napi]
    pub async fn resume_run(&self, checkpoint_run_id: String) -> napi::Result<AgentResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.resume_run(&checkpoint_run_id).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(AgentResult::from(result))
    }

    /// Run `specs` as a fan-out of agent steps, bounded by the session's
    /// configured parallelism, and resolve with each step's outcome in input
    /// order. A failed step surfaces as `success: false` without failing the
    /// batch.
    ///
    /// Pass `budgetTokens` to run the fan-out under one shared token budget:
    /// every child agent feeds a single ledger and, once the cap is reached,
    /// further child LLM calls are denied (a *soft* cap — a wide fan-out can race
    /// a few in-flight turns past it before the post-call ledger catches up; the
    /// in-flight fan-out is never force-killed). With a budget the result is
    /// `{ outcomes, budget }` (the ledger snapshot); without one it is the plain
    /// outcomes array, unchanged.
    #[napi(ts_return_type = "Promise<Array<StepOutcomeObject> | WorkflowParallelResult>")]
    pub async fn parallel(
        &self,
        specs: Vec<AgentStepSpecObject>,
        budget_tokens: Option<i64>,
    ) -> napi::Result<Either<Vec<StepOutcomeObject>, WorkflowParallelResult>> {
        let session = self.inner.clone();
        let rust_specs: Vec<RustAgentStepSpec> = specs.into_iter().map(Into::into).collect();

        // No budget → unchanged behavior: the plain outcomes array.
        let Some(budget) = budget_tokens else {
            let outcomes = get_runtime()
                .spawn(async move {
                    let executor = session.agent_executor();
                    execute_steps_parallel(executor, rust_specs, None).await
                })
                .await
                .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
            return Ok(Either::A(
                outcomes.into_iter().map(StepOutcomeObject::from).collect(),
            ));
        };

        // Budget → shared ledger across the fan-out; return outcomes + snapshot.
        let limit = budget.max(0) as u64;
        let (outcomes, snapshot) = get_runtime()
            .spawn(async move {
                let wf = session.workflow_with_token_budget(Some(limit));
                let outcomes = wf.parallel(rust_specs).await;
                (outcomes, wf.budget_snapshot())
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(Either::B(WorkflowParallelResult {
            outcomes: outcomes.into_iter().map(StepOutcomeObject::from).collect(),
            budget: snapshot
                .map(|b| WorkflowBudgetObject {
                    consumed_tokens: b.consumed_tokens as i64,
                    limit_tokens: b.limit_tokens.map(|l| l as i64),
                })
                .unwrap_or(WorkflowBudgetObject {
                    consumed_tokens: 0,
                    limit_tokens: Some(limit as i64),
                }),
        }))
    }

    /// Like `parallel`, but resumable: progress is journaled under
    /// `workflowId` via the session's `sessionStore`, so an interrupted run
    /// skips already-completed steps. Rejects when no `sessionStore` is
    /// configured.
    #[napi]
    pub async fn parallel_resumable(
        &self,
        specs: Vec<AgentStepSpecObject>,
        workflow_id: String,
    ) -> napi::Result<Vec<StepOutcomeObject>> {
        let session = self.inner.clone();
        let rust_specs: Vec<RustAgentStepSpec> = specs.into_iter().map(Into::into).collect();
        let outcomes = get_runtime()
            .spawn(async move {
                let Some(store) = session.session_store() else {
                    return Err("parallelResumable requires a sessionStore on the session");
                };
                let executor = session.agent_executor();
                Ok(execute_steps_parallel_resumable(
                    executor,
                    rust_specs,
                    &workflow_id,
                    store,
                    None,
                )
                .await)
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(napi::Error::from_reason)?;
        Ok(outcomes.into_iter().map(StepOutcomeObject::from).collect())
    }

    /// Run each item through a chain of `stages`, with no barrier between
    /// stages — item A can be in stage 3 while item B is still in stage 1.
    ///
    /// Each stage is a function `(ctx) => spec | null` where `ctx` is
    /// `{ previous: StepOutcomeObject | null, item: any }`. Return an
    /// `AgentStepSpecObject` (camelCase keys) to run that step, or `null` to
    /// stop the item's chain. A chain also stops when a step fails.
    ///
    /// A callback exception, asynchronous return, malformed return, or timeout
    /// fails closed (it is treated as `null`, stopping that item's chain).
    /// `timeoutMs` defaults to 30 seconds.
    ///
    /// This is a *synchronous* napi method that returns a Promise via a
    /// deferred: the JS stage functions (which are not `Send`) are converted
    /// to thread-safe functions on the JS thread here, then the chains run on
    /// the worker runtime and resolve the Promise — so the event loop is never
    /// blocked and no non-`Send` value crosses the async boundary.
    #[napi(
        ts_args_type = "items: Array<any>, stages: Array<(ctx: { previous: StepOutcomeObject | null, item: any }) => AgentStepSpecObject | null>, timeoutMs?: number",
        ts_return_type = "Promise<Array<StepOutcomeObject | null>>"
    )]
    pub fn pipeline(
        &self,
        env: Env,
        items: Vec<serde_json::Value>,
        stages: Vec<napi::JsFunction>,
        timeout_ms: Option<u32>,
    ) -> napi::Result<napi::JsObject> {
        use napi::threadsafe_function::{ThreadSafeCallContext, ThreadsafeFunction};
        // Single-object arg so the JS stage signature is `(ctx) => ...`.
        let single_obj = |ctx: ThreadSafeCallContext<serde_json::Value>| {
            Ok(vec![ctx.env.to_js_value(&ctx.value)?])
        };
        let timeout = timeout_ms.map(|t| t as u64).unwrap_or(30_000);

        // Build the thread-safe functions on the JS thread (JsFunction is not
        // Send), then wrap each as a synchronous PipelineStage the combinator
        // can call from the worker runtime.
        let rust_stages: Vec<RustPipelineStage<serde_json::Value>> = stages
            .into_iter()
            .map(|f| {
                let safe = wrap_sync_callback(&env, f)?;
                let tsfn: ThreadsafeFunction<
                    serde_json::Value,
                    napi::threadsafe_function::ErrorStrategy::Fatal,
                > = safe.create_threadsafe_function(0, single_obj)?;
                let stage = Arc::new(NodePipelineStage {
                    tsfn,
                    timeout_ms: timeout,
                });
                let pipeline_stage: RustPipelineStage<serde_json::Value> =
                    Arc::new(move |prev, item| stage.invoke(prev, item));
                Ok::<_, napi::Error>(pipeline_stage)
            })
            .collect::<napi::Result<Vec<_>>>()?;

        let session = self.inner.clone();
        let (deferred, promise) = env.create_deferred::<Vec<Option<StepOutcomeObject>>, _>()?;
        get_runtime().spawn(async move {
            let executor = session.agent_executor();
            let outcomes = execute_pipeline(executor, items, rust_stages, None).await;
            let mapped: Vec<Option<StepOutcomeObject>> = outcomes
                .into_iter()
                .map(|o| o.map(StepOutcomeObject::from))
                .collect();
            deferred.resolve(move |_env| Ok(mapped));
        });
        Ok(promise)
    }

    /// Send a prompt or request and get a streaming event iterator.
    ///
    /// Returns an `EventStream`. Use `for await (const event of stream)` or call `.next()` manually.
    /// When `history` is omitted, the session history and verification evidence are
    /// updated after the stream completes. Supplying `history` keeps the stream isolated.
    #[napi(
        ts_args_type = "request: string | SessionRequestOptions, history?: Array<MessageObject> | null"
    )]
    pub async fn stream(
        &self,
        request: Either<String, SessionRequestOptions>,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<EventStream> {
        let (prompt, rust_history, rust_attachments) = session_request_parts(request, history)?;
        stream_session_request(self.inner.clone(), prompt, rust_history, rust_attachments).await
    }

    /// Send a request using the long-lived object-shaped API.
    ///
    /// Prefer this for new integrations when the call may need history,
    /// attachments, or future request options.
    #[napi(js_name = "sendRequest")]
    pub async fn send_request(&self, request: SessionRequestOptions) -> napi::Result<AgentResult> {
        let (prompt, rust_history, rust_attachments) =
            session_request_parts(Either::B(request), None)?;
        send_session_request(self.inner.clone(), prompt, rust_history, rust_attachments).await
    }

    /// Stream a request using the long-lived object-shaped API.
    #[napi(js_name = "streamRequest")]
    pub async fn stream_request(
        &self,
        request: SessionRequestOptions,
    ) -> napi::Result<EventStream> {
        let (prompt, rust_history, rust_attachments) =
            session_request_parts(Either::B(request), None)?;
        stream_session_request(self.inner.clone(), prompt, rust_history, rust_attachments).await
    }

    /// Send a prompt with image attachments and wait for the complete response.
    ///
    /// @param prompt - The prompt to send
    /// @param attachments - Array of `{ data: Buffer, mediaType: string }`
    /// @param history - Optional conversation history
    #[napi]
    pub async fn send_with_attachments(
        &self,
        prompt: String,
        attachments: Vec<AttachmentObject>,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<AgentResult> {
        let rust_attachments = js_attachments_to_rust(&attachments);
        let rust_history = history.map(|h| js_messages_to_rust(&h)).transpose()?;
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move {
                session
                    .send_with_attachments(&prompt, &rust_attachments, rust_history.as_deref())
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(AgentResult::from(result))
    }

    /// Stream a prompt with image attachments.
    ///
    /// When `history` is omitted, the session history and verification evidence are
    /// updated after the stream completes. Supplying `history` keeps the stream isolated.
    ///
    /// @param prompt - The prompt to send
    /// @param attachments - Array of `{ data: Buffer, mediaType: string }`
    /// @param history - Optional conversation history
    #[napi]
    pub async fn stream_with_attachments(
        &self,
        prompt: String,
        attachments: Vec<AttachmentObject>,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<EventStream> {
        let rust_attachments = js_attachments_to_rust(&attachments);
        let rust_history = history.map(|h| js_messages_to_rust(&h)).transpose()?;
        let session = self.inner.clone();
        let (rx, handle) = get_runtime()
            .spawn(async move {
                session
                    .stream_with_attachments(&prompt, &rust_attachments, rust_history.as_deref())
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(EventStream {
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(Some(handle))),
        })
    }

    /// Return the session's conversation history.
    #[napi]
    pub fn history(&self) -> Vec<MessageObject> {
        rust_messages_to_js(&self.inner.history())
    }

    /// Return run snapshots recorded by this session.
    #[napi]
    pub async fn runs(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let runs = get_runtime()
            .spawn(async move { session.runs().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(runs)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return a run snapshot by ID, or null when it is unknown.
    #[napi(js_name = "runSnapshot")]
    pub async fn run_snapshot(&self, run_id: String) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let snapshot = get_runtime()
            .spawn(async move { session.run_snapshot(&run_id).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(snapshot)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return recorded runtime events for a run.
    #[napi(
        js_name = "runEvents",
        ts_return_type = "Promise<Array<{ version: 1; type: string; payload: unknown; metadata: { run_id: string; session_id: string; sequence: number; timestamp_ms: number } }>>"
    )]
    pub async fn run_events(&self, run_id: String) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let session_id = self.inner.session_id().to_string();
        let requested_run_id = run_id.clone();
        let events = get_runtime()
            .spawn(async move { session.run_events(&requested_run_id).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        let envelopes = events
            .iter()
            .map(|record| rust_run_event_envelope_v1(record, &run_id, &session_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| napi::Error::from_reason(format!("Event protocol error: {e}")))?;
        serde_json::to_value(envelopes)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return a cursor-based page from the retained event window.
    #[napi(
        js_name = "runEventPage",
        ts_return_type = "Promise<{ events: Array<{ version: 1; type: string; payload: unknown; metadata: { run_id: string; session_id: string; sequence: number; timestamp_ms: number } }>; firstAvailableSequence: number | null; latestSequenceExclusive: number; nextAfterSequence: number | null; retentionGap: boolean; hasMore: boolean } | null>"
    )]
    pub async fn run_event_page(
        &self,
        run_id: String,
        after_sequence: Option<f64>,
        limit: Option<f64>,
    ) -> napi::Result<serde_json::Value> {
        let after_sequence = match after_sequence {
            Some(value) => Some(js_optional_usize(Some(value), "afterSequence", 0)?),
            None => None,
        };
        let limit = js_optional_usize(limit, "limit", 256)?;
        let session = self.inner.clone();
        let session_id = self.inner.session_id().to_string();
        let requested_run_id = run_id.clone();
        let page = get_runtime()
            .spawn(async move {
                session
                    .run_event_page(&requested_run_id, after_sequence, limit)
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        let Some(page) = page else {
            return Ok(serde_json::Value::Null);
        };
        let events = page
            .events
            .iter()
            .map(|record| rust_run_event_envelope_v1(record, &run_id, &session_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| napi::Error::from_reason(format!("Event protocol error: {e}")))?;
        Ok(serde_json::json!({
            "events": events,
            "firstAvailableSequence": page.first_available_sequence,
            "latestSequenceExclusive": page.latest_sequence_exclusive,
            "nextAfterSequence": page.next_after_sequence,
            "retentionGap": page.retention_gap,
            "hasMore": page.has_more,
        }))
    }

    /// Return the currently running operation, or null when idle.
    #[napi(js_name = "currentRun")]
    pub async fn current_run(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let snapshot = get_runtime()
            .spawn(async move {
                match session.current_run().await {
                    Some(run) => run.snapshot().await,
                    None => None,
                }
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(snapshot)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return active tool calls observed for the currently running operation.
    #[napi(js_name = "activeTools")]
    pub async fn active_tools(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let active_tools = get_runtime()
            .spawn(async move { session.active_tools().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(active_tools)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Look up a delegated subagent task by id. Resolves to `null` when no
    /// such task has been observed in this session.
    #[napi(js_name = "subagentTask")]
    pub async fn subagent_task(&self, task_id: String) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let snapshot = get_runtime()
            .spawn(async move { session.subagent_task(&task_id).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(snapshot)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return snapshots of every delegated subagent task observed in this
    /// session (including completed and failed ones), oldest first.
    #[napi(js_name = "subagentTasks")]
    pub async fn subagent_tasks(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let tasks = get_runtime()
            .spawn(async move { session.subagent_tasks().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(tasks)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return snapshots of subagent tasks still in `running` state.
    #[napi(js_name = "pendingSubagentTasks")]
    pub async fn pending_subagent_tasks(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let tasks = get_runtime()
            .spawn(async move { session.pending_subagent_tasks().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(tasks)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Cancel an in-flight subagent task by id. Resolves to `true` when a
    /// cancellation token was found and fired, `false` when the task id
    /// is unknown or the task already finished.
    #[napi(js_name = "cancelSubagentTask")]
    pub async fn cancel_subagent_task(&self, task_id: String) -> napi::Result<bool> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.cancel_subagent_task(&task_id).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))
    }

    /// Cancel a specific run only if it is still the active run.
    #[napi(js_name = "cancelRun")]
    pub async fn cancel_run(&self, run_id: String) -> napi::Result<bool> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.cancel_run(&run_id).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))
    }

    // ========================================================================
    // Hook API
    // ========================================================================

    /// Register a hook for lifecycle event interception.
    ///
    /// Hooks registered on a session are automatically propagated to all sub-agents
    /// spawned by the `task` tool, including grandchild agents at arbitrary depth.
    /// This ensures security hooks (e.g. a sentinel) apply across the full agent tree
    /// without requiring explicit registration on each sub-agent session.
    ///
    /// @param hookId - Unique hook identifier
    /// @param eventType - Event type such as "pre_tool_use", "post_tool_use",
    ///   "pre_prompt", "post_response", "pre_planning", or "post_planning".
    /// @param matcher - Optional matcher: { tool?, pathPattern?, commandPattern?, sessionId?, skill? }
    /// @param config - Optional config: { priority?, timeoutMs?, asyncExecution?, maxRetries? }
    /// @param handler - Optional callback `(event: any) => { action: 'continue' | 'block' | 'skip' | 'retry',
    ///   reason?: string, delayMs?: number, modified?: any } | null`. When provided, the function is
    ///   called for every matching event and its return value controls execution. Return
    ///   `{ action: 'block', reason: '...' }` to cancel the operation, `{ action: 'skip' }` to skip
    ///   remaining hooks, `{ action: 'retry', delayMs: 1000 }` to request a retry, or
    ///   `{ action: 'continue', modified: {...} }` to continue with modified data. Hooks with no
    ///   handler still fire (observable via stream events) but always continue.
    #[napi]
    pub fn register_hook(
        &self,
        env: Env,
        hook_id: String,
        event_type: String,
        matcher: Option<HookMatcherObject>,
        config: Option<HookConfigObject>,
        #[napi(
            ts_arg_type = "((event: Record<string, unknown>) => { action: string; reason?: string; delayMs?: number; modified?: any } | null | undefined) | null | undefined"
        )]
        handler: Option<napi::JsFunction>,
    ) -> napi::Result<()> {
        use napi::threadsafe_function::{ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction};

        let rust_event_type = parse_hook_event_type(&event_type)?;
        let mut hook = RustHook::new(&hook_id, rust_event_type);

        if let Some(m) = matcher {
            let mut rust_matcher = RustHookMatcher::new();
            if let Some(tool) = m.tool {
                rust_matcher = rust_matcher.with_tool(tool);
            }
            if let Some(path) = m.path_pattern {
                rust_matcher = rust_matcher.with_path(path);
            }
            if let Some(cmd) = m.command_pattern {
                rust_matcher = rust_matcher.with_command(cmd);
            }
            if let Some(sid) = m.session_id {
                rust_matcher = rust_matcher.with_session(sid);
            }
            if let Some(skill) = m.skill {
                rust_matcher = rust_matcher.with_skill(skill);
            }
            hook = hook.with_matcher(rust_matcher);
        }

        if let Some(c) = config {
            hook = hook.with_config(RustHookConfig {
                priority: c.priority.unwrap_or(100),
                timeout_ms: c.timeout_ms.map(|v| v as u64).unwrap_or(30000),
                async_execution: c.async_execution.unwrap_or(false),
                max_retries: c.max_retries.unwrap_or(0),
            });
        }

        let timeout_ms = hook.config.timeout_ms;
        let handler = if let Some(js_fn) = handler {
            let safe_handler = wrap_sync_callback(&env, js_fn)?;
            let tsfn: ThreadsafeFunction<serde_json::Value, ErrorStrategy::Fatal> = safe_handler
                .create_threadsafe_function(
                    0,
                    |ctx: ThreadSafeCallContext<serde_json::Value>| {
                        let js_val = ctx.env.to_js_value(&ctx.value)?;
                        Ok(vec![js_val])
                    },
                )?;
            Some(Arc::new(NodeCallbackHandler { tsfn, timeout_ms }))
        } else {
            None
        };

        // Construct every fallible JavaScript bridge before publishing the
        // hook. register_hook_handler itself is infallible, so callers never
        // observe a half-registered hook after an error.
        self.inner.register_hook(hook).map_err(node_code_error)?;
        if let Some(handler) = handler {
            self.inner
                .register_hook_handler(&hook_id, handler)
                .map_err(node_code_error)?;
        } else {
            // Re-registering an existing hook without a handler must not retain
            // the previous callback under the same ID.
            self.inner
                .unregister_hook_handler(&hook_id)
                .map_err(node_code_error)?;
        }

        Ok(())
    }

    /// Unregister a hook by ID.
    ///
    /// @param hookId - The hook identifier to remove
    /// @returns true if the hook was found and removed
    #[napi]
    pub fn unregister_hook(&self, hook_id: String) -> napi::Result<bool> {
        self.inner
            .unregister_hook_handler(&hook_id)
            .map_err(node_code_error)?;
        self.inner
            .unregister_hook(&hook_id)
            .map(|hook| hook.is_some())
            .map_err(node_code_error)
    }

    /// Get the number of registered hooks.
    #[napi]
    pub fn hook_count(&self) -> u32 {
        self.inner.hook_count() as u32
    }

    // ========================================================================
    // Session Metadata API
    // ========================================================================

    /// Return the session ID.
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Return the workspace path.
    #[napi(getter)]
    pub fn workspace(&self) -> String {
        self.inner.workspace().display().to_string()
    }

    /// Return any deferred init warning (e.g. memory store failed to initialize).
    #[napi(getter)]
    pub fn init_warning(&self) -> Option<String> {
        self.inner.init_warning().map(|s| s.to_string())
    }

    /// Host-defined tenant id attached at session creation, if any.
    #[napi(getter)]
    pub fn tenant_id(&self) -> Option<String> {
        self.inner.tenant_id().map(|s| s.to_string())
    }

    /// Identity of the principal that triggered the session, if any.
    #[napi(getter)]
    pub fn principal(&self) -> Option<String> {
        self.inner.principal().map(|s| s.to_string())
    }

    /// Logical agent template / definition id, if any.
    #[napi(getter)]
    pub fn agent_template_id(&self) -> Option<String> {
        self.inner.agent_template_id().map(|s| s.to_string())
    }

    /// Distributed-trace correlation id propagated through this session, if any.
    #[napi(getter)]
    pub fn correlation_id(&self) -> Option<String> {
        self.inner.correlation_id().map(|s| s.to_string())
    }

    // ========================================================================
    // Session Persistence API
    // ========================================================================

    /// Save the session to the configured store.
    #[napi]
    pub async fn save(&self) -> napi::Result<()> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.save().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)
    }

    // ========================================================================
    // Slash Command & Scheduler API
    // ========================================================================

    /// Register a custom slash command.
    ///
    /// Slash commands are invoked via `session.send("/command args")` and execute
    /// before the LLM sees the input. The handler receives the command arguments
    /// and a context object with session metadata.
    ///
    /// @param name - Command name without the leading `/` (e.g., `"status"`)
    /// @param description - Short description shown in `/help`
    /// @param handler - Callback `(args: string, ctx: CommandContext) => string`
    /// @param timeoutMs - Maximum synchronous callback time (default 5000ms)
    ///
    /// @example
    /// ```typescript
    /// session.registerCommand("status", "Show session info", (args, ctx) => {
    ///   return `Session ${ctx.sessionId} in ${ctx.workspace}`;
    /// });
    /// await session.send("/status");
    /// ```
    #[napi(
        ts_args_type = "name: string, description: string, handler: (args: string, ctx: CommandContext) => string, timeoutMs?: number"
    )]
    pub fn register_command(
        &self,
        env: Env,
        name: String,
        description: String,
        handler: napi::JsFunction,
        timeout_ms: Option<u32>,
    ) -> napi::Result<()> {
        use napi::threadsafe_function::ThreadSafeCallContext;

        // The TSFN must only invoke the never-throw JavaScript wrapper.  A
        // user exception is decoded into a regular command error in Rust.
        let safe_handler = wrap_sync_callback(&env, handler)?;
        let tsfn: napi::threadsafe_function::ThreadsafeFunction<
            (String, RustCommandContext),
            napi::threadsafe_function::ErrorStrategy::Fatal,
        > = safe_handler.create_threadsafe_function(
            0,
            |ctx: ThreadSafeCallContext<(String, RustCommandContext)>| {
                // Extract the values
                let args = ctx.value.0;
                let cmd_ctx = ctx.value.1;

                // Convert to JS values
                let args_str = ctx.env.create_string(&args)?;
                let ctx_obj = js_command_context_to_object(&ctx.env, &cmd_ctx)?;

                // Return the arguments that will be passed to the JS function
                Ok(vec![args_str.into_unknown(), ctx_obj.into_unknown()])
            },
        )?;

        let cmd = Arc::new(JsSlashCommand {
            name,
            description,
            handler: Arc::new(tsfn),
            timeout_ms: timeout_ms.map(u64::from).unwrap_or(5_000),
        });
        self.inner
            .clone()
            .register_command(cmd)
            .map_err(node_code_error)?;
        Ok(())
    }

    /// List all registered slash commands.
    ///
    /// Returns each command's name, description, and optional usage hint.
    /// Slash commands can be invoked via `session.send("/command args")`.
    ///
    /// @returns Array of CommandInfo objects sorted by name
    #[napi]
    pub fn list_commands(&self) -> Vec<CommandInfo> {
        self.inner
            .command_registry()
            .list_full()
            .into_iter()
            .map(|(name, description, usage)| CommandInfo {
                name,
                description,
                usage,
            })
            .collect()
    }

    /// Cancel the current ongoing operation (send/stream).
    ///
    /// If an operation is in progress, this will trigger cancellation of the LLM streaming
    /// and tool execution. The operation will terminate as soon as possible.
    ///
    /// @returns `true` if an operation was cancelled, `false` if no operation was in progress
    /// @deprecated Prefer `cancelAsync()` to avoid blocking the JavaScript event loop.
    #[napi]
    pub fn cancel(&self) -> bool {
        let session = self.inner.clone();
        get_runtime().block_on(session.cancel())
    }

    /// Asynchronously cancel the active operation without blocking the
    /// JavaScript event loop.
    #[napi(js_name = "cancelAsync")]
    pub async fn cancel_async(&self) -> napi::Result<bool> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.cancel().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))
    }

    /// Close the session and cancel any active operation.
    ///
    /// Call this when the session will no longer be used so Node.js can exit
    /// cleanly without waiting on session-scoped background workers.
    /// @deprecated Prefer `closeAsync()` to avoid blocking the JavaScript event loop.
    #[napi]
    pub fn close(&self) {
        let session = self.inner.clone();
        get_runtime().block_on(session.close())
    }

    /// Asynchronously close the session without blocking the JavaScript event
    /// loop. New applications should prefer this over `close()`.
    #[napi(js_name = "closeAsync")]
    pub async fn close_async(&self) -> napi::Result<()> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.close().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(())
    }

    /// Whether [`close`](#method.close) has been called on this session.
    ///
    /// Once `true`, calls to `send` / `stream` reject with a "Session closed"
    /// error instead of starting a new run.
    #[napi]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Install a host-supplied BudgetGuard on this session.
    ///
    /// Each callback receives a single context object:
    /// - `checkBeforeLlm({ sessionId, estimatedTokens }) -> BudgetDecision | null`
    /// - `recordAfterLlm({ sessionId, usage }) -> void`
    /// - `checkBeforeTool({ sessionId, toolName }) -> BudgetDecision | null`
    ///
    /// where `BudgetDecision` is one of:
    /// - `null` / `{ decision: 'allow' }`                                                     → allow
    /// - `{ decision: 'soft', resource, consumed, limit, message? }`                          → emits BudgetThresholdHit('soft'), proceeds
    /// - `{ decision: 'deny',  resource, reason }`                                            → aborts the call, throws "Budget exhausted"
    ///
    /// FAIL-CLOSED on hang: a `check*` callback that does not return
    /// within `timeoutMs` (default 5000) is treated as a **deny**, never
    /// a silent allow — a budget control must not disable itself when the
    /// guard stalls. A malformed/unreadable return likewise denies.
    ///
    /// Callback exceptions and asynchronous or malformed returns are converted
    /// to controlled failures. Check callbacks fail closed with a deny;
    /// `recordAfterLlm` failures are safely ignored.
    ///
    /// The guard takes effect on the next `send` / `stream`. Pass `null`
    /// for a method to leave it unhandled (default allow / no-op). Pass
    /// `null` for the whole handlers arg to clear the guard.
    #[napi(
        ts_args_type = "handlers: { checkBeforeLlm?: ((ctx: { sessionId: string; estimatedTokens: number }) => any) | null; recordAfterLlm?: ((ctx: { sessionId: string; usage: any }) => void) | null; checkBeforeTool?: ((ctx: { sessionId: string; toolName: string }) => any) | null; timeoutMs?: number | null } | null"
    )]
    pub fn set_budget_guard(
        &self,
        env: Env,
        handlers: Option<BudgetGuardHandlers>,
    ) -> napi::Result<()> {
        use napi::threadsafe_function::{ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction};

        let Some(h) = handlers else {
            self.inner.set_budget_guard(None).map_err(node_code_error)?;
            return Ok(());
        };

        // Pass the call context as a SINGLE object arg so the JS callback
        // signature is the clean `(ctx) => decision`.  Only the never-throw
        // wrapper is handed to ErrorStrategy::Fatal; user exceptions travel
        // back as tagged values and are handled below.
        let single_obj = |ctx: ThreadSafeCallContext<serde_json::Value>| {
            Ok(vec![ctx.env.to_js_value(&ctx.value)?])
        };

        let check_llm_tsfn: Option<ThreadsafeFunction<serde_json::Value, ErrorStrategy::Fatal>> = h
            .check_before_llm
            .map(|f| wrap_sync_callback(&env, f)?.create_threadsafe_function(0, single_obj))
            .transpose()?;

        let record_tsfn: Option<ThreadsafeFunction<serde_json::Value, ErrorStrategy::Fatal>> = h
            .record_after_llm
            .map(|f| wrap_sync_callback(&env, f)?.create_threadsafe_function(0, single_obj))
            .transpose()?;

        let check_tool_tsfn: Option<ThreadsafeFunction<serde_json::Value, ErrorStrategy::Fatal>> =
            h.check_before_tool
                .map(|f| wrap_sync_callback(&env, f)?.create_threadsafe_function(0, single_obj))
                .transpose()?;

        let guard: Arc<dyn a3s_code_core::budget::BudgetGuard> = Arc::new(NodeBudgetGuard {
            check_before_llm: check_llm_tsfn,
            record_after_llm: record_tsfn,
            check_before_tool: check_tool_tsfn,
            // Configurable; default 5s. On timeout the guard fails CLOSED
            // (Deny), so a small value trades latency-on-hang for faster
            // denial of a stuck guard.
            timeout_ms: h.timeout_ms.map(|t| t as u64).unwrap_or(5_000),
        });
        self.inner
            .set_budget_guard(Some(guard))
            .map_err(node_code_error)?;
        Ok(())
    }
}

// ============================================================================
// Node-side BudgetGuard wrapper
