use crate::tools::{Tool, ToolContext, ToolExecutor, ToolOutput, MAX_OUTPUT_SIZE};
use async_trait::async_trait;
use std::sync::Arc;

/// A log whose needle sits past the retained head, so only the artifact
/// holds it.
struct LongLogTool;

#[async_trait]
impl Tool for LongLogTool {
    fn name(&self) -> &str {
        "long_log"
    }

    fn description(&self) -> &str {
        "Emit a long log"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false, "properties": {}})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let mut log = String::new();
        let mut line = 0usize;
        while log.len() <= MAX_OUTPUT_SIZE * 2 {
            line += 1;
            log.push_str(&format!("step {line:06} ok padding-padding-padding\n"));
            if line == 4_000 {
                log.push_str("NEEDLE: linker failed on symbol foo\n");
            }
        }
        Ok(ToolOutput::success(log))
    }
}

fn artifact_uri(output: &str) -> String {
    let start = output
        .rfind(crate::tools::FULL_OUTPUT_ARTIFACT_MARKER)
        .expect("artifact marker")
        + crate::tools::FULL_OUTPUT_ARTIFACT_MARKER.len();
    output[start..]
        .split(|c: char| c.is_whitespace() || c == ']')
        .next()
        .expect("artifact uri")
        .to_string()
}

#[tokio::test]
async fn the_model_reads_and_greps_the_region_a_truncated_result_omitted() {
    let workspace = tempfile::tempdir().expect("workspace");
    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let registry = executor.registry();
    registry.register(Arc::new(LongLogTool));

    let truncated = registry
        .execute("long_log", &serde_json::json!({}))
        .await
        .expect("long_log");
    assert!(
        !truncated.output.contains("NEEDLE"),
        "needle must be omitted"
    );
    assert!(
        truncated.output.contains("page it with read"),
        "{}",
        &truncated.output[truncated.output.len().saturating_sub(300)..]
    );
    let uri = artifact_uri(&truncated.output);

    let found = registry
        .execute(
            "search",
            &serde_json::json!({"mode": "grep", "query": "needle", "path": uri, "case_sensitive": false}),
        )
        .await
        .expect("search");
    assert_eq!(found.exit_code, 0, "{}", found.output);
    assert!(
        found
            .output
            .contains("4001:NEEDLE: linker failed on symbol foo"),
        "{}",
        found.output
    );

    let page = registry
        .execute(
            "read",
            &serde_json::json!({"file_path": uri, "offset": 4000, "limit": 2}),
        )
        .await
        .expect("read");
    assert_eq!(page.exit_code, 0, "{}", page.output);
    assert!(page.output.contains("  4001\tNEEDLE"), "{}", page.output);
    assert!(
        page.output.contains("continue with offset=4002"),
        "{}",
        page.output
    );

    let wrong_mode = registry
        .execute(
            "search",
            &serde_json::json!({"mode": "glob", "query": "*", "path": uri}),
        )
        .await
        .expect("search");
    assert_ne!(wrong_mode.exit_code, 0);
}

#[tokio::test]
async fn an_unretained_artifact_is_refused_by_name() {
    let workspace = tempfile::tempdir().expect("workspace");
    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let uri = "a3s://tool-output/bash/0000";
    let result = executor
        .registry()
        .execute("read", &serde_json::json!({"file_path": uri}))
        .await
        .expect("read");
    assert_ne!(result.exit_code, 0);
    assert!(
        result.output.contains("not retained in this session"),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn a_read_page_ends_inside_the_result_budget_and_continues() {
    let workspace = tempfile::tempdir().expect("workspace");
    let body = (1..=2_000)
        .map(|line| format!("line {line:04} {}\n", "z".repeat(40)))
        .collect::<String>();
    std::fs::write(workspace.path().join("big.txt"), &body).expect("write");
    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let registry = executor.registry();
    let budget = crate::tools::ToolResultTransformPolicyV1::default().max_output_bytes;

    let first = registry
        .execute("read", &serde_json::json!({"file_path": "big.txt"}))
        .await
        .expect("read");
    assert_eq!(first.exit_code, 0, "{}", first.output);
    assert!(first.output.len() <= budget, "{}", first.output.len());
    assert!(
        !first.output.contains("omitted"),
        "the page must not be cut mid-page"
    );
    let next = first
        .output
        .rsplit("continue with offset=")
        .next()
        .expect("continuation");
    let next: usize = next
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .expect("offset");
    assert!(next > 100 && next < 2_000, "{next}");

    let second = registry
        .execute(
            "read",
            &serde_json::json!({"file_path": "big.txt", "offset": next}),
        )
        .await
        .expect("read");
    assert!(
        second
            .output
            .starts_with(&format!("{:>6}\tline {:04}", next + 1, next + 1)),
        "{}",
        &second.output[..80]
    );
}
