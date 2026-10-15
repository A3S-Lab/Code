#![cfg(test)]
#![cfg(unix)]

use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio_util::sync::CancellationToken;

struct Fixture(PathBuf);

impl Fixture {
    fn new(script: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("a3s-acp-owned-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let entry = root.join("entry");
        std::fs::write(&entry, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self(root)
    }

    fn tool(&self) -> Arc<dyn Tool> {
        from_entries(&[serde_json::json!({"id": "owned", "entry": self.0.join("entry")})])
            .into_iter()
            .next()
            .unwrap()
    }

    async fn wait_for_start(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !self.0.join("started").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("actual plugin tool did not start");
    }

    async fn assert_no_later_write(&self) {
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!self.0.join("leaked").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn cancelled_context_does_not_execute_an_installed_tool() {
    let fixture = Fixture::new("touch leaked");
    let context = ToolContext::new(fixture.0.clone());
    context.cancellation_token().cancel();
    let output = fixture
        .tool()
        .execute(&serde_json::json!({}), &context)
        .await
        .unwrap();
    assert!(!output.success);
    assert!(output.content.contains("cancelled"));
    assert!(matches!(
        output.error_kind,
        Some(ToolErrorKind::Cancelled { .. })
    ));
    assert!(!fixture.0.join("leaked").exists());
}

#[tokio::test]
async fn tool_context_cancellation_stops_the_actual_descendant() {
    let fixture = Fixture::new("cat >/dev/null; (touch started; sleep 0.40; touch leaked) & wait");
    let context = ToolContext::new(fixture.0.clone());
    let cancellation: CancellationToken = context.cancellation_token();
    let tool = fixture.tool();
    let invocation =
        tokio::spawn(async move { tool.execute(&serde_json::json!({}), &context).await });
    fixture.wait_for_start().await;
    cancellation.cancel();
    let output = invocation.await.unwrap().unwrap();
    assert!(!output.success);
    assert!(output.content.contains("cancelled"));
    fixture.assert_no_later_write().await;
}

#[tokio::test]
async fn plugin_output_is_bounded_before_the_plugin_result_is_built() {
    let fixture =
        Fixture::new("dd if=/dev/zero bs=1024 count=2048 2>/dev/null | tr '\\000' o; cat > input");
    let args = serde_json::json!({"body": "i".repeat(60 * 1024)});
    let output = fixture
        .tool()
        .execute(&args, &ToolContext::new(fixture.0.clone()))
        .await
        .unwrap();
    assert!(output.success);
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("input")).unwrap(),
        args.to_string()
    );
    assert!(output.content.starts_with("oooo"));
    assert!(output.content.len() <= IO_LIMIT);
}

#[tokio::test]
async fn oversized_arguments_cannot_start_a_tool() {
    let fixture = Fixture::new("touch leaked");
    let args = serde_json::json!({"body": "i".repeat(IO_LIMIT)});
    let output = fixture
        .tool()
        .execute(&args, &ToolContext::new(fixture.0.clone()))
        .await
        .unwrap();
    assert!(!output.success);
    assert!(output.content.contains("arguments exceed"));
    assert!(!fixture.0.join("leaked").exists());
}

#[tokio::test]
async fn native_backend_refusal_is_available_before_a_live_turn_or_process() {
    use a3s_code_core::workspace::{
        WorkspaceCapabilities, WorkspaceServices, WorkspaceServicesBuilder,
    };
    let fixture = Fixture::new("touch leaked");
    let tool = from_publication(
        "native",
        fixture.0.join("entry"),
        crate::plugin_workspace::WorkspaceBinding::new(&fixture.0).unwrap(),
    )
    .unwrap();
    let base = WorkspaceServices::local(&fixture.0);
    let services = WorkspaceServicesBuilder::new(base.workspace_ref().clone(), base.fs())
        .capabilities(WorkspaceCapabilities::read_write())
        .build();
    let ctx = ToolContext::new(fixture.0.clone()).with_workspace_services(services);
    let error = tool
        .validate_execution_available(&serde_json::json!({}), &ctx)
        .expect_err("read/write backend cannot execute native plugins");
    assert!(error
        .to_string()
        .contains("active workspace capability scope"));
    let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
    assert!(!result.success);
    assert!(!fixture.0.join("leaked").exists());
}

#[test]
fn native_replaced_workspace_refuses_during_effect_free_preparation() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let fixture = Fixture::new("touch leaked");
    let tool = from_publication(
        "native",
        fixture.0.join("entry"),
        crate::plugin_workspace::WorkspaceBinding::new(&workspace).unwrap(),
    )
    .unwrap();
    std::fs::rename(&workspace, root.path().join("retired")).unwrap();
    std::fs::create_dir(&workspace).unwrap();
    let error = tool
        .validate_execution_available(&serde_json::json!({}), &ToolContext::new(workspace))
        .expect_err("same path cannot replace the admitted directory inode");
    assert!(error.to_string().contains("workspace is not admitted"));
    assert!(!fixture.0.join("leaked").exists());
}

#[tokio::test]
async fn plugin_deadline_kills_a_writer_after_its_streams_close() {
    let fixture =
        Fixture::new("cat >/dev/null; exec 1>&- 2>&-; touch started; sleep 6; touch leaked");
    let output = fixture
        .tool()
        .execute(&serde_json::json!({}), &ToolContext::new(fixture.0.clone()))
        .await
        .unwrap();
    assert!(fixture.0.join("started").exists());
    assert!(!output.success);
    assert!(output.content.contains("timed out"), "{}", output.content);
    assert!(matches!(
        output.error_kind,
        Some(ToolErrorKind::Timeout { .. })
    ));
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    assert!(
        !fixture.0.join("leaked").exists(),
        "the actual plugin writer survived its five-second deadline"
    );
}
