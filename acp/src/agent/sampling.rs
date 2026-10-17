//! Client-mediated sampling consent for MCP servers.

use super::*;

impl A3sCodeAgent {
    /// Build the consent channel (once) and arm the manager's sampling
    /// factory with the composition `Bounded(ApprovalGated(Llm))` — the
    /// consent bridge INSIDE the per-server ceiling, per both types'
    /// docs. The sampling voice is the launch default model. Any arming
    /// failure leaves sampling refused (fail closed); this never fails
    /// the agent itself.
    pub(super) async fn arm_sampling_consent(&self, agent: &Agent) -> acp::Result<()> {
        let Some(tx) = self.consent_tx.borrow().clone() else {
            // The channel is built at construction; unreachable, but a
            // missing surface refuses rather than arms.
            return Ok(());
        };
        let model_ref = self
            .launch
            .config
            .default_model
            .as_deref()
            .unwrap_or_default()
            .to_string();
        let Some((provider_id, model_id)) = model_ref.split_once('/') else {
            // No default model voice: sampling stays refused.
            return Ok(());
        };
        let Some(llm) = self.launch.config.llm_config(provider_id, model_id) else {
            // No usable client for the default voice: refused.
            return Ok(());
        };
        let llm = a3s_code_core::llm::create_client_with_config(llm);
        let factory: a3s_code_core::mcp::SamplingHandlerFactory =
            std::sync::Arc::new(move |server| sampling_handler_for(server, &llm, &model_ref, &tx));
        agent
            .set_sampling_handler_factory(factory)
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(())
    }
}

/// Resolve which live session a sampling consent ask belongs to: the
/// session whose declared `mcp_servers` names the server owns it; a
/// baseline (config-declared) server belongs to whichever session is
/// live; no live session means nobody to ask (the caller refuses).
pub(super) fn resolve_consent_session(
    sessions: &HashMap<String, LiveSession>,
    server: &str,
) -> Option<acp::SessionId> {
    sessions
        .iter()
        .find(|(_, live)| {
            live.mcp_servers
                .iter()
                .any(|declared| declared.name == server)
        })
        .map(|(key, _)| acp::SessionId::new(key.clone()))
        .or_else(|| {
            sessions
                .keys()
                .next()
                .map(|key| acp::SessionId::new(key.clone()))
        })
}

/// The composition the wiring contract pins: consent bridge INSIDE the
/// per-server ceiling, the model voice capped on generation.
pub(super) fn sampling_handler_for(
    server: &str,
    llm: &std::sync::Arc<dyn a3s_code_core::llm::LlmClient>,
    model: &str,
    consent: &tokio::sync::mpsc::Sender<crate::sampling_consent::SamplingConsentAsk>,
) -> std::sync::Arc<dyn a3s_code_core::mcp::transport::SamplingHandler> {
    use a3s_code_core::mcp::client::{
        ApprovalGatedSampling, BoundedSamplingHandler, LlmSamplingHandler,
    };
    let voice = std::sync::Arc::new(LlmSamplingHandler::new(
        std::sync::Arc::clone(llm),
        model.to_string(),
    ));
    let bridge = std::sync::Arc::new(crate::sampling_consent::SamplingConsentBridge::new(
        consent.clone(),
    ));
    let gated = std::sync::Arc::new(ApprovalGatedSampling::new(
        server.to_string(),
        voice,
        bridge,
    ));
    std::sync::Arc::new(BoundedSamplingHandler::new(gated))
}

/// Serve parked sampling asks: resolve the owning session, ask through
/// the request_permission channel (Allow once / Deny), and settle the
/// reply. Every non-approval path refuses (the bridge also times out).
pub(super) async fn serve_sampling_consent(
    mut asks: tokio::sync::mpsc::Receiver<crate::sampling_consent::SamplingConsentAsk>,
    client: Rc<RefCell<Option<Rc<acp::AgentSideConnection>>>>,
    sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
) {
    const OPTIONS: [&str; 2] = ["Allow once", "Deny"];
    while let Some(ask) = asks.recv().await {
        let allowed = async {
            let owning = {
                let sessions = sessions.lock().await;
                resolve_consent_session(&sessions, &ask.server)
            };
            // No session to ask and no live connection both refuse.
            let session_id = owning?;
            let conn = client.borrow().clone();
            let conn = conn?;
            let options: Vec<String> = OPTIONS.iter().map(|text| text.to_string()).collect();
            let question_id = format!("mcp-sampling-{}", ask.server);
            // Sampling consent is a closed yes/no; free text is never consent.
            let request = question_permission_request(
                &session_id,
                &question_id,
                &ask.question,
                &options,
                /*allow_free_text=*/ false,
            )?;
            let response = conn.request_permission(request).await.ok()?;
            let answer = if followup_message(&response).is_some() {
                // Typed text means the user answered in prose; the picked row
                // then names nothing trustworthy, and "Allow once" on it must
                // not grant sampling.
                None
            } else {
                answer_from_permission_outcome(&response.outcome, &options)
            };
            answer.as_deref().map(|answer| answer == "Allow once")
        }
        .await
        .unwrap_or(false);
        let _ = ask.reply.send(allowed);
    }
}
