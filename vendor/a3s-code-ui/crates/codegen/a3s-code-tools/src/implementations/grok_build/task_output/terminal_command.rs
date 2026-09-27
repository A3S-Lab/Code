//! Workspace-only, subagent-free variant of `get_task_output` (delegates to [`TaskOutputTool`]).

use super::{TaskOutputTool, background_bash_requires_exprs};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::resources::{Params, ResourceType};
use crate::types::tool::{ToolKind, ToolNamespace};
use a3s_tool_types::{TaskOutputOutput, TaskOutputToolInput};

/// Session config for `get_terminal_command_output`. `output_byte_limit` is the
/// builtin dump cap, the same role as bash `BashParams::output_byte_limit`.
/// `None` keeps the shared `get_command_or_subagent_output` truncation lookup.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TerminalCommandOutputParams {
    #[serde(default)]
    pub output_byte_limit: Option<usize>,
}

impl ResourceType for TerminalCommandOutputParams {
    const ID: &'static str = "grok_build.GetTerminalCommandOutput";
}

fn terminal_command_output_requires_expr() -> Expr<ToolRequirement> {
    Expr::Or(background_bash_requires_exprs())
}

#[derive(Debug, Default)]
pub struct GetTerminalCommandOutputTool;

impl crate::types::tool_metadata::ToolMetadata for GetTerminalCommandOutputTool {
    fn kind(&self) -> ToolKind {
        ToolKind::BackgroundTaskAction
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        // `{max_wait_ms}` is resolved per session by the finalize loop's
        // `TruncationConfig::interpolate_description`, like `{max_lines_read}`:
        // the cap is client-configurable, so it cannot be baked in here.
        r#"Get output and status from a background terminal command${%- if tools.by_kind.monitor %} or monitor${%- endif %}.

Usage notes:
- Pass ${{ params.background_task_action.task_ids }} with one or more ids from ${%- if params is defined and params.execute is defined and params.execute.is_background %} ${{ params.execute.is_background }}=true commands${%- else %} background commands${%- endif %}${%- if tools.by_kind.monitor %} (a monitor's ${{ params.kill_task_action.task_id }} is returned by ${{ tools.by_kind.monitor }})${%- endif %}; for a single task use a one-element array. Multiple ids with a positive ${{ params.background_task_action.timeout_ms }} wait until all complete
- Omit ${{ params.background_task_action.timeout_ms }} or pass 0 for a non-blocking status snapshot; set a positive ${{ params.background_task_action.timeout_ms }} to wait up to that many milliseconds, capped at {max_wait_ms}
- Returns current output, status, and exit code if completed${%- if tools.by_kind.read %}
- If output is large, use ${{ tools.by_kind.read }} on the output_file path${%- endif %}"#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        terminal_command_output_requires_expr()
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl a3s_tool_runtime::Tool for GetTerminalCommandOutputTool {
    type Args = TaskOutputToolInput;
    type Output = TaskOutputOutput;

    fn id(&self) -> a3s_tool_protocol::ToolId {
        a3s_tool_protocol::ToolId::new("get_terminal_command_output").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::a3s_tool_runtime::ListToolsContext,
    ) -> a3s_tool_types::ToolDescription {
        a3s_tool_types::ToolDescription::new(
            "get_terminal_command_output",
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> a3s_tool_protocol::ToolCapabilities {
        a3s_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(a3s_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.get_terminal_command_output",
        skip_all,
        fields(waits = %input.waits())
    )]
    async fn run(
        &self,
        ctx: a3s_tool_runtime::ToolCallContext,
        input: TaskOutputToolInput,
    ) -> Result<TaskOutputOutput, a3s_tool_runtime::ToolError> {
        let resources = crate::types::tool_metadata::shared_resources(&ctx)?;
        let output_byte_limit = resources
            .lock()
            .await
            .get::<Params<TerminalCommandOutputParams>>()
            .and_then(|params| params.output_byte_limit);
        TaskOutputTool
            .run_with_output_byte_limit(ctx, input, output_byte_limit)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implementations::grok_build::task_output::test_helpers::{
        make_snapshot, resources_with_terminal,
    };
    use crate::types::tool_metadata::ToolMetadata;
    use crate::types::tool_metadata::test_ctx;

    #[test]
    fn tool_name_and_description_are_subagent_free() {
        let tool = GetTerminalCommandOutputTool;
        assert_eq!(
            a3s_tool_runtime::Tool::id(&tool).as_str(),
            "get_terminal_command_output"
        );
        let tmpl = ToolMetadata::description_template(&tool);
        assert!(
            !tmpl.to_lowercase().contains("subagent"),
            "workspace tool must not mention subagents: {tmpl}"
        );
    }

    #[test]
    fn is_read_only() {
        let tool = GetTerminalCommandOutputTool;
        assert!(ToolMetadata::is_read_only(&tool));
    }

    #[tokio::test]
    async fn delegates_to_task_output_for_running_task() {
        let snapshot = make_snapshot("tc-1", false, None);
        let resources = resources_with_terminal(Some(snapshot));
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-1".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.task_id, "tc-1");
                assert_eq!(r.status, "running");
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delegates_to_task_output_for_completed_task() {
        let snapshot = make_snapshot("tc-2", true, Some(0));
        let resources = resources_with_terminal(Some(snapshot));
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-2".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.status, "completed");
                assert_eq!(r.exit_code, Some(0));
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    fn with_poll_cap(
        snapshot: crate::computer::types::TaskSnapshot,
    ) -> crate::types::resources::Resources {
        use crate::types::context::TruncationConfig;
        use crate::types::resources::TruncationCfg;

        let mut resources = resources_with_terminal(Some(snapshot));
        let mut trunc = TruncationConfig::default();
        trunc
            .per_tool_max_output_bytes
            .insert("get_command_or_subagent_output".to_string(), 5_000);
        resources.insert(TruncationCfg(trunc));
        resources
    }

    #[tokio::test]
    async fn truncation_config_caps_the_dump_and_names_the_log() {
        let mut snapshot = make_snapshot("tc-big", true, Some(0));
        snapshot.output = "x".repeat(8_000);
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(with_poll_cap(snapshot).into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-big".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(r.truncated, "8k dump must be marked truncated");
                assert!(
                    r.output.len() <= 5_000,
                    "capped output is {} bytes",
                    r.output.len()
                );
                assert!(r.output.contains("[Output truncated"));
                assert!(
                    r.output
                        .contains("Use read_file on /tmp/tc-big.log for full content"),
                    "footer: {}",
                    r.output
                );
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn truncation_config_keeps_the_running_wait_hint() {
        let mut snapshot = make_snapshot("tc-run", false, None);
        snapshot.output = "x".repeat(8_000);
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(with_poll_cap(snapshot).into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-run".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(r.truncated);
                let footer = r
                    .output
                    .find("Use read_file on /tmp/tc-run.log for full content")
                    .expect("footer missing");
                let hint = r
                    .output
                    .find("do not kill this task")
                    .expect("wait hint missing");
                assert!(
                    footer < hint,
                    "wait hint must follow the truncated dump: {}",
                    r.output
                );
                assert!(r.output.contains("It is still working."));
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn truncation_config_reports_the_real_task_size() {
        let mut snapshot = make_snapshot("tc-size", true, Some(0));
        snapshot.output = "y".repeat(8_000);
        snapshot.output_total_bytes = 5_000_000;
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(with_poll_cap(snapshot).into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-size".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(
                    r.output.contains("5000000 bytes total"),
                    "footer should report the task total, not the preview: {}",
                    r.output
                );
                assert!(
                    r.output
                        .contains("Use read_file on /tmp/tc-size.log for full content"),
                    "footer: {}",
                    r.output
                );
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multi_result_prompt_includes_read_tool_footer() {
        use crate::types::output::ToolOutput;

        let mut snapshot = make_snapshot("tc-multi", false, None);
        snapshot.output = "z".repeat(8_000);
        snapshot.output_total_bytes = 5_000_000;
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(with_poll_cap(snapshot).into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-multi".into(), "tc-other".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        let rendered = ToolOutput::TaskOutput(result).to_prompt_format();
        assert!(
            rendered.contains("Use read_file on /tmp/tc-multi.log for full content"),
            "multi prompt dropped the read-tool pointer: {rendered}"
        );
        assert!(
            rendered.contains("5000000 bytes total"),
            "multi prompt dropped the real size: {rendered}"
        );
        assert!(
            rendered.contains("do not kill this task"),
            "multi prompt dropped the wait hint: {rendered}"
        );
    }

    #[tokio::test]
    async fn output_byte_limit_caps_the_dump() {
        let mut snapshot = make_snapshot("tc-param", true, Some(0));
        snapshot.output = "x".repeat(8_000);
        let mut resources = resources_with_terminal(Some(snapshot));
        resources.insert(Params(TerminalCommandOutputParams {
            output_byte_limit: Some(5_000),
        }));
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-param".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(r.truncated, "a set output_byte_limit must cap an 8k dump");
                assert!(
                    r.output.len() <= 5_000,
                    "capped output is {} bytes",
                    r.output.len()
                );
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn truncation_config_overrides_output_byte_limit() {
        use crate::types::context::TruncationConfig;
        use crate::types::resources::TruncationCfg;

        let mut snapshot = make_snapshot("tc-override", true, Some(0));
        snapshot.output = "x".repeat(8_000);
        let mut resources = resources_with_terminal(Some(snapshot));
        resources.insert(Params(TerminalCommandOutputParams {
            output_byte_limit: Some(20_000),
        }));
        let mut trunc = TruncationConfig::default();
        trunc
            .per_tool_max_output_bytes
            .insert("get_terminal_command_output".to_string(), 5_000);
        resources.insert(TruncationCfg(trunc));
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-override".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(
                    r.truncated,
                    "per-tool TruncationConfig must beat output_byte_limit"
                );
                assert!(
                    r.output.len() <= 5_000,
                    "capped output is {} bytes",
                    r.output.len()
                );
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_truncation_config_keeps_an_8k_dump() {
        let mut snapshot = make_snapshot("tc-uncapped", true, Some(0));
        snapshot.output = "x".repeat(8_000);
        let resources = resources_with_terminal(Some(snapshot));
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-uncapped".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(!r.truncated, "8k is under the 40k fallback");
                assert!(r.output.len() > 5_000, "dump was capped without a param");
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn short_output_is_unchanged() {
        let snapshot = make_snapshot("tc-short", true, Some(0));
        let expected = snapshot.output.clone();
        let resources = resources_with_terminal(Some(snapshot));
        let result = a3s_tool_runtime::Tool::run(
            &GetTerminalCommandOutputTool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["tc-short".into()],
                timeout_ms: None,
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(!r.truncated);
                assert_eq!(r.output, expected);
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }
}
