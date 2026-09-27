//! Records, at session load, a turn the previous process never finished so it does not look like a silent stop.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use crate::session::persistence::Summary;

/// `stop_reason` for a turn lost with its process; distinct from `error` and `cancelled` so trace consumers can tell them apart.
pub const INTERRUPTED_STOP_REASON: &str = "interrupted";

/// Transcript line and `turn_result.error` text.
pub const INTERRUPTED_MESSAGE: &str = "A3S Code stopped before this turn finished (the agent process exited or was restarted). Committed tool results were kept.";

/// Only the tail of `events.jsonl` is scanned; one turn's events fit here many times over.
const EVENTS_TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedTurn {
    /// Trace turn number the previous process uploaded `metadata.json` under.
    pub trace_turn: u64,
    /// Prompt id of that turn; the `request_id` in its `metadata.json`.
    pub prompt_id: String,
    /// `ts` of the unclosed `turn_started`, when readable.
    pub started_at: Option<String>,
}

impl InterruptedTurn {
    pub(crate) fn model_reminder(&self) -> String {
        "The previous turn was interrupted (the system exited or was restarted). Work before this message may have been lost."
            .to_string()
    }

    /// Closes the turn on the replay rail so clients paint an end marker. Meta matches the actor's own emits so it stays cursor-addressable.
    pub(crate) fn turn_completed_update(
        &self,
        session_id: &agent_client_protocol::SessionId,
    ) -> crate::session::storage::SessionUpdate {
        let update = crate::session::turn_completion::build_turn_completed(
            self.prompt_id.clone(),
            serde_json::json!(INTERRUPTED_STOP_REASON),
            serde_json::json!(INTERRUPTED_MESSAGE),
            None,
            None,
            None,
        );
        let notification = crate::extensions::notification::SessionNotification {
            session_id: session_id.clone(),
            update,
            meta: Some(serde_json::json!({
                "eventId": crate::util::event_id::generate_event_id(&session_id.0),
                "agentTimestampMs": chrono::Utc::now().timestamp_millis(),
            })),
        };
        crate::session::storage::SessionUpdate::A3s(Box::new(notification))
    }

    /// `turn_result.json` for the trace turn whose `metadata.json` the dead process already uploaded.
    pub(crate) fn turn_result(&self) -> crate::upload::trace::TurnResultMetadata {
        crate::upload::trace::TurnResultMetadata {
            schema_version: crate::upload::trace::GCS_SCHEMA_VERSION,
            request_id: self.prompt_id.clone(),
            completed: false,
            stop_reason: Some(INTERRUPTED_STOP_REASON.to_string()),
            total_tokens: None,
            input_tokens: None,
            cached_input_tokens: None,
            output_tokens: None,
            error: Some(INTERRUPTED_MESSAGE.to_string()),
            finished_at: chrono::Utc::now().to_rfc3339(),
            signals: None,
            turn_delta: None,
            resolved_model: None,
            subagents_spawned: Vec::new(),
            start_prompt_mode: None,
            end_prompt_mode: None,
        }
    }

    /// Closes the open `turn_started` in `events.jsonl` so the next load does not report this turn again.
    pub(crate) fn close_events_turn(&self, session_dir: &Path) {
        a3s_code_session_events::EventWriter::open(session_dir).emit(
            a3s_code_session_events::Event::TurnEnded {
                outcome: a3s_code_session_events::TurnOutcomeLabel::Interrupted,
                cancellation_category: None,
                cancellation_context: None,
            },
        );
    }
}

/// `Some` when the newest turn in `session_dir` started but never ended and `summary` still names it.
pub(crate) fn detect_interrupted_turn(
    session_dir: &Path,
    summary: &Summary,
) -> Option<InterruptedTurn> {
    let prompt_id = summary.request_id.clone()?;
    let trace_turn = summary.next_trace_turn.checked_sub(1)?;
    let tail = read_events_tail(&session_dir.join("events.jsonl"))?;
    let started_at = open_turn_started_at(&tail)?;
    Some(InterruptedTurn {
        trace_turn,
        prompt_id,
        started_at,
    })
}

/// Last `EVENTS_TAIL_BYTES` of the file, cut to a line boundary.
fn read_events_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(EVENTS_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        // The first line is cut mid-way; drop it.
        match text.find('\n') {
            Some(idx) => text.drain(..=idx),
            None => return None,
        };
    }
    Some(text)
}

/// `Some(ts)` when the last turn event in `tail` is a `turn_started`; `None` when it is a `turn_ended` or there is none.
fn open_turn_started_at(tail: &str) -> Option<Option<String>> {
    let mut open: Option<Option<String>> = None;
    for line in tail.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("turn_started") => {
                open = Some(
                    event
                        .get("ts")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
            Some("turn_ended") => open = None,
            _ => {}
        }
    }
    open
}
