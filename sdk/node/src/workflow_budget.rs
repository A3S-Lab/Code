use super::*;

/// One unit of orchestrated agent work — what to run, independent of where.
#[napi(object)]
#[derive(Clone)]
pub struct AgentStepSpecObject {
    /// Stable id for this step (assigned by the caller).
    pub task_id: String,
    /// Registry key of the agent to run (e.g. "explore", "review").
    pub agent: String,
    /// Short label for display/tracking.
    pub description: String,
    /// Instruction handed to the child agent.
    pub prompt: String,
    /// Optional per-step tool-round cap.
    pub max_steps: Option<u32>,
    /// Optional parent session id for event correlation.
    pub parent_session_id: Option<String>,
    /// When set, the step must return JSON conforming to this schema; the
    /// validated object lands in `StepOutcomeObject.structured`.
    pub output_schema: Option<serde_json::Value>,
}

impl From<AgentStepSpecObject> for RustAgentStepSpec {
    fn from(o: AgentStepSpecObject) -> Self {
        RustAgentStepSpec {
            task_id: o.task_id,
            agent: o.agent,
            description: o.description,
            prompt: o.prompt,
            max_steps: o.max_steps.map(|n| n as usize),
            parent_session_id: o.parent_session_id,
            output_schema: o.output_schema,
        }
    }
}

/// A source location observed by a successful delegated research tool.
#[napi(object)]
#[derive(Clone)]
pub struct ToolSourceAnchorObject {
    pub tool: String,
    pub url_or_path: String,
}

impl From<RustToolSourceAnchor> for ToolSourceAnchorObject {
    fn from(anchor: RustToolSourceAnchor) -> Self {
        Self {
            tool: anchor.tool,
            url_or_path: anchor.url_or_path,
        }
    }
}

/// The result of running one orchestrated step.
#[napi(object)]
#[derive(Clone)]
pub struct StepOutcomeObject {
    pub task_id: String,
    pub session_id: String,
    pub agent: String,
    pub output: String,
    pub success: bool,
    /// Schema-validated structured output, when the step requested one.
    pub structured: Option<serde_json::Value>,
    /// Source locations observed by successful child research tools.
    pub source_anchors: Vec<ToolSourceAnchorObject>,
}

impl From<RustStepOutcome> for StepOutcomeObject {
    fn from(o: RustStepOutcome) -> Self {
        StepOutcomeObject {
            task_id: o.task_id,
            session_id: o.session_id,
            agent: o.agent,
            output: o.output,
            success: o.success,
            structured: o.structured,
            source_anchors: o.source_anchors.into_iter().map(Into::into).collect(),
        }
    }
}

/// A snapshot of a workflow's shared token ledger. `consumedTokens` is the total
/// recorded across every step; `limitTokens` is the hard ceiling, if one was set.
#[napi(object)]
#[derive(Clone)]
pub struct WorkflowBudgetObject {
    pub consumed_tokens: i64,
    pub limit_tokens: Option<i64>,
}

/// The result of a budgeted workflow fan-out: the per-step outcomes plus the
/// shared budget ledger snapshot.
#[napi(object)]
pub struct WorkflowParallelResult {
    pub outcomes: Vec<StepOutcomeObject>,
    pub budget: WorkflowBudgetObject,
}

/// Shape of the JS handlers object accepted by `session.setBudgetGuard`.
/// Each field is optional — methods that aren't provided fall back to
/// the framework's default Allow / no-op behaviour.
#[napi(object)]
pub struct BudgetGuardHandlers {
    pub check_before_llm: Option<napi::JsFunction>,
    pub record_after_llm: Option<napi::JsFunction>,
    pub check_before_tool: Option<napi::JsFunction>,
    /// Max time (ms) to wait for a `check*` callback to return before
    /// the guard fails **closed** (denies). Default 5000. A guard that
    /// hangs is denied after this deadline; a thrown exception is converted
    /// to a deny immediately. Budget enforcement never silently disables
    /// itself.
    pub timeout_ms: Option<u32>,
}

pub(super) struct NodeBudgetGuard {
    pub(super) check_before_llm: Option<
        napi::threadsafe_function::ThreadsafeFunction<
            serde_json::Value,
            napi::threadsafe_function::ErrorStrategy::Fatal,
        >,
    >,
    pub(super) record_after_llm: Option<
        napi::threadsafe_function::ThreadsafeFunction<
            serde_json::Value,
            napi::threadsafe_function::ErrorStrategy::Fatal,
        >,
    >,
    pub(super) check_before_tool: Option<
        napi::threadsafe_function::ThreadsafeFunction<
            serde_json::Value,
            napi::threadsafe_function::ErrorStrategy::Fatal,
        >,
    >,
    pub(super) timeout_ms: u64,
}

// SAFETY: ThreadsafeFunction is designed to be sent across threads.
unsafe impl Send for NodeBudgetGuard {}
unsafe impl Sync for NodeBudgetGuard {}

/// Bridges a JS pipeline-stage function to a synchronous `PipelineStage`.
pub(super) struct NodePipelineStage {
    pub(super) tsfn: napi::threadsafe_function::ThreadsafeFunction<
        serde_json::Value,
        napi::threadsafe_function::ErrorStrategy::Fatal,
    >,
    pub(super) timeout_ms: u64,
}

// SAFETY: ThreadsafeFunction is designed to be sent across threads.
unsafe impl Send for NodePipelineStage {}
unsafe impl Sync for NodePipelineStage {}

impl NodePipelineStage {
    pub(super) fn invoke(
        &self,
        prev: Option<&RustStepOutcome>,
        item: &serde_json::Value,
    ) -> Option<RustAgentStepSpec> {
        let previous = prev
            .map(|o| serde_json::to_value(o).unwrap_or(serde_json::Value::Null))
            .unwrap_or(serde_json::Value::Null);
        let payload = serde_json::json!({ "previous": previous, "item": item });

        let (tx, rx) = std::sync::mpsc::sync_channel::<Option<RustAgentStepSpec>>(1);
        let status = self.tsfn.call_with_return_value(
            payload,
            napi::threadsafe_function::ThreadsafeFunctionCallMode::NonBlocking,
            move |ret: napi::JsUnknown| {
                // Fail closed without returning a napi conversion error from
                // this closure: doing so would call napi_fatal_error.
                let step = match decode_callback_outcome(ret) {
                    JsCallbackOutcome::Returned(value) => parse_js_step_spec(value),
                    JsCallbackOutcome::Failed(_) => None,
                };
                let _ = tx.send(step);
                Ok(())
            },
        );
        if status != napi::Status::Ok {
            return None;
        }
        // Fail closed on timeout by stopping this chain.
        tokio::task::block_in_place(|| {
            rx.recv_timeout(std::time::Duration::from_millis(self.timeout_ms))
                .unwrap_or(None)
        })
    }
}

/// Parse a JS pipeline-stage return value into an `AgentStepSpec`, or `None`
/// for `null`/`undefined`/unreadable input (which stops the chain). Accepts
/// camelCase (the SDK convention) and snake_case keys.
fn parse_js_step_spec(val: napi::JsUnknown) -> Option<RustAgentStepSpec> {
    use napi::{JsObject, ValueType};
    if !matches!(val.get_type().ok()?, ValueType::Object) {
        return None;
    }
    let obj = unsafe { val.cast::<JsObject>() };
    let get_str = |keys: &[&str]| -> Option<String> {
        for k in keys {
            if let Ok(s) = obj.get_named_property::<napi::JsString>(k) {
                if let Some(v) = s.into_utf8().ok().and_then(|s| s.into_owned().ok()) {
                    return Some(v);
                }
            }
        }
        None
    };
    let task_id = get_str(&["taskId", "task_id"])?;
    let agent = get_str(&["agent"])?;
    let prompt = get_str(&["prompt"])?;
    let description = get_str(&["description"]).unwrap_or_default();
    let max_steps = ["maxSteps", "max_steps"]
        .iter()
        .find_map(|k| obj.get_named_property::<napi::JsNumber>(k).ok())
        .and_then(|n| n.get_uint32().ok())
        .map(|n| n as usize);
    let parent_session_id = get_str(&["parentSessionId", "parent_session_id"]);
    Some(RustAgentStepSpec {
        task_id,
        agent,
        description,
        prompt,
        max_steps,
        parent_session_id,
        // Per-stage `outputSchema` is not yet supported on pipeline stages
        // (the lenient JsUnknown parse here can't read an arbitrary JSON-schema
        // property safely). Use `parallel` for schema-validated steps.
        output_schema: None,
    })
}

impl NodeBudgetGuard {
    fn call_decision(
        &self,
        tsfn: &napi::threadsafe_function::ThreadsafeFunction<
            serde_json::Value,
            napi::threadsafe_function::ErrorStrategy::Fatal,
        >,
        args: serde_json::Value,
    ) -> a3s_code_core::budget::BudgetDecision {
        let (tx, rx) = std::sync::mpsc::sync_channel::<a3s_code_core::budget::BudgetDecision>(1);
        let status = tsfn.call_with_return_value(
            args,
            napi::threadsafe_function::ThreadsafeFunctionCallMode::NonBlocking,
            move |ret: napi::JsUnknown| {
                // Never let a parser error escape this TSFN return closure.
                // napi-rs escalates such errors to napi_fatal_error.
                let decision = match decode_callback_outcome(ret) {
                    JsCallbackOutcome::Returned(value) => parse_js_budget_decision(value)
                        .unwrap_or_else(|error| a3s_code_core::budget::BudgetDecision::Deny {
                            resource: "budget_guard_error".to_string(),
                            reason: format!("invalid budget guard return: {error}"),
                        }),
                    JsCallbackOutcome::Failed(error) => {
                        a3s_code_core::budget::BudgetDecision::Deny {
                            resource: "budget_guard_callback".to_string(),
                            reason: format!("budget guard callback failed: {error}"),
                        }
                    }
                };
                let _ = tx.send(decision);
                Ok(())
            },
        );
        if status != napi::Status::Ok {
            return a3s_code_core::budget::BudgetDecision::Deny {
                resource: "budget_guard_unavailable".to_string(),
                reason: format!("budget guard callback could not be queued: {status:?}"),
            };
        }
        // FAIL-CLOSED on timeout: a stuck guard must DENY, never silently
        // disable budget enforcement.
        tokio::task::block_in_place(|| {
            rx.recv_timeout(std::time::Duration::from_millis(self.timeout_ms))
                .unwrap_or_else(|_| a3s_code_core::budget::BudgetDecision::Deny {
                    resource: "budget_guard_timeout".to_string(),
                    reason: format!("budget guard did not respond within {}ms", self.timeout_ms),
                })
        })
    }
}

#[async_trait::async_trait]
impl a3s_code_core::budget::BudgetGuard for NodeBudgetGuard {
    async fn check_before_llm(
        &self,
        session_id: &str,
        estimated_prompt_tokens: usize,
    ) -> a3s_code_core::budget::BudgetDecision {
        let Some(tsfn) = self.check_before_llm.as_ref() else {
            return a3s_code_core::budget::BudgetDecision::Allow;
        };
        self.call_decision(
            tsfn,
            serde_json::json!({
                "sessionId": session_id,
                "estimatedTokens": estimated_prompt_tokens,
            }),
        )
    }

    async fn record_after_llm(&self, session_id: &str, usage: &a3s_code_core::llm::TokenUsage) {
        let Some(tsfn) = self.record_after_llm.as_ref() else {
            return;
        };
        let _ = tsfn.call_with_return_value(
            serde_json::json!({
                "sessionId": session_id,
                "usage": {
                    "promptTokens": usage.prompt_tokens,
                    "completionTokens": usage.completion_tokens,
                    "totalTokens": usage.total_tokens,
                    "cacheReadTokens": usage.cache_read_tokens,
                    "cacheWriteTokens": usage.cache_write_tokens,
                },
            }),
            napi::threadsafe_function::ThreadsafeFunctionCallMode::NonBlocking,
            move |ret: napi::JsUnknown| {
                // Recording is observational. Decode the envelope so neither
                // a user exception nor a malformed return reaches napi-rs,
                // then safely ignore failures.
                let _ = decode_callback_outcome(ret);
                Ok(())
            },
        );
    }

    async fn check_before_tool(
        &self,
        session_id: &str,
        tool_name: &str,
    ) -> a3s_code_core::budget::BudgetDecision {
        let Some(tsfn) = self.check_before_tool.as_ref() else {
            return a3s_code_core::budget::BudgetDecision::Allow;
        };
        self.call_decision(
            tsfn,
            serde_json::json!({ "sessionId": session_id, "toolName": tool_name }),
        )
    }
}

/// Parse the return value of a JS BudgetGuard callback into a
/// [`BudgetDecision`](a3s_code_core::budget::BudgetDecision).
///
/// Accepted JS shapes mirror Python's:
/// - `null` / `undefined` / `{ decision: 'allow' }`                                                 → Allow
/// - `{ decision: 'soft', resource, consumed, limit, message? }`                                    → SoftLimit
/// - `{ decision: 'deny',  resource, reason }`                                                      → Deny
fn parse_js_budget_decision(
    val: napi::JsUnknown,
) -> napi::Result<a3s_code_core::budget::BudgetDecision> {
    use a3s_code_core::budget::BudgetDecision;
    use napi::{JsObject, ValueType};

    match val.get_type()? {
        ValueType::Null | ValueType::Undefined => Ok(BudgetDecision::Allow),
        ValueType::Object => {
            let obj = unsafe { val.cast::<JsObject>() };
            let required_string = |name: &str| -> napi::Result<String> {
                obj.get_named_property::<napi::JsString>(name)?
                    .into_utf8()?
                    .into_owned()
            };
            let required_number = |name: &str| -> napi::Result<f64> {
                let value = obj
                    .get_named_property::<napi::JsNumber>(name)?
                    .get_double()?;
                if value.is_finite() {
                    Ok(value)
                } else {
                    Err(napi::Error::from_reason(format!(
                        "budget decision '{name}' must be finite"
                    )))
                }
            };

            let decision = required_string("decision")?;
            match decision.as_str() {
                "allow" => Ok(BudgetDecision::Allow),
                "deny" => Ok(BudgetDecision::Deny {
                    resource: required_string("resource")?,
                    reason: required_string("reason")?,
                }),
                "soft" => {
                    let resource = required_string("resource")?;
                    let consumed = required_number("consumed")?;
                    let limit = required_number("limit")?;
                    let message = if obj.has_named_property("message")? {
                        Some(required_string("message")?)
                    } else {
                        None
                    };
                    Ok(BudgetDecision::SoftLimit {
                        resource,
                        consumed,
                        limit,
                        message,
                    })
                }
                _ => Err(napi::Error::from_reason(format!(
                    "unknown budget decision '{decision}'"
                ))),
            }
        }
        _ => Err(napi::Error::from_reason(
            "budget guard must return null, undefined, or a decision object",
        )),
    }
}
