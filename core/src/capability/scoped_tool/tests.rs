#![cfg(test)]

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Probe(Arc<AtomicUsize>);

#[async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        "availability-probe"
    }
    fn description(&self) -> &str {
        "No process or workspace effects during preparation"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    fn validate_execution_available(
        &self,
        args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(args["unavailable"] != true, "backend is unavailable");
        Ok(())
    }
    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success("executed"))
    }
}

fn projected(executions: Arc<AtomicUsize>) -> Arc<dyn Tool> {
    let revision = Sha256Digest::new(format!("sha256:{}", "ab".repeat(32))).unwrap();
    let source =
        crate::capability::CapabilitySource::host("availability", revision.clone()).unwrap();
    let id = CapabilityId::new(&source, crate::capability::CapabilityKind::Tool, "probe").unwrap();
    ScopedProjectedTool::bind(Arc::new(Probe(executions)), id, revision)
}

#[test]
fn projected_tool_preserves_effect_free_backend_refusal_before_a_turn_exists() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tool = projected(Arc::clone(&executions));
    let ctx = ToolContext::new(std::path::PathBuf::from("."));
    let error = tool
        .validate_execution_available(&serde_json::json!({"unavailable":true}), &ctx)
        .expect_err("projection cannot erase the admitted backend refusal");
    assert_eq!(error.to_string(), "backend is unavailable");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn successful_preparation_does_not_grant_projection_execution_authority() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tool = projected(Arc::clone(&executions));
    let ctx = ToolContext::new(std::path::PathBuf::from("."));
    tool.validate_execution_available(&serde_json::json!({}), &ctx)
        .unwrap();
    let error = tool
        .execute(&serde_json::json!({}), &ctx)
        .await
        .expect_err("unscoped context still cannot execute the projected tool");
    assert!(error.to_string().contains("active capability scope"));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
