use super::*;

fn py_dumps(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<String> {
    let json_mod = py.import("json")?;
    json_mod.call_method1("dumps", (obj,))?.extract()
}

/// Convert a Python spec dict into an `AgentStepSpec` (snake_case keys) via a
/// JSON round-trip.
pub(super) fn py_to_step_spec(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
) -> PyResult<RustAgentStepSpec> {
    serde_json::from_str(&py_dumps(py, obj)?)
        .map_err(|e| PyValueError::new_err(format!("invalid AgentStepSpec: {e}")))
}

/// Convert an arbitrary Python value into a `serde_json::Value`.
pub(super) fn py_to_json_value(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
) -> PyResult<serde_json::Value> {
    serde_json::from_str(&py_dumps(py, obj)?)
        .map_err(|e| PyValueError::new_err(format!("invalid JSON: {e}")))
}

/// Convert a `StepOutcome` into a Python dict.
pub(super) fn step_outcome_to_py(py: Python<'_>, outcome: &RustStepOutcome) -> PyResult<PyObject> {
    let json = serde_json::to_string(outcome)
        .map_err(|e| PyRuntimeError::new_err(format!("serialize outcome: {e}")))?;
    json_string_to_py(py, &json)
}

/// Bridges a Python pipeline-stage callable into a synchronous `PipelineStage`.
///
/// GIL safety: `pipeline()` releases the GIL via `py.allow_threads`, so
/// re-acquiring it here from a tokio worker thread does not deadlock (same as
/// the hook/budget bridges). A raised exception is caught and treated as
/// `None` (stop the chain).
pub(super) struct PythonPipelineStage {
    pub(super) callback: pyo3::Py<pyo3::PyAny>,
}

impl PythonPipelineStage {
    pub(super) fn invoke(
        &self,
        prev: Option<&RustStepOutcome>,
        item: &serde_json::Value,
    ) -> Option<RustAgentStepSpec> {
        pyo3::Python::with_gil(|py| {
            let result = (|| -> PyResult<Option<RustAgentStepSpec>> {
                let json_mod = py.import("json")?;
                let previous = match prev {
                    Some(o) => {
                        let s = serde_json::to_string(o)
                            .map_err(|e| PyValueError::new_err(e.to_string()))?;
                        json_mod.call_method1("loads", (s,))?
                    }
                    None => py.None().into_bound(py),
                };
                let item_str = serde_json::to_string(item)
                    .map_err(|e| PyValueError::new_err(e.to_string()))?;
                let item_py = json_mod.call_method1("loads", (item_str,))?;
                let ctx = PyDict::new(py);
                ctx.set_item("previous", previous)?;
                ctx.set_item("item", item_py)?;
                let ret = self.callback.call1(py, (ctx,))?;
                let bound = ret.bind(py);
                if bound.is_none() {
                    return Ok(None);
                }
                let spec_json: String = json_mod.call_method1("dumps", (bound,))?.extract()?;
                serde_json::from_str::<RustAgentStepSpec>(&spec_json)
                    .map(Some)
                    .map_err(|e| PyValueError::new_err(format!("invalid step spec: {e}")))
            })();
            // Fail-closed: any exception → stop this chain.
            result.unwrap_or(None)
        })
    }
}

// ============================================================================
// Python BudgetGuard bridge
// ============================================================================

/// Bridges a Python BudgetGuard instance into the Rust async
/// [`a3s_code_core::budget::BudgetGuard`] trait.
///
/// Looks up `check_before_llm`, `record_after_llm`, and
/// `check_before_tool` on the held `PyObject` at call time, so the
/// user's Python class only needs to define the methods it cares
/// about — missing methods are treated as a permissive default
/// (Allow / no-op).
///
/// Calls into Python acquire the GIL via `Python::with_gil`, which
/// blocks the tokio worker thread briefly. Acceptable here because
/// `BudgetGuard` is called at most once per LLM turn / tool call,
/// not on a hot path.
///
/// RE-ENTRANCY WARNING: do **not** call session/agent APIs (or any
/// blocking Rust path) from inside a Python budget-guard callback. The
/// tokio worker thread is already blocked acquiring the GIL to run the
/// callback; re-entering the runtime from there risks a deadlock or
/// re-entrancy panic. Budget guards should be pure policy — inspect the
/// args, consult host-side counters, return a decision.
pub(super) struct PyBudgetGuard {
    inner: pyo3::Py<pyo3::PyAny>,
}

impl PyBudgetGuard {
    pub(super) fn new(inner: pyo3::Py<pyo3::PyAny>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl a3s_code_core::budget::BudgetGuard for PyBudgetGuard {
    async fn check_before_llm(
        &self,
        session_id: &str,
        estimated_prompt_tokens: usize,
    ) -> a3s_code_core::budget::BudgetDecision {
        pyo3::Python::with_gil(|py| {
            let inner = self.inner.bind(py);
            let method = match inner.getattr("check_before_llm") {
                Ok(m) if !m.is_none() => m,
                _ => return a3s_code_core::budget::BudgetDecision::Allow,
            };
            match method.call1((session_id, estimated_prompt_tokens)) {
                Ok(val) => parse_py_budget_decision(&val),
                Err(e) => {
                    eprintln!(
                        "[a3s-code] warning: Python BudgetGuard.check_before_llm raised: {e}; defaulting to Allow"
                    );
                    a3s_code_core::budget::BudgetDecision::Allow
                }
            }
        })
    }

    async fn record_after_llm(&self, session_id: &str, usage: &a3s_code_core::llm::TokenUsage) {
        pyo3::Python::with_gil(|py| {
            let inner = self.inner.bind(py);
            let method = match inner.getattr("record_after_llm") {
                Ok(m) if !m.is_none() => m,
                _ => return,
            };
            // Hand Python a dict so they don't have to construct a
            // TokenUsage type on their side.
            let usage_dict = pyo3::types::PyDict::new(py);
            let _ = usage_dict.set_item("prompt_tokens", usage.prompt_tokens);
            let _ = usage_dict.set_item("completion_tokens", usage.completion_tokens);
            let _ = usage_dict.set_item("total_tokens", usage.total_tokens);
            let _ = usage_dict.set_item("cache_read_tokens", usage.cache_read_tokens);
            let _ = usage_dict.set_item("cache_write_tokens", usage.cache_write_tokens);
            if let Err(e) = method.call1((session_id, usage_dict)) {
                eprintln!(
                    "[a3s-code] warning: Python BudgetGuard.record_after_llm raised: {e}; ignored"
                );
            }
        })
    }

    async fn check_before_tool(
        &self,
        session_id: &str,
        tool_name: &str,
    ) -> a3s_code_core::budget::BudgetDecision {
        pyo3::Python::with_gil(|py| {
            let inner = self.inner.bind(py);
            let method = match inner.getattr("check_before_tool") {
                Ok(m) if !m.is_none() => m,
                _ => return a3s_code_core::budget::BudgetDecision::Allow,
            };
            match method.call1((session_id, tool_name)) {
                Ok(val) => parse_py_budget_decision(&val),
                Err(e) => {
                    eprintln!(
                        "[a3s-code] warning: Python BudgetGuard.check_before_tool raised: {e}; defaulting to Allow"
                    );
                    a3s_code_core::budget::BudgetDecision::Allow
                }
            }
        })
    }
}

/// Parse the return value of a Python BudgetGuard method into a
/// [`BudgetDecision`](a3s_code_core::budget::BudgetDecision).
///
/// Accepted shapes:
/// - `None`                                                        → Allow
/// - `{"decision": "allow"}`                                       → Allow
/// - `{"decision": "soft", "resource": str, "consumed": float,
///     "limit": float, "message"?: str}`                           → SoftLimit
/// - `{"decision": "deny", "resource": str, "reason": str}`        → Deny
fn parse_py_budget_decision(
    val: &pyo3::Bound<pyo3::PyAny>,
) -> a3s_code_core::budget::BudgetDecision {
    use a3s_code_core::budget::BudgetDecision;
    use pyo3::types::PyDict;

    if val.is_none() {
        return BudgetDecision::Allow;
    }

    let Ok(dict) = val.downcast::<PyDict>() else {
        return BudgetDecision::Allow;
    };

    let decision = dict
        .get_item("decision")
        .ok()
        .flatten()
        .and_then(|v| v.extract::<String>().ok())
        .unwrap_or_else(|| "allow".to_string());

    match decision.as_str() {
        "deny" => {
            let resource = dict
                .get_item("resource")
                .ok()
                .flatten()
                .and_then(|v| v.extract::<String>().ok())
                .unwrap_or_else(|| "unspecified".to_string());
            let reason = dict
                .get_item("reason")
                .ok()
                .flatten()
                .and_then(|v| v.extract::<String>().ok())
                .unwrap_or_else(|| "denied by host".to_string());
            BudgetDecision::Deny { resource, reason }
        }
        "soft" => {
            let resource = dict
                .get_item("resource")
                .ok()
                .flatten()
                .and_then(|v| v.extract::<String>().ok())
                .unwrap_or_else(|| "unspecified".to_string());
            let consumed = dict
                .get_item("consumed")
                .ok()
                .flatten()
                .and_then(|v| v.extract::<f64>().ok())
                .unwrap_or(0.0);
            let limit = dict
                .get_item("limit")
                .ok()
                .flatten()
                .and_then(|v| v.extract::<f64>().ok())
                .unwrap_or(0.0);
            let message = dict
                .get_item("message")
                .ok()
                .flatten()
                .and_then(|v| v.extract::<String>().ok());
            BudgetDecision::SoftLimit {
                resource,
                consumed,
                limit,
                message,
            }
        }
        _ => BudgetDecision::Allow,
    }
}
