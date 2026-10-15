#![cfg(test)]
use super::*;

#[test]
fn accepts_openui_and_refuses_mdx_or_json() {
    let form = "root = Card(\"Details\", [reason, submit])\nreason = Input(\"reason\", \"Reason\")\nsubmit = Button(\"submit\", \"Continue\")";
    assert!(check_form_shape(form).is_ok());
    for source in [
        "",
        "  ",
        "not a form",
        r#"<Card title="Details"></Card>"#,
        r#"{"root":"f","elements":{"f":{}}}"#,
    ] {
        assert!(check_form_shape(source).is_err());
    }
}

#[test]
fn oversized_openui_is_rejected() {
    assert!(check_form_shape(&format!("root = Card(\"{}\")", "x".repeat(MAX_FORM_BYTES))).is_err());
}

const FORM: &str = "root = Card(\"Details\", [name, go])\nname = Input(\"name\", \"Name\")\ngo = Button(\"submit\", \"Continue\")";

async fn run_form(answer: &'static str) -> ToolOutput {
    let root = tempfile::tempdir().unwrap();
    let session = format!("form-{}", uuid::Uuid::new_v4());
    let (event_tx, mut events) = tokio::sync::broadcast::channel(4);
    let ctx = ToolContext::new(root.path().to_path_buf())
        .with_session_id(&session)
        .with_agent_event_tx(event_tx);
    let host = tokio::spawn(async move {
        match events.recv().await.unwrap() {
            AgentEvent::UserQuestion {
                question_id, form, ..
            } => {
                assert_eq!(form.as_deref(), Some(FORM));
                ask_user::answer(&question_id, answer);
            }
            other => panic!("expected a form question, got {other:?}"),
        }
    });
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        AskUserFormTool.execute(&serde_json::json!({ "form": FORM }), &ctx),
    )
    .await
    .expect("the form call returns once the host answers")
    .unwrap();
    host.await.unwrap();
    output
}

#[tokio::test]
async fn submitted_values_are_the_tool_result() {
    let output = run_form(r#"{"name":"e2e","include":true}"#).await;
    assert!(output.success, "{}", output.content);
    let result: serde_json::Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(result["status"], "answered");
    assert_eq!(
        result["values"],
        serde_json::json!({"name":"e2e","include":true})
    );
    assert_eq!(result["permission_grant"], false);
}

#[tokio::test]
async fn a_host_without_values_fails_the_call_instead_of_hanging() {
    let output = run_form("null").await;
    assert!(!output.success);
    assert!(
        output.content.contains("no form values"),
        "{}",
        output.content
    );
}
