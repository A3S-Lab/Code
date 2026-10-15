//! Read-only ACP observations of the Run admitted by Core's streaming entry point.

use a3s_code_core::{run::RunHandle, AgentEvent};
use serde::Serialize;

pub(crate) const METHOD: &str = "_a3s/execution/observe";

pub(crate) struct Binding {
    prompt_id: String,
}

impl Binding {
    pub(crate) fn from_meta(meta: Option<&agent_client_protocol::Meta>) -> Option<Self> {
        let meta = meta?;
        if meta.get("a3sExecutionObservation")?.as_u64()? != 1 {
            return None;
        }
        let id = meta.get("promptId")?.as_str()?;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return None;
        }
        Some(Self {
            prompt_id: id.to_owned(),
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    RunOpened,
    ModelTurnOpened,
    ModelTurnClosed,
    RunClosed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Observation {
    protocol_version: u32,
    session_id: String,
    prompt_id: String,
    core_session_id: String,
    run_id: String,
    sequence: u64,
    kind: Kind,
    turn: Option<usize>,
    status: Option<a3s_code_core::run::RunStatus>,
}

pub(crate) struct Observer {
    run: RunHandle,
    session_id: String,
    prompt_id: String,
    sequence: u64,
    turn: Option<usize>,
}

impl Observer {
    pub(crate) fn new(run: RunHandle, session_id: String, binding: &Binding) -> Self {
        Self {
            run,
            session_id,
            prompt_id: binding.prompt_id.clone(),
            sequence: 0,
            turn: None,
        }
    }

    fn packet(
        &mut self,
        kind: Kind,
        turn: Option<usize>,
        status: Option<a3s_code_core::run::RunStatus>,
    ) -> Option<Observation> {
        self.sequence = self.sequence.checked_add(1)?;
        Some(Observation {
            protocol_version: 1,
            session_id: self.session_id.clone(),
            prompt_id: self.prompt_id.clone(),
            core_session_id: self.run.session_id().to_owned(),
            run_id: self.run.id().to_owned(),
            sequence: self.sequence,
            kind,
            turn,
            status,
        })
    }

    pub(crate) fn opened(&mut self) -> Option<Observation> {
        self.packet(Kind::RunOpened, None, None)
    }

    pub(crate) fn event(&mut self, event: &AgentEvent) -> Option<Observation> {
        match event {
            AgentEvent::TurnStart { turn } => {
                self.turn = Some(*turn);
                self.packet(Kind::ModelTurnOpened, Some(*turn), None)
            }
            AgentEvent::TurnEnd { turn, .. } if self.turn == Some(*turn) => {
                self.turn = None;
                self.packet(Kind::ModelTurnClosed, Some(*turn), None)
            }
            _ => None,
        }
    }

    pub(crate) async fn closed(&mut self) -> Option<Observation> {
        let snapshot = self.run.snapshot().await?;
        if !snapshot.status.is_terminal() {
            return None;
        }
        self.turn = None;
        self.packet(Kind::RunClosed, None, Some(snapshot.status))
    }
}

#[cfg(test)]
#[path = "run_observation_tests.rs"]
mod tests;
