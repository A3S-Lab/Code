#![cfg(test)]

use super::*;
use crate::sandbox::SandboxOutput;
use std::sync::atomic::{AtomicBool, Ordering};

struct MutableSandbox {
    ready: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl BashSandbox for MutableSandbox {
    fn validate_execution_available(&self) -> anyhow::Result<()> {
        if !self.ready.load(Ordering::Acquire) {
            anyhow::bail!("missing descendant cleanup")
        }
        Ok(())
    }

    async fn exec_command(&self, _: &str, _: &str) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!("availability context does not execute commands")
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
async fn host_unavailability_survives_tiny_retrieval_budgets_and_refreshes() {
    let ready = Arc::new(AtomicBool::new(false));
    let provider = SandboxContextProvider::new(Arc::new(MutableSandbox {
        ready: Arc::clone(&ready),
    }));
    let mut query = ContextQuery::new("read a file");
    query.max_tokens = 1;
    query.max_results = 0;
    let unavailable = provider.query(&query).await.unwrap();
    let assembled = crate::context::ContextAssembler::new(crate::context::ContextBudget {
        max_tokens: 1,
        max_items: 0,
    })
    .assemble(&[unavailable]);
    assert!(assembled.to_xml().contains("missing descendant cleanup"));
    ready.store(true, Ordering::Release);
    assert!(provider.query(&query).await.unwrap().is_empty());
}
