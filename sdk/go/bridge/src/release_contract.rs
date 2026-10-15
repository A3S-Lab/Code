//! Additive release operations over the existing Agent and Session authority.

use super::*;
use a3s_code_core::mcp::{client::BoundedSamplingHandler, transport::SamplingHandler};

struct BridgeSampling {
    client: Arc<CallbackClient>,
    handler_id: String,
    server: String,
    timeout_ms: u64,
}

#[async_trait::async_trait]
impl SamplingHandler for BridgeSampling {
    async fn create_message(&self, params: Value) -> anyhow::Result<Value> {
        let result = self
            .client
            .invoke(
                &self.handler_id,
                "sampling/createMessage",
                json!({"server_name": self.server, "params": params}),
                self.timeout_ms,
            )
            .await
            .map_err(|_| anyhow::anyhow!("sampling refused by host callback"))?;
        anyhow::ensure!(
            result.is_object(),
            "sampling host returned an invalid result"
        );
        Ok(result)
    }
}

pub(super) async fn dispatch(
    state: &BridgeState,
    request: &BridgeRequest,
) -> Result<Value, BridgeFailure> {
    match request.operation.as_str() {
        "agent_global_mcp_configs" => encode(
            state
                .agent(&required::<String>(&request.params, "agent_id")?)
                .await?
                .global_mcp_configs()
                .await,
        ),
        "agent_has_sampling_handler_factory" => encode(
            state
                .agent(&required::<String>(&request.params, "agent_id")?)
                .await?
                .has_sampling_handler_factory()
                .await,
        ),
        "agent_set_sampling_handler_factory" => {
            let handler_id: String = required(&request.params, "handler_id")?;
            let timeout_ms: u64 = required(&request.params, "timeout_ms")?;
            if handler_id.is_empty() || !(1..=300_000).contains(&timeout_ms) {
                return Err(BridgeFailure::new(
                    "INVALID_REQUEST",
                    "invalid sampling callback identity or timeout",
                ));
            }
            let client = state.callback_client().await?;
            let factory: a3s_code_core::mcp::SamplingHandlerFactory = Arc::new(move |server| {
                Arc::new(BoundedSamplingHandler::new(Arc::new(BridgeSampling {
                    client: client.clone(),
                    handler_id: handler_id.clone(),
                    server: server.to_owned(),
                    timeout_ms,
                })))
            });
            state
                .agent(&required::<String>(&request.params, "agent_id")?)
                .await?
                .set_sampling_handler_factory(factory)
                .await?;
            Ok(json!({"armed": true}))
        }
        "session_projected_capability_names" => encode(
            state
                .request_session(&request.params)
                .await?
                .projected_capability_names(),
        ),
        "session_set_permission_policy" => {
            state
                .request_session(&request.params)
                .await?
                .set_permission_policy(required(&request.params, "policy")?)?;
            Ok(json!({"updated": true}))
        }
        "session_set_permission_posture" => {
            let posture: String = required(&request.params, "posture")?;
            if !matches!(
                posture.as_str(),
                "default" | "plan" | "auto" | "always-approve" | "yolo"
            ) {
                return Err(BridgeFailure::new(
                    "INVALID_REQUEST",
                    "unknown permission posture",
                ));
            }
            let posture = if posture == "always-approve" {
                "yolo"
            } else {
                &posture
            };
            state
                .request_session(&request.params)
                .await?
                .set_permission_posture(posture)?;
            Ok(json!({"updated": true}))
        }
        "session_fork_session_at" => encode(
            state
                .request_session(&request.params)
                .await?
                .fork_session_at(required(&request.params, "seq")?)
                .await?,
        ),
        #[cfg(feature = "advanced-harness")]
        "session_read_evaluation_evidence" => {
            let result = state
                .request_session(&request.params)
                .await?
                .read_evaluation_evidence(required(&request.params, "request")?)
                .await
                .map_err(|error| BridgeFailure::new("EVIDENCE_ERROR", error.to_string()))?;
            encode(result)
        }
        _ => Err(BridgeFailure::new(
            "UNSUPPORTED_OPERATION",
            "operation requires advanced-harness",
        )),
    }
}
