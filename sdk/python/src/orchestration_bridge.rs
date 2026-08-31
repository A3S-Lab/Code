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

pub(super) const DEFAULT_BUDGET_GUARD_TIMEOUT_MS: u64 = 5_000;

enum PyBudgetCheck {
    Llm {
        session_id: String,
        estimated_prompt_tokens: usize,
    },
    Tool {
        session_id: String,
        tool_name: String,
    },
}

impl PyBudgetCheck {
    fn method_name(&self) -> &'static str {
        match self {
            Self::Llm { .. } => "check_before_llm",
            Self::Tool { .. } => "check_before_tool",
        }
    }
}

fn budget_guard_deny(
    resource: &str,
    reason: impl Into<String>,
) -> a3s_code_core::budget::BudgetDecision {
    a3s_code_core::budget::BudgetDecision::Deny {
        resource: resource.to_string(),
        reason: reason.into(),
    }
}

/// Bridges a Python BudgetGuard instance into the Rust async
/// [`a3s_code_core::budget::BudgetGuard`] trait.
///
/// Looks up `check_before_llm`, `record_after_llm`, and
/// `check_before_tool` on the held `PyObject` at call time, so the
/// user's Python class only needs to define the methods it cares
/// about — missing methods are treated as a permissive default
/// (Allow / no-op).
///
/// Check callbacks run on Tokio's blocking pool and are bounded by a timeout.
/// Exceptions, malformed decisions, join failures, and timeouts all fail closed
/// with `BudgetDecision::Deny`; a broken policy callback must never disable its
/// own enforcement. Recording remains observational and ignores failures.
///
/// RE-ENTRANCY WARNING: do **not** call session/agent APIs (or any
/// blocking Rust path) from inside a Python budget-guard callback. The
/// tokio worker thread is already blocked acquiring the GIL to run the
/// callback; re-entering the runtime from there risks a deadlock or
/// re-entrancy panic. Budget guards should be pure policy — inspect the
/// args, consult host-side counters, return a decision.
pub(super) struct PyBudgetGuard {
    inner: pyo3::Py<pyo3::PyAny>,
    timeout_ms: u64,
}

impl PyBudgetGuard {
    pub(super) fn new(inner: pyo3::Py<pyo3::PyAny>, timeout_ms: u64) -> Self {
        Self { inner, timeout_ms }
    }

    async fn check(&self, call: PyBudgetCheck) -> a3s_code_core::budget::BudgetDecision {
        let method_name = call.method_name();
        let timeout_ms = self.timeout_ms;
        let inner = pyo3::Python::with_gil(|py| self.inner.clone_ref(py));
        let task = tokio::task::spawn_blocking(move || {
            pyo3::Python::with_gil(|py| {
                let inner = inner.bind(py);
                let method = match inner.getattr(method_name) {
                    Ok(method) if !method.is_none() => method,
                    Ok(_) => return a3s_code_core::budget::BudgetDecision::Allow,
                    Err(error)
                        if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(py) =>
                    {
                        return a3s_code_core::budget::BudgetDecision::Allow;
                    }
                    Err(error) => {
                        return budget_guard_deny(
                            "budget_guard_callback",
                            format!("Python BudgetGuard.{method_name} lookup failed: {error}"),
                        );
                    }
                };

                let result = match call {
                    PyBudgetCheck::Llm {
                        session_id,
                        estimated_prompt_tokens,
                    } => method.call1((session_id, estimated_prompt_tokens)),
                    PyBudgetCheck::Tool {
                        session_id,
                        tool_name,
                    } => method.call1((session_id, tool_name)),
                };

                match result {
                    Ok(value) => parse_py_budget_decision(&value).unwrap_or_else(|error| {
                        budget_guard_deny(
                            "budget_guard_error",
                            format!("invalid Python BudgetGuard.{method_name} return: {error}"),
                        )
                    }),
                    Err(error) => budget_guard_deny(
                        "budget_guard_callback",
                        format!("Python BudgetGuard.{method_name} failed: {error}"),
                    ),
                }
            })
        });

        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), task).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(error)) => budget_guard_deny(
                "budget_guard_unavailable",
                format!("Python BudgetGuard.{method_name} worker failed: {error}"),
            ),
            Err(_) => budget_guard_deny(
                "budget_guard_timeout",
                format!("Python BudgetGuard.{method_name} did not respond within {timeout_ms}ms"),
            ),
        }
    }
}

#[async_trait::async_trait]
impl a3s_code_core::budget::BudgetGuard for PyBudgetGuard {
    async fn check_before_llm(
        &self,
        session_id: &str,
        estimated_prompt_tokens: usize,
    ) -> a3s_code_core::budget::BudgetDecision {
        self.check(PyBudgetCheck::Llm {
            session_id: session_id.to_string(),
            estimated_prompt_tokens,
        })
        .await
    }

    async fn record_after_llm(&self, session_id: &str, usage: &a3s_code_core::llm::TokenUsage) {
        let inner = pyo3::Python::with_gil(|py| self.inner.clone_ref(py));
        let session_id = session_id.to_string();
        let prompt_tokens = usage.prompt_tokens;
        let completion_tokens = usage.completion_tokens;
        let total_tokens = usage.total_tokens;
        let cache_read_tokens = usage.cache_read_tokens;
        let cache_write_tokens = usage.cache_write_tokens;
        let task = tokio::task::spawn_blocking(move || {
            pyo3::Python::with_gil(|py| -> PyResult<()> {
                let inner = inner.bind(py);
                let method = match inner.getattr("record_after_llm") {
                    Ok(method) if !method.is_none() => method,
                    Ok(_) => return Ok(()),
                    Err(error)
                        if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(py) =>
                    {
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                };
                let usage_dict = pyo3::types::PyDict::new(py);
                usage_dict.set_item("prompt_tokens", prompt_tokens)?;
                usage_dict.set_item("completion_tokens", completion_tokens)?;
                usage_dict.set_item("total_tokens", total_tokens)?;
                usage_dict.set_item("cache_read_tokens", cache_read_tokens)?;
                usage_dict.set_item("cache_write_tokens", cache_write_tokens)?;
                method.call1((session_id, usage_dict))?;
                Ok(())
            })
        });

        match tokio::time::timeout(std::time::Duration::from_millis(self.timeout_ms), task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => eprintln!(
                "[a3s-code] warning: Python BudgetGuard.record_after_llm failed: {error}; ignored"
            ),
            Ok(Err(error)) => eprintln!(
                "[a3s-code] warning: Python BudgetGuard.record_after_llm worker failed: {error}; ignored"
            ),
            Err(_) => eprintln!(
                "[a3s-code] warning: Python BudgetGuard.record_after_llm timed out after {}ms; ignored",
                self.timeout_ms
            ),
        }
    }

    async fn check_before_tool(
        &self,
        session_id: &str,
        tool_name: &str,
    ) -> a3s_code_core::budget::BudgetDecision {
        self.check(PyBudgetCheck::Tool {
            session_id: session_id.to_string(),
            tool_name: tool_name.to_string(),
        })
        .await
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
) -> Result<a3s_code_core::budget::BudgetDecision, String> {
    use a3s_code_core::budget::BudgetDecision;
    use pyo3::types::PyDict;

    if val.is_none() {
        return Ok(BudgetDecision::Allow);
    }

    let dict = val
        .downcast::<PyDict>()
        .map_err(|_| "expected None or a decision dict".to_string())?;
    let required_string = |name: &str| -> Result<String, String> {
        dict.get_item(name)
            .map_err(|error| format!("could not read '{name}': {error}"))?
            .ok_or_else(|| format!("missing required string '{name}'"))?
            .extract::<String>()
            .map_err(|error| format!("'{name}' must be a string: {error}"))
    };
    let required_number = |name: &str| -> Result<f64, String> {
        let value = dict
            .get_item(name)
            .map_err(|error| format!("could not read '{name}': {error}"))?
            .ok_or_else(|| format!("missing required number '{name}'"))?
            .extract::<f64>()
            .map_err(|error| format!("'{name}' must be a number: {error}"))?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(format!("'{name}' must be finite"))
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
            let message_value = dict
                .get_item("message")
                .map_err(|error| format!("could not read 'message': {error}"))?;
            let message = match message_value {
                Some(value) if !value.is_none() => Some(
                    value
                        .extract::<String>()
                        .map_err(|error| format!("'message' must be a string: {error}"))?,
                ),
                _ => None,
            };
            Ok(BudgetDecision::SoftLimit {
                resource: required_string("resource")?,
                consumed: required_number("consumed")?,
                limit: required_number("limit")?,
                message,
            })
        }
        _ => Err(format!("unknown budget decision '{decision}'")),
    }
}
