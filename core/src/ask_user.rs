//! Structured question that parks a run without a write, a permission grant,
//! or a host steer.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const USER_QUESTION_SCHEMA: &str = "a3s.code.user-question.v1";
pub const USER_ANSWER_SCHEMA: &str = "a3s.code.user-answer.v1";
const MAX_QUESTIONS_PER_RUN: u32 = 3;
const MAX_OPTIONS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuestionV1 {
    pub schema: String,
    pub question_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskUserResume {
    Answered { text: String },
    Unanswered,
}

fn counts() -> &'static Mutex<HashMap<String, u32>> {
    static COUNTS: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug)]
pub enum AskUserError {
    CapExceeded,
    Invalid(&'static str),
}

/// Admit one question for `run_id` before a question fact is appended.
///
/// The fourth question in a run is rejected here. An answer is a
/// `question.answered` fact, not an in-process wait.
pub fn begin(
    run_id: &str,
    question_id: &str,
    question: &str,
    options: &[String],
    allow_free_text: bool,
) -> Result<UserQuestionV1, AskUserError> {
    if question.trim().is_empty() {
        return Err(AskUserError::Invalid("question is required"));
    }
    if options.len() > MAX_OPTIONS {
        return Err(AskUserError::Invalid("too many options"));
    }
    let mut counts = counts().lock().expect("ask-user counts");
    let count = counts.entry(run_id.to_string()).or_insert(0);
    if *count >= MAX_QUESTIONS_PER_RUN {
        return Err(AskUserError::CapExceeded);
    }
    *count += 1;
    let sequence = *count;
    drop(counts);
    Ok(UserQuestionV1 {
        schema: USER_QUESTION_SCHEMA.to_string(),
        question_id: format!("{question_id}-{sequence}"),
        question: question.to_string(),
        options: options.to_vec(),
        allow_free_text,
    })
}

fn pending_answers() -> &'static Mutex<HashMap<String, String>> {
    static ANSWERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ANSWERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn answer_notify() -> &'static tokio::sync::Notify {
    static NOTIFY: OnceLock<tokio::sync::Notify> = OnceLock::new();
    NOTIFY.get_or_init(tokio::sync::Notify::new)
}

/// Record a host answer for a later `question.answered` fact.
///
/// Returns false. The call does not settle the run; the fact log does.
pub fn answer(question_id: &str, text: &str) -> bool {
    if question_id.trim().is_empty() || text.trim().is_empty() {
        return false;
    }
    pending_answers()
        .lock()
        .expect("ask-user answers")
        .insert(question_id.to_string(), text.to_string());
    answer_notify().notify_waiters();
    false
}

/// Wait until [`answer`] stores text for `question_id`, or the run is cancelled.
pub async fn wait_for_answer(
    question_id: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Option<String> {
    loop {
        let notified = answer_notify().notified();
        if let Some(text) = pending_answers()
            .lock()
            .expect("ask-user answers")
            .remove(question_id)
        {
            return Some(text);
        }
        if cancel.is_cancelled() {
            return None;
        }
        tokio::select! {
            _ = notified => {}
            _ = cancel.cancelled() => return None,
        }
    }
}

/// Cancellation no longer synthesizes an unanswered result.
pub fn cancel(_question_id: &str) -> bool {
    false
}

pub fn resume_message(question_id: &str, resume: &AskUserResume) -> String {
    let value = match resume {
        AskUserResume::Answered { text } => serde_json::json!({
            "schema": USER_ANSWER_SCHEMA,
            "question_id": question_id,
            "status": "answered",
            "answer": text,
            "permission_grant": false,
        }),
        AskUserResume::Unanswered => serde_json::json!({
            "schema": USER_ANSWER_SCHEMA,
            "question_id": question_id,
            "status": "unanswered",
            "permission_grant": false,
        }),
    };
    value.to_string()
}

pub fn is_permission_grant(metadata: &Value) -> bool {
    metadata
        .get("permission_grant")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || metadata
            .get("approved")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_keeps_allow_free_text_and_does_not_answer_in_process() {
        let question = begin("run-1", "q-1", "Which file?", &["a.rs".into()], true).unwrap();
        assert_eq!(question.schema, USER_QUESTION_SCHEMA);
        assert!(question.allow_free_text);
        assert_eq!(question.options, vec!["a.rs".to_string()]);
        assert!(!answer(&question.question_id, "a.rs"));
        assert!(!cancel(&question.question_id));
        let message = resume_message(
            &question.question_id,
            &AskUserResume::Answered {
                text: "a.rs".into(),
            },
        );
        let value: Value = serde_json::from_str(&message).unwrap();
        assert_eq!(value["status"], "answered");
        assert!(!is_permission_grant(&value));
    }

    #[test]
    fn question_cap_rejects_a_fourth_question() {
        for index in 0..MAX_QUESTIONS_PER_RUN {
            begin("run-cap", &format!("q-{index}"), "again?", &[], false).unwrap();
        }
        assert!(matches!(
            begin("run-cap", "q-over", "again?", &[], false),
            Err(AskUserError::CapExceeded)
        ));
    }

    #[test]
    fn begin_rejects_empty_question_and_option_overflow() {
        assert!(matches!(
            begin("run-invalid", "q-empty", "   ", &[], false),
            Err(AskUserError::Invalid(_))
        ));
        let options: Vec<String> = (0..=MAX_OPTIONS)
            .map(|index| format!("opt-{index}"))
            .collect();
        assert!(matches!(
            begin("run-invalid", "q-options", "pick one?", &options, false),
            Err(AskUserError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn wait_for_answer_reads_the_stored_host_text() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let question_id = format!("wait-{}", std::process::id());
        let cancel_for_answer = cancel.clone();
        let id = question_id.clone();
        tokio::spawn(async move {
            let _ = cancel_for_answer;
            assert!(!answer(&id, "host-token"));
        });
        let text = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            wait_for_answer(&question_id, &cancel),
        )
        .await
        .expect("host answer")
        .expect("stored answer");
        assert_eq!(text, "host-token");
    }

    #[test]
    fn answer_and_cancel_return_false_for_unknown_question_ids() {
        assert!(!answer("missing-question", "nope"));
        assert!(!cancel("missing-question"));
    }

    #[test]
    fn is_permission_grant_honors_legacy_approved_field() {
        let metadata = serde_json::json!({"approved": true});
        assert!(is_permission_grant(&metadata));
        assert!(!is_permission_grant(
            &serde_json::json!({"permission_grant": false})
        ));
    }
}
