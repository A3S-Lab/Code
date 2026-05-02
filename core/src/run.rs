//! Durable run primitives for agent executions.
//!
//! This module is intentionally small: it records runtime events and maintains a
//! stable run status snapshot that can be persisted by session stores.

use crate::agent::AgentEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Created,
    Planning,
    Executing,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventRecord {
    pub sequence: usize,
    pub timestamp_ms: u64,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: String,
    pub session_id: String,
    pub status: RunStatus,
    pub prompt: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub event_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub snapshot: RunSnapshot,
    pub events: Vec<RunEventRecord>,
}

impl RunSnapshot {
    fn new(id: String, session_id: String, prompt: String) -> Self {
        let now = now_ms();
        Self {
            id,
            session_id,
            status: RunStatus::Created,
            prompt,
            created_at_ms: now,
            updated_at_ms: now,
            result_text: None,
            error: None,
            event_count: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemoryRunStore {
    runs: RwLock<HashMap<String, RunSnapshot>>,
    events: RwLock<HashMap<String, Vec<RunEventRecord>>>,
}

impl InMemoryRunStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_run(&self, session_id: &str, prompt: &str) -> RunSnapshot {
        let id = format!("run-{}", uuid::Uuid::new_v4());
        let snapshot = RunSnapshot::new(id.clone(), session_id.to_string(), prompt.to_string());
        self.runs.write().await.insert(id.clone(), snapshot.clone());
        self.events.write().await.insert(id, Vec::new());
        snapshot
    }

    pub async fn record_event(&self, run_id: &str, event: AgentEvent) -> Option<RunSnapshot> {
        let mut events = self.events.write().await;
        let run_events = events.get_mut(run_id)?;
        let sequence = run_events.len();
        run_events.push(RunEventRecord {
            sequence,
            timestamp_ms: now_ms(),
            event: event.clone(),
        });
        drop(events);

        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        apply_event_to_snapshot(run, &event);
        run.event_count += 1;
        run.updated_at_ms = now_ms();
        Some(run.clone())
    }

    pub async fn mark_failed(&self, run_id: &str, error: impl Into<String>) -> Option<RunSnapshot> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        if run.status == RunStatus::Cancelled {
            return Some(run.clone());
        }
        run.status = RunStatus::Failed;
        run.error = Some(error.into());
        run.updated_at_ms = now_ms();
        Some(run.clone())
    }

    pub async fn mark_cancelled(&self, run_id: &str) -> Option<RunSnapshot> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        run.status = RunStatus::Cancelled;
        run.updated_at_ms = now_ms();
        Some(run.clone())
    }

    pub async fn snapshot(&self, run_id: &str) -> Option<RunSnapshot> {
        self.runs.read().await.get(run_id).cloned()
    }

    pub async fn events(&self, run_id: &str) -> Vec<RunEventRecord> {
        self.events
            .read()
            .await
            .get(run_id)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn list(&self) -> Vec<RunSnapshot> {
        let mut runs = self.runs.read().await.values().cloned().collect::<Vec<_>>();
        runs.sort_by_key(|run| run.created_at_ms);
        runs
    }

    pub async fn records(&self) -> Vec<RunRecord> {
        let snapshots = self.runs.read().await.values().cloned().collect::<Vec<_>>();
        let events = self.events.read().await;
        let mut records = snapshots
            .into_iter()
            .map(|snapshot| RunRecord {
                events: events.get(&snapshot.id).cloned().unwrap_or_default(),
                snapshot,
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.snapshot.created_at_ms);
        records
    }

    pub async fn replace_records(&self, records: Vec<RunRecord>) {
        let mut run_map = HashMap::new();
        let mut event_map = HashMap::new();

        for mut record in records {
            record.snapshot.event_count = record.events.len();
            event_map.insert(record.snapshot.id.clone(), record.events);
            run_map.insert(record.snapshot.id.clone(), record.snapshot);
        }

        *self.runs.write().await = run_map;
        *self.events.write().await = event_map;
    }
}

#[derive(Clone)]
pub struct RunHandle {
    id: String,
    session_id: String,
    store: Arc<InMemoryRunStore>,
    cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    current_run_id: Arc<Mutex<Option<String>>>,
    hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
}

impl RunHandle {
    pub(crate) fn new(
        id: String,
        session_id: String,
        store: Arc<InMemoryRunStore>,
        cancel_token: Arc<Mutex<Option<CancellationToken>>>,
        current_run_id: Arc<Mutex<Option<String>>>,
        hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    ) -> Self {
        Self {
            id,
            session_id,
            store,
            cancel_token,
            current_run_id,
            hook_executor,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn snapshot(&self) -> Option<RunSnapshot> {
        self.store.snapshot(&self.id).await
    }

    pub async fn events(&self) -> Vec<RunEventRecord> {
        self.store.events(&self.id).await
    }

    pub async fn status(&self) -> Option<RunStatus> {
        self.snapshot().await.map(|snapshot| snapshot.status)
    }

    pub async fn cancel(&self) -> bool {
        let current_run_id = self.current_run_id.lock().await.clone();
        if current_run_id.as_deref() != Some(self.id.as_str()) {
            return false;
        }

        let token = self.cancel_token.lock().await.clone();
        if let Some(token) = token {
            token.cancel();
            let _ = self.store.mark_cancelled(&self.id).await;
            if let Some(executor) = &self.hook_executor {
                executor
                    .record_run_cancelled(&self.id, &self.session_id, Some("cancelled by host"))
                    .await;
            }
            true
        } else {
            false
        }
    }
}

fn apply_event_to_snapshot(run: &mut RunSnapshot, event: &AgentEvent) {
    match event {
        AgentEvent::Start { prompt } => {
            run.status = RunStatus::Executing;
            if run.prompt.is_empty() {
                run.prompt = prompt.clone();
            }
        }
        AgentEvent::PlanningStart { .. } => {
            run.status = RunStatus::Planning;
        }
        AgentEvent::StepStart { .. }
        | AgentEvent::ToolStart { .. }
        | AgentEvent::TurnStart { .. }
            if !matches!(run.status, RunStatus::Planning) =>
        {
            run.status = RunStatus::Executing;
        }
        AgentEvent::End { text, .. } => {
            if run.status == RunStatus::Cancelled {
                return;
            }
            run.status = RunStatus::Completed;
            run.result_text = Some(text.clone());
            run.error = None;
        }
        AgentEvent::Error { message } => {
            if run.status == RunStatus::Cancelled {
                return;
            }
            run.status = RunStatus::Failed;
            run.error = Some(message.clone());
        }
        _ => {}
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_store_tracks_status_and_events() {
        let store = InMemoryRunStore::new();
        let run = store.create_run("session-1", "fix tests").await;

        store
            .record_event(
                &run.id,
                AgentEvent::Start {
                    prompt: "fix tests".to_string(),
                },
            )
            .await;
        store
            .record_event(
                &run.id,
                AgentEvent::End {
                    text: "done".to_string(),
                    usage: Default::default(),
                    verification_summary: Box::new(
                        crate::verification::VerificationSummary::from_reports(&[]),
                    ),
                    meta: None,
                },
            )
            .await;

        let snapshot = store.snapshot(&run.id).await.unwrap();
        assert_eq!(snapshot.status, RunStatus::Completed);
        assert_eq!(snapshot.result_text.as_deref(), Some("done"));
        assert_eq!(snapshot.event_count, 2);
        assert_eq!(store.events(&run.id).await.len(), 2);
    }

    #[tokio::test]
    async fn run_store_replaces_persisted_records() {
        let source = InMemoryRunStore::new();
        let run = source.create_run("session-1", "persist").await;
        source
            .record_event(
                &run.id,
                AgentEvent::Start {
                    prompt: "persist".to_string(),
                },
            )
            .await;

        let target = InMemoryRunStore::new();
        target.replace_records(source.records().await).await;

        assert_eq!(target.list().await.len(), 1);
        assert_eq!(target.events(&run.id).await.len(), 1);
        assert_eq!(target.snapshot(&run.id).await.unwrap().event_count, 1);
    }

    #[tokio::test]
    async fn run_handle_only_cancels_current_run() {
        let store = Arc::new(InMemoryRunStore::new());
        let run = store.create_run("session-1", "fix tests").await;
        let cancel_token = Arc::new(Mutex::new(Some(CancellationToken::new())));
        let current_run_id = Arc::new(Mutex::new(Some(run.id.clone())));
        let handle = RunHandle::new(
            run.id.clone(),
            run.session_id.clone(),
            store.clone(),
            cancel_token,
            current_run_id.clone(),
            None,
        );

        assert!(handle.cancel().await);
        assert_eq!(handle.status().await, Some(RunStatus::Cancelled));

        *current_run_id.lock().await = Some("other-run".to_string());
        assert!(!handle.cancel().await);
    }
}
