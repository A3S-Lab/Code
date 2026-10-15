//! `_a3s/form/request` — the agent asks the client to render an OpenUI Lang
//! form and waits for the submitted values. Mirrors the document bridge's
//! ext-method round trip, but open-ended: a form can stay on screen until the
//! user submits or the session dies, so there is no call timeout.

use agent_client_protocol::{AgentSideConnection, Client, ExtRequest};
use serde_json::{json, Value};

pub const FORM_REQUEST_METHOD: &str = "a3s/form/request";

/// Send one form request. Returns the submitted values JSON, or `None` when
/// the client cannot serve forms or the session ended first.
pub async fn request_form(
    client: &std::rc::Rc<AgentSideConnection>,
    question_id: &str,
    question: &str,
    form: &str,
) -> Option<Value> {
    let params = json!({
        "formId": question_id,
        "prompt": question,
        "spec": form,
        "format": "openui",
    });
    let params = serde_json::value::to_raw_value(&params).ok()?;
    let response = client
        .ext_method(ExtRequest::new(FORM_REQUEST_METHOD, params.into()))
        .await
        .ok()?;
    let values = response.0.get();
    submitted_values(values)
}

fn submitted_values(response: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(response).ok()?;
    let values = parsed.get("values").cloned()?;
    if !values.is_object() {
        return None;
    }
    Some(values)
}

#[cfg(test)]
#[path = "form_request_tests.rs"]
mod tests;
