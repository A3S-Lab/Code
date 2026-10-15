use super::*;
use a3s_code_core::capability::{CapabilityEffect, CapabilityEffectError};

#[derive(Clone)]
pub(super) struct RetainedRun {
    session: String,
    nonce: String,
}

struct Release {
    bridge: Arc<DocumentBridge>,
    owner: String,
    run: RetainedRun,
}

#[async_trait]
impl CapabilityEffect for Release {
    fn name(&self) -> &str {
        "studio.document.run"
    }

    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.bridge
            .release_run(&self.run.session, &self.run.nonce)
            .await
            .map_err(|error| CapabilityEffectError::new(error.to_string()))?;
        self.bridge
            .runs
            .lock()
            .map_err(|_| CapabilityEffectError::new("document Run registry unavailable"))?
            .remove(&self.owner);
        Ok(())
    }
}

impl DocumentBridge {
    pub(super) fn retain_run(self: &Arc<Self>, ctx: &ToolContext, session: &str) -> Result<String> {
        ctx.workspace_operation(WorkspaceOperation::Read)?.check()?;
        let owner = ctx.capability_run_instance_id()?.to_string();
        let mut runs = self
            .runs
            .lock()
            .map_err(|_| anyhow::anyhow!("document Run registry unavailable"))?;
        if let Some(run) = runs.get(&owner) {
            anyhow::ensure!(run.session == session, "document session binding mismatch");
            return Ok(run.nonce.clone());
        }
        anyhow::ensure!(runs.len() < 64, "document Run capacity exhausted");
        let run = RetainedRun {
            session: session.to_owned(),
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        ctx.register_run_capability_effect(Release {
            bridge: self.clone(),
            owner: owner.to_owned(),
            run: run.clone(),
        })?;
        runs.insert(owner.to_owned(), run.clone());
        Ok(run.nonce)
    }
}
