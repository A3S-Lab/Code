//! Release bindings that preserve Core's policy, evidence and exact Run handles.

use super::session::Session;
use super::*;

#[napi]
pub struct RunHandle {
    inner: a3s_code_core::run::RunHandle,
}

#[napi]
impl RunHandle {
    #[napi(getter)]
    pub fn id(&self) -> String {
        self.inner.id().to_owned()
    }
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        self.inner.session_id().to_owned()
    }
    /// Request cancellation of this exact original worker.
    #[napi]
    pub async fn cancel(&self) -> bool {
        self.inner.cancel().await
    }
    #[napi]
    pub async fn snapshot(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.snapshot().await)
            .map_err(|error| napi::Error::from_reason(error.to_string()))
    }
}

#[napi(object, object_from_js = false)]
pub struct AgentRunStream {
    pub stream: EventStream,
    pub run: Option<RunHandle>,
}

/// Exactly one field is set: the decision, the started slice's stream, or the
/// snapshot of the Run this operation already admitted.
#[napi(object, object_from_js = false)]
pub struct GoalTickStream {
    pub decision: Option<String>,
    pub started: Option<AgentRunStream>,
    pub replayed: Option<serde_json::Value>,
}

fn run_stream(admitted: a3s_code_core::AgentRunStream) -> AgentRunStream {
    AgentRunStream {
        stream: EventStream {
            rx: Arc::new(tokio::sync::Mutex::new(admitted.events)),
            done: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(Some(admitted.worker))),
        },
        run: admitted.run.map(|inner| RunHandle { inner }),
    }
}

#[napi]
impl Session {
    /// Stream with the exact Core Run captured at admission. Local commands
    /// that do not admit a model Run omit the optional Run handle.
    #[napi]
    pub async fn stream_with_run(
        &self,
        prompt: String,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<AgentRunStream> {
        let history = history
            .map(|value| js_messages_to_rust(&value))
            .transpose()?;
        let admitted = self
            .inner
            .stream_with_run(&prompt, history.as_deref())
            .await
            .map_err(node_code_error)?;
        Ok(run_stream(admitted))
    }

    /// `tickGoal` for an interactive host: a started slice's events arrive as a stream.
    #[napi]
    pub async fn stream_goal_tick(
        &self,
        command_json: serde_json::Value,
    ) -> napi::Result<GoalTickStream> {
        let command = serde_json::from_value(command_json)
            .map_err(|e| napi::Error::from_reason(format!("invalid goal command: {e}")))?;
        let none = GoalTickStream {
            decision: None,
            started: None,
            replayed: None,
        };
        Ok(
            match self
                .inner
                .stream_goal_tick(&command)
                .await
                .map_err(node_code_error)?
            {
                a3s_code_core::GoalTickStream::Decided(decision) => GoalTickStream {
                    decision: Some(decision.as_str().to_owned()),
                    ..none
                },
                a3s_code_core::GoalTickStream::Started(admitted) => GoalTickStream {
                    started: Some(run_stream(admitted)),
                    ..none
                },
                a3s_code_core::GoalTickStream::Replayed(snapshot) => GoalTickStream {
                    replayed: Some(
                        serde_json::to_value(snapshot)
                            .map_err(|error| napi::Error::from_reason(error.to_string()))?,
                    ),
                    ..none
                },
            },
        )
    }

    /// Irreversibly discard facts after seq in this session's log. This does
    /// not create a second session or roll back files, vectors or memory.
    /// Refuses while a Run owns this session.
    #[napi]
    pub async fn fork_session_at(&self, seq: f64) -> napi::Result<f64> {
        if !seq.is_finite() || seq < 0.0 || seq.fract() != 0.0 || seq > 9_007_199_254_740_991.0 {
            return Err(napi::Error::from_reason(
                "seq must be a nonnegative safe integer",
            ));
        }
        self.inner
            .fork_session_at(seq as u64)
            .await
            .map(|removed| removed as f64)
            .map_err(node_code_error)
    }

    #[napi]
    pub fn projected_capability_names(&self) -> Vec<String> {
        self.inner.projected_capability_names()
    }

    /// Store the policy admitted by the next Run; an open Run keeps its pin.
    #[napi]
    pub fn set_permission_policy(&self, policy: PermissionPolicy) -> napi::Result<()> {
        self.inner
            .set_permission_policy(js_permission_policy_to_rust(policy)?)
            .map_err(node_code_error)
    }

    #[napi]
    pub fn set_permission_posture(&self, posture: String) -> napi::Result<()> {
        if !matches!(
            posture.as_str(),
            "default" | "plan" | "auto" | "always-approve" | "yolo"
        ) {
            return Err(napi::Error::from_reason("unknown permission posture"));
        }
        let posture = if posture == "always-approve" {
            "yolo"
        } else {
            &posture
        };
        self.inner
            .set_permission_posture(posture)
            .map_err(node_code_error)
    }

    /// Bounded read of this session's original Run/artifact evidence.
    #[cfg(feature = "advanced-harness")]
    #[napi]
    pub async fn read_evaluation_evidence(
        &self,
        request: serde_json::Value,
    ) -> napi::Result<serde_json::Value> {
        let request = serde_json::from_value(request).map_err(|error| {
            napi::Error::from_reason(format!("invalid evidence request: {error}"))
        })?;
        let evidence = self
            .inner
            .read_evaluation_evidence(request)
            .await
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        serde_json::to_value(evidence).map_err(|error| napi::Error::from_reason(error.to_string()))
    }
}
