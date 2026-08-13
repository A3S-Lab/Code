use super::*;

/// Matcher for filtering which events trigger a hook.
#[napi(object)]
#[derive(Clone)]
pub struct HookMatcherObject {
    /// Match specific tool name (exact match)
    pub tool: Option<String>,
    /// Match file path pattern (glob)
    pub path_pattern: Option<String>,
    /// Match command pattern (regex for Bash commands)
    pub command_pattern: Option<String>,
    /// Match session ID (exact match)
    pub session_id: Option<String>,
    /// Match skill name (supports glob patterns)
    pub skill: Option<String>,
}

/// Configuration for a hook.
#[napi(object)]
#[derive(Clone)]
pub struct HookConfigObject {
    /// Priority (lower values = higher priority, default: 100)
    pub priority: Option<i32>,
    /// Timeout in milliseconds (default: 30000)
    pub timeout_ms: Option<i64>,
    /// Whether to execute asynchronously (fire-and-forget)
    pub async_execution: Option<bool>,
    /// Maximum retry attempts
    pub max_retries: Option<u32>,
}

pub(super) fn metrics_snapshot_to_json(snapshot: Option<RustMetricsSnapshot>) -> serde_json::Value {
    let s = match snapshot {
        None => return serde_json::Value::Null,
        Some(s) => s,
    };
    let counters: serde_json::Map<String, serde_json::Value> = s
        .counters
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::Number(v.into())))
        .collect();
    let gauges: serde_json::Map<String, serde_json::Value> = s
        .gauges
        .into_iter()
        .map(|(k, v)| {
            let n = serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into());
            (k, serde_json::Value::Number(n))
        })
        .collect();
    let histograms: serde_json::Map<String, serde_json::Value> = s
        .histograms
        .into_iter()
        .map(|(k, h)| {
            let to_f = |v: f64| serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into());
            let (min, max) = if h.count == 0 {
                (0.into(), 0.into())
            } else {
                (to_f(h.min), to_f(h.max))
            };
            let v = serde_json::json!({
                "count": h.count,
                "sum": to_f(h.sum),
                "min": min,
                "max": max,
                "mean": to_f(h.mean),
                "p50": to_f(h.percentiles.p50),
                "p90": to_f(h.percentiles.p90),
                "p95": to_f(h.percentiles.p95),
                "p99": to_f(h.percentiles.p99),
            });
            (k, v)
        })
        .collect();
    serde_json::json!({
        "counters": serde_json::Value::Object(counters),
        "gauges": serde_json::Value::Object(gauges),
        "histograms": serde_json::Value::Object(histograms),
    })
}

pub(super) fn parse_hook_event_type(event_type: &str) -> napi::Result<RustHookEventType> {
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
        "intent_detection" => Ok(RustHookEventType::IntentDetection),
        _ => Err(napi::Error::from_reason(format!(
            "Invalid hook event type: '{}'. Expected one of: pre_tool_use, post_tool_use, \
             generate_start, generate_end, session_start, session_end, skill_load, \
             skill_unload, pre_prompt, post_response, on_error, pre_context_perception, \
             post_context_perception, on_success, pre_memory_recall, post_memory_recall, \
             pre_planning, post_planning, pre_reasoning, post_reasoning, on_rate_limit, \
             on_confirmation, intent_detection",
            event_type
        ))),
    }
}

// ============================================================================
// NodeCallbackHandler — bridges JS hook callbacks into the Rust HookHandler trait
// ============================================================================

pub(super) struct NodeCallbackHandler {
    pub(super) tsfn: napi::threadsafe_function::ThreadsafeFunction<
        serde_json::Value,
        napi::threadsafe_function::ErrorStrategy::Fatal,
    >,
    pub(super) timeout_ms: u64,
}

// SAFETY: ThreadsafeFunction is designed to be sent across threads.
unsafe impl Send for NodeCallbackHandler {}
unsafe impl Sync for NodeCallbackHandler {}

impl RustHookHandler for NodeCallbackHandler {
    fn handle(&self, event: &RustHookEvent) -> RustHookResponse {
        self.try_handle(event)
            .unwrap_or_else(|_| RustHookResponse::continue_())
    }

    fn try_handle(&self, event: &RustHookEvent) -> Result<RustHookResponse, String> {
        let event_json = serde_json::to_value(event)
            .map_err(|error| format!("failed to serialize hook event: {error}"))?;

        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<RustHookResponse, String>>(1);

        let status = self.tsfn.call_with_return_value(
            event_json,
            napi::threadsafe_function::ThreadsafeFunctionCallMode::NonBlocking,
            move |ret: napi::JsUnknown| {
                // This closure must itself remain infallible: returning a napi
                // error from TSFN return conversion triggers napi_fatal_error.
                let response = match decode_callback_outcome(ret) {
                    JsCallbackOutcome::Returned(value) => parse_js_hook_response(value)
                        .map_err(|error| format!("invalid hook callback return: {error}")),
                    JsCallbackOutcome::Failed(error) => {
                        Err(format!("hook callback failed: {error}"))
                    }
                };
                let _ = tx.send(response);
                Ok(())
            },
        );

        if status != napi::Status::Ok {
            return Err(format!("hook callback could not be queued: {status:?}"));
        }

        // HookEngine already runs handlers in spawn_blocking. A plain bounded
        // receive avoids nesting block_in_place and reports infrastructure
        // failure through try_handle so gating hooks fail securely.
        match rx.recv_timeout(std::time::Duration::from_millis(self.timeout_ms)) {
            Ok(response) => response,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "hook callback did not respond within {}ms",
                self.timeout_ms
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("hook callback channel disconnected".to_string())
            }
        }
    }
}

/// Parse the return value from a JS hook callback into a `HookResponse`.
///
/// Accepted JS return shapes:
/// - `null` / `undefined`              → continue
/// - `{ action: 'continue' }`          → continue
/// - `{ action: 'block', reason: '…' }` → block
/// - `{ action: 'skip' }`              → skip
/// - `{ action: 'retry', delayMs: N }` → retry after N ms
/// - `{ action: 'continue', modified: {...} }` → continue with modified data
///
/// Retry objects may include `reason`; the bridge preserves it with the delay.
fn parse_js_hook_response(val: napi::JsUnknown) -> napi::Result<RustHookResponse> {
    use napi::{JsObject, ValueType};

    match val.get_type()? {
        ValueType::Null | ValueType::Undefined => Ok(RustHookResponse::continue_()),
        ValueType::Object => {
            let obj = unsafe { val.cast::<JsObject>() };
            let string_property = |name: &str| -> napi::Result<String> {
                obj.get_named_property::<napi::JsString>(name)?
                    .into_utf8()?
                    .into_owned()
            };
            let action = string_property("action")?;

            match action.as_str() {
                "block" => {
                    let reason = if obj.has_named_property("reason")? {
                        string_property("reason")?
                    } else {
                        "Blocked by hook".to_string()
                    };
                    Ok(RustHookResponse::block(reason))
                }
                "skip" => Ok(RustHookResponse::skip()),
                "retry" => {
                    let delay_ms = if obj.has_named_property("delayMs")? {
                        obj.get_named_property::<napi::JsNumber>("delayMs")?
                            .get_uint32()? as u64
                    } else {
                        1000
                    };
                    if obj.has_named_property("reason")? {
                        Ok(RustHookResponse::retry_with_reason(
                            string_property("reason")?,
                            delay_ms,
                        ))
                    } else {
                        Ok(RustHookResponse::retry(delay_ms))
                    }
                }
                "continue" => {
                    if obj.has_named_property("modified")? {
                        let modified = obj.get_named_property::<napi::JsUnknown>("modified")?;
                        if !matches!(modified.get_type()?, ValueType::Null | ValueType::Undefined) {
                            let modified = js_unknown_to_json(modified)?;
                            return Ok(RustHookResponse::continue_with(modified));
                        }
                    }
                    Ok(RustHookResponse::continue_())
                }
                _ => Err(napi::Error::from_reason(format!(
                    "unknown hook action '{action}'"
                ))),
            }
        }
        _ => Err(napi::Error::from_reason(
            "hook callback must return null, undefined, or an action object",
        )),
    }
}

fn js_unknown_to_json(value: napi::JsUnknown) -> napi::Result<serde_json::Value> {
    use napi::{JsBoolean, JsNumber, JsObject, JsString, ValueType};

    match value.get_type()? {
        ValueType::Null | ValueType::Undefined => Ok(serde_json::Value::Null),
        ValueType::Boolean => {
            let value = unsafe { value.cast::<JsBoolean>() };
            Ok(serde_json::Value::Bool(value.get_value()?))
        }
        ValueType::Number => {
            let value = unsafe { value.cast::<JsNumber>() };
            let number = serde_json::Number::from_f64(value.get_double()?)
                .unwrap_or_else(|| serde_json::Number::from(0));
            Ok(serde_json::Value::Number(number))
        }
        ValueType::String => {
            let value = unsafe { value.cast::<JsString>() };
            Ok(serde_json::Value::String(value.into_utf8()?.into_owned()?))
        }
        ValueType::Object => {
            let object = unsafe { value.cast::<JsObject>() };
            if object.is_array()? {
                let mut values = Vec::new();
                for index in 0..object.get_array_length()? {
                    let item = object.get_element::<napi::JsUnknown>(index)?;
                    values.push(js_unknown_to_json(item)?);
                }
                return Ok(serde_json::Value::Array(values));
            }

            let names = object.get_property_names()?;
            let mut map = serde_json::Map::new();
            for index in 0..names.get_array_length()? {
                let key = names
                    .get_element::<JsString>(index)?
                    .into_utf8()?
                    .into_owned()?;
                let item = object.get_named_property::<napi::JsUnknown>(&key)?;
                map.insert(key, js_unknown_to_json(item)?);
            }
            Ok(serde_json::Value::Object(map))
        }
        _ => Ok(serde_json::Value::Null),
    }
}
