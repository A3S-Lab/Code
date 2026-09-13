//! Structured question that parks a run without a write, a permission grant,
//! or a host steer.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio::sync::oneshot;

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

struct Pending {
    tx: oneshot::Sender<AskUserResume>,
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
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

pub fn begin(
    run_id: &str,
    question_id: &str,
    question: &str,
    options: &[String],
    allow_free_text: bool,
) -> Result<(UserQuestionV1, oneshot::Receiver<AskUserResume>), AskUserError> {
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
    // The caller prefix is not an identity. A second question in the same run
    // must not replace the parked oneshot or receive its answer.
    let question_id = format!("{question_id}-{sequence}");
    let (tx, rx) = oneshot::channel();
    let mut pending = pending().lock().expect("ask-user pending");
    if pending.contains_key(&question_id) {
        return Err(AskUserError::Invalid("question already pending"));
    }
    pending.insert(question_id.clone(), Pending { tx });
    drop(pending);
    Ok((
        UserQuestionV1 {
            schema: USER_QUESTION_SCHEMA.to_string(),
            question_id,
            question: question.to_string(),
            options: options.to_vec(),
            allow_free_text,
        },
        rx,
    ))
}

pub fn answer(question_id: &str, text: &str) -> bool {
    let Some(pending) = pending()
        .lock()
        .expect("ask-user pending")
        .remove(question_id)
    else {
        return false;
    };
    pending
        .tx
        .send(AskUserResume::Answered {
            text: text.to_string(),
        })
        .is_ok()
}

pub fn cancel(question_id: &str) -> bool {
    let Some(pending) = pending()
        .lock()
        .expect("ask-user pending")
        .remove(question_id)
    else {
        return false;
    };
    pending.tx.send(AskUserResume::Unanswered).is_ok()
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

    #[tokio::test]
    async fn answer_is_structured_and_is_not_a_permission_grant() {
        let (question, rx) = begin("run-1", "q-1", "Which file?", &["a.rs".into()], false).unwrap();
        assert_eq!(question.schema, USER_QUESTION_SCHEMA);
        assert!(answer(&question.question_id, "a.rs"));
        let resume = rx.await.unwrap();
        let message = resume_message(&question.question_id, &resume);
        let value: Value = serde_json::from_str(&message).unwrap();
        assert_eq!(value["status"], "answered");
        assert_eq!(value["answer"], "a.rs");
        assert!(!is_permission_grant(&value));
        assert_ne!(question.schema, "confirmation_required");
        assert_ne!(question.schema, "run_control_applied");
    }

    #[tokio::test]
    async fn cancel_resumes_as_unanswered_not_approval() {
        let (question, rx) = begin("run-2", "q-2", "Continue?", &[], true).unwrap();
        assert!(cancel(&question.question_id));
        let resume = rx.await.unwrap();
        let value: Value =
            serde_json::from_str(&resume_message(&question.question_id, &resume)).unwrap();
        assert_eq!(value["status"], "unanswered");
        assert!(!is_permission_grant(&value));
    }

    #[tokio::test]
    async fn second_question_does_not_steal_the_first_slot() {
        let (first, mut first_rx) =
            begin("run-steal", "ask-run-steal-0", "one?", &[], true).unwrap();
        let (second, second_rx) = begin("run-steal", "ask-run-steal-0", "two?", &[], true).unwrap();
        assert_ne!(
            first.question_id, second.question_id,
            "two questions in one run must not share a slot"
        );
        assert!(
            matches!(
                first_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "starting the second question must not cancel the first"
        );
        assert!(answer(&second.question_id, "later"));
        match second_rx.await.unwrap() {
            AskUserResume::Answered { text } => assert_eq!(text, "later"),
            AskUserResume::Unanswered => panic!("the second question was not answered"),
        }
        assert!(
            matches!(
                first_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "answering the second question must not resume the first"
        );
        assert!(answer(&first.question_id, "earlier"));
        match first_rx.await.unwrap() {
            AskUserResume::Answered { text } => assert_eq!(text, "earlier"),
            AskUserResume::Unanswered => panic!("the first question was stolen"),
        }
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
}
