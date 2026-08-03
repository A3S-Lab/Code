use super::*;

pub(super) fn py_parse_hook_event_type(event_type: &str) -> PyResult<RustHookEventType> {
    match event_type {
        "pre_tool_use" => Ok(RustHookEventType::PreToolUse),
        "post_tool_use" => Ok(RustHookEventType::PostToolUse),
        "generate_start" => Ok(RustHookEventType::GenerateStart),
        "generate_end" => Ok(RustHookEventType::GenerateEnd),
        "session_start" => Ok(RustHookEventType::SessionStart),
        "session_end" => Ok(RustHookEventType::SessionEnd),
        "skill_load" => Ok(RustHookEventType::SkillLoad),
        "skill_unload" => Ok(RustHookEventType::SkillUnload),
        "pre_prompt" => Ok(RustHookEventType::PrePrompt),
        "post_response" => Ok(RustHookEventType::PostResponse),
        "on_error" => Ok(RustHookEventType::OnError),
        // Harness control points
        "pre_context_perception" => Ok(RustHookEventType::PreContextPerception),
        "post_context_perception" => Ok(RustHookEventType::PostContextPerception),
        "on_success" => Ok(RustHookEventType::OnSuccess),
        "pre_memory_recall" => Ok(RustHookEventType::PreMemoryRecall),
        "post_memory_recall" => Ok(RustHookEventType::PostMemoryRecall),
        "pre_planning" => Ok(RustHookEventType::PrePlanning),
        "post_planning" => Ok(RustHookEventType::PostPlanning),
        "pre_reasoning" => Ok(RustHookEventType::PreReasoning),
        "post_reasoning" => Ok(RustHookEventType::PostReasoning),
        "on_rate_limit" => Ok(RustHookEventType::OnRateLimit),
        "on_confirmation" => Ok(RustHookEventType::OnConfirmation),
        _ => Err(PyValueError::new_err(format!(
            "Invalid hook event type: '{}'. Expected one of: pre_tool_use, post_tool_use, \
             generate_start, generate_end, session_start, session_end, skill_load, \
             skill_unload, pre_prompt, post_response, on_error, pre_context_perception, \
             post_context_perception, on_success, pre_memory_recall, post_memory_recall, \
             pre_planning, post_planning, pre_reasoning, post_reasoning, on_rate_limit, \
             on_confirmation",
            event_type
        ))),
    }
}

// ============================================================================
// PythonCallbackHandler — bridges Python callables into the Rust HookHandler trait
// ============================================================================

/// Wraps a Python callable so it can be used as a `HookHandler`.
///
/// The callable receives a dict (the serialized `HookEvent`) and must return
/// `None` / `{"action": "continue"}` to allow execution, or
/// `{"action": "block", "reason": "..."}` to cancel it.
///
/// GIL safety: `send()` and `stream()` both release the GIL via `py.allow_threads()`,
/// so acquiring it here from a tokio worker thread does not deadlock.
pub(super) struct PythonCallbackHandler {
    pub(super) callback: pyo3::Py<pyo3::PyAny>,
}

impl RustHookHandler for PythonCallbackHandler {
    fn handle(&self, event: &RustHookEvent) -> RustHookResponse {
        self.try_handle(event)
            .unwrap_or_else(|_| RustHookResponse::continue_())
    }

    fn try_handle(&self, event: &RustHookEvent) -> Result<RustHookResponse, String> {
        let json_str = serde_json::to_string(event)
            .map_err(|_| "Python hook event serialization failed".to_string())?;

        pyo3::Python::with_gil(|py| {
            // Deserialize the event into a Python dict via json.loads.
            let result = (|| -> pyo3::PyResult<RustHookResponse> {
                let json_mod = py.import("json")?;
                let event_dict = json_mod.call_method1("loads", (json_str.as_str(),))?;
                let ret = self.callback.call1(py, (event_dict,))?;
                parse_py_hook_response(py, ret.bind(py))
            })();

            result.map_err(|_| "Python hook callback failed".to_string())
        })
    }
}

/// Parse the return value of a Python hook callback into a `HookResponse`.
///
/// Accepted shapes:
/// - `None`                                   → continue
/// - `{"action": "continue"}`                 → continue
/// - `{"action": "block", "reason": "…"}`     → block
/// - `{"action": "skip"}`                     → skip
/// - `{"action": "retry", "delay_ms": N}`     → retry
/// Retry dictionaries may include `reason`; the bridge preserves it with the delay.
pub(super) fn parse_py_hook_response(
    py: pyo3::Python,
    val: &pyo3::Bound<pyo3::PyAny>,
) -> pyo3::PyResult<RustHookResponse> {
    use pyo3::types::PyDict;

    if val.is_none() {
        return Ok(RustHookResponse::continue_());
    }

    let dict = val
        .downcast::<PyDict>()
        .map_err(|_| PyTypeError::new_err("hook callback must return None or a response dict"))?;
    let action = dict
        .get_item("action")?
        .ok_or_else(|| PyValueError::new_err("hook response dict requires an 'action' field"))?
        .extract::<String>()?;

    match action.as_str() {
        "continue" => {
            if let Some(modified) = dict.get_item("modified")? {
                if !modified.is_none() {
                    return Ok(RustHookResponse::continue_with(py_to_json_value(
                        py, &modified,
                    )?));
                }
            }
            Ok(RustHookResponse::continue_())
        }
        "block" => {
            let reason = dict
                .get_item("reason")?
                .and_then(|v| v.extract::<String>().ok())
                .unwrap_or_else(|| "Blocked by hook".to_string());
            Ok(RustHookResponse::block(reason))
        }
        "skip" => Ok(RustHookResponse::skip()),
        "retry" => {
            let reason = dict
                .get_item("reason")?
                .and_then(|v| v.extract::<String>().ok());
            let delay_ms = dict
                .get_item("delay_ms")?
                .and_then(|v| v.extract::<u64>().ok())
                .unwrap_or(1000);
            Ok(match reason {
                Some(reason) => RustHookResponse::retry_with_reason(reason, delay_ms),
                None => RustHookResponse::retry(delay_ms),
            })
        }
        other => Err(PyValueError::new_err(format!(
            "unsupported hook action '{other}'"
        ))),
    }
}
