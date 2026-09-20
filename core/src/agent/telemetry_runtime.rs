use super::AgentLoop;

impl AgentLoop {
    /// Track a tool call result in the current telemetry span.
    pub(super) fn track_tool_result(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        exit_code: i32,
    ) {
        record_tool_result_metadata(tool_name, args, exit_code);
    }
}

fn record_tool_result_metadata(tool_name: &str, args: &serde_json::Value, exit_code: i32) {
    let args_bytes = serde_json::to_string(args)
        .map(|s| s.len() as u64)
        .unwrap_or(0);
    // The span carries the tool name and sizes only. Argument values stay off
    // the exported attributes so a collector cannot see secrets from the call.
    let span = tracing::info_span!(
        crate::telemetry::SPAN_TOOL_EXECUTE,
        tool_name = tool_name,
        exit_code = exit_code,
        success = exit_code == 0,
        args_bytes = args_bytes,
    );
    let _entered = span.enter();
    span.record(crate::telemetry::ATTR_TOOL_NAME, tool_name);
    span.record(crate::telemetry::ATTR_TOOL_EXIT_CODE, exit_code as i64);
    span.record(crate::telemetry::ATTR_TOOL_SUCCESS, exit_code == 0);

    tracing::debug!(
        a3s.tool.name = tool_name,
        a3s.tool.exit_code = exit_code,
        a3s.tool.success = exit_code == 0,
        a3s.tool.args_bytes = args_bytes,
        "Tool execution result recorded"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_tool_result_metadata_does_not_panic_without_active_span() {
        record_tool_result_metadata("read", &json!({"path": "src/lib.rs"}), 0);
        record_tool_result_metadata("bash", &json!({"cmd": "false"}), 1);
    }
}
