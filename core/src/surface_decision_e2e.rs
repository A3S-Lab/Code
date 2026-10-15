//! Gateway decision through OpenUI projection.
//!
//! Each case posts the same body the agent loop posts, against a local
//! stand-in for `POST /v1/systemone`. The returned trace is what the `apofasi`
//! tool row shows, and `form.openui` is what Studio compiles.

use super::decide_tool_surface;
use crate::llm::GatewayDecisionAuth;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LEAVE: &str =
    r#"{"root":"leave_card","state":{"form":{"cm_content":"去医院","cm_type":"病假","days":2}}}"#;

const LEAVE_FORM: &str = r#"n0 = Input("cm_content", "cm_content", "去医院", "text")
n1 = Input("cm_type", "cm_type", "病假", "text")
n2 = Input("days", "days", 2, "number")
submit = Button("submit", "提交")
root = Card("leave_card", [n0, n1, n2, submit])"#;

const CHART: &str = r#"{"title":"本周","labels":["一","二"],"values":[1,2],"note":"已汇总完成"}"#;

/// No `state.form`. The gateway chooses the surface.
const BUDGET: &str = r#"{"title":"预算","note":"请确认金额","days":5,"daily":300}"#;

const BUDGET_VIEW: &str = r#"n0 = Metric("daily", 300, "")
n1 = Metric("days", 5, "")
n2 = Text("note: 请确认金额")
root = Card("预算", [n0, n1, n2])"#;

const BUDGET_FORM: &str = r#"n0 = Input("daily", "daily", 300, "number")
n1 = Input("days", "days", 5, "number")
n2 = Input("note", "note", "请确认金额", "text")
submit = Button("submit", "提交")
root = Card("预算", [n0, n1, n2, submit])"#;

const CHART_VIEW: &str = r#"n0 = BarChart(["一", "二"], [1, 2])
n1 = Text("note: 已汇总完成")
root = Card("本周", [n0, n1])"#;

fn auth(base: &str) -> GatewayDecisionAuth {
    GatewayDecisionAuth {
        base_url: base.to_owned(),
        api_key: "test-key".to_owned(),
    }
}

fn no_credential() -> GatewayDecisionAuth {
    GatewayDecisionAuth {
        base_url: String::new(),
        api_key: String::new(),
    }
}

fn lexical(choice: &str) -> Value {
    json!({
        "model": "apofasi-lexical-9",
        "answers": { "surface": { "type": "choice", "choice": choice } }
    })
}

async fn gateway(body: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;
    server
}

async fn posted_body(server: &MockServer) -> Value {
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
    requests[0].body_json().expect("json body")
}

#[tokio::test]
async fn a_declared_form_renders_without_asking_the_gateway() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let trace = decide_tool_surface(LEAVE, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.exit_code, 0);
    assert_eq!(trace.output, "declared=form");
    let form = trace.form.expect("surface");
    assert!(form.interactive);
    assert_eq!(form.openui, LEAVE_FORM);
    assert_eq!(
        form.model_note,
        "Structured data is on screen as Intelligent UI. Do not repeat its JSON."
    );

    let hyphen = r#"{"root":"leave-card","state":{"form":{"cm_content":"去医院","cm_type":"病假","days":2}}}"#;
    let trace = decide_tool_surface(hyphen, Some(auth(&server.uri())))
        .await
        .expect("trace");
    assert_eq!(trace.output, "declared=form");
    assert!(trace.form.expect("surface").openui.contains("leave-card"));
}

const WEEK_VIEW: &str = r#"{"root":"week","title":"本周","state":{"view":{"labels":["一","二"],"values":[1,2],"note":"已汇总完成"}}}"#;

#[tokio::test]
async fn a_wrapped_declaration_stays_declared_when_the_envelope_has_siblings() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let output = format!(r#"{{"ok":true,"status":"ready","data":{LEAVE}}}"#);
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn a_later_form_declaration_outranks_an_earlier_envelope_document() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let output = json!({
        "answer": BUDGET,
        "data": serde_json::from_str::<Value>(LEAVE).expect("leave"),
    })
    .to_string();
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);

    let output = json!({
        "answer": serde_json::from_str::<Value>(WEEK_VIEW).expect("week"),
        "data": serde_json::from_str::<Value>(LEAVE).expect("leave"),
    })
    .to_string();
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn a_small_form_outranks_richer_view_declarations() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let mut output = String::new();
    for _ in 0..8 {
        output.push_str(WEEK_VIEW);
        output.push('\n');
    }
    output.push_str(LEAVE);
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn a_declared_view_renders_without_a_credential() {
    let trace = decide_tool_surface(WEEK_VIEW, None).await.expect("trace");
    assert_eq!(trace.exit_code, 0);
    assert_eq!(trace.output, "declared=view");
    let form = trace.form.expect("surface");
    assert!(!form.interactive);
    assert_eq!(form.openui, CHART_VIEW);
}

#[tokio::test]
async fn a_declared_form_wins_when_a_view_is_also_present() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let both = r#"{"root":"leave_card","state":{"form":{"cm_content":"去医院","cm_type":"病假","days":2},"view":{"labels":["一","二"],"values":[1,2],"note":"已汇总完成"}}}"#;
    let trace = decide_tool_surface(both, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    let form = trace.form.expect("surface");
    assert!(form.interactive);
    assert_eq!(form.openui, LEAVE_FORM);
    assert!(!form.openui.contains("BarChart"));
}

#[tokio::test]
async fn an_empty_view_declaration_still_needs_a_credential() {
    let empty = r#"{"root":"budget","title":"预算","state":{"view":{},"note":"请确认金额","days":5,"daily":300}}"#;
    let trace = decide_tool_surface(empty, Some(no_credential()))
        .await
        .expect("trace");
    assert!(trace.form.is_none());
    assert_eq!(trace.exit_code, 1);
    assert_eq!(trace.output, "unavailable: no gateway credential");
}

#[tokio::test]
async fn a_declared_form_that_cannot_be_projected_does_not_ask_the_gateway() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let chart_form =
        r#"{"root":"week","title":"本周","state":{"form":{"labels":["一","二"],"values":[1,2]}}}"#;
    let trace = decide_tool_surface(chart_form, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert!(trace.form.is_none());
    assert_eq!(trace.exit_code, 1);
    assert_eq!(
        trace.output,
        "unavailable: declared surface could not be projected"
    );
    assert_eq!(
        trace.model_note.as_deref(),
        Some(super::DECLARED_SURFACE_REFUSAL)
    );
}

#[tokio::test]
async fn a_view_without_root_is_not_a_declared_view() {
    let missing_root = r#"{"title":"本周","state":{"view":{"labels":["一","二"],"values":[1,2],"note":"已汇总完成"}}}"#;
    let trace = decide_tool_surface(missing_root, Some(no_credential()))
        .await
        .expect("trace");
    assert!(trace.form.is_none());
    assert_eq!(trace.output, "unavailable: no gateway credential");

    let one_field = r#"{"root":"week","state":{"view":{"only":"hi"}}}"#;
    let trace = decide_tool_surface(one_field, Some(no_credential()))
        .await
        .expect("trace");
    assert_eq!(trace.output, "declared=view");
    assert!(!trace.form.expect("surface").interactive);

    let blank = r#"{"root":"week","state":{"view":{"only":""}}}"#;
    let trace = decide_tool_surface(blank, Some(no_credential()))
        .await
        .expect("trace");
    assert!(trace.form.is_none());
    assert_eq!(
        trace.output,
        "unavailable: declared surface could not be projected"
    );
    assert_eq!(
        trace.model_note.as_deref(),
        Some(super::DECLARED_SURFACE_REFUSAL)
    );
}

#[tokio::test]
async fn a_small_declared_form_outranks_a_larger_undeclared_document() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let mut larger = String::from(r#"{"title":"预算","note":"请确认金额","days":5,"daily":300"#);
    for index in 0..120 {
        larger.push_str(&format!(r#","n{index}":{index}"#));
    }
    larger.push('}');
    let output = format!("{larger}\n{LEAVE}");
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn an_unprojectable_declaration_does_not_hide_a_projectable_one() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let heavy =
        r#"{"root":"week","state":{"form":{"1":1,"2":2,"3":3,"4":4,"5":5,"6":6,"7":7,"8":8}}}"#;
    let output = format!("{heavy}\n{LEAVE}");
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn unprojectable_declarations_do_not_fill_the_cap_ahead_of_a_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let mut output = String::new();
    for index in 0..8 {
        output.push_str(&format!(
            r#"{{"root":"bad{index}","state":{{"form":{{"1":{index},"2":2,"3":3,"4":4,"5":5,"6":6,"7":7,"8":8}}}}}} "#
        ));
    }
    output.push_str(LEAVE);
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn braces_inside_one_value_do_not_hide_a_later_declaration() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let mut output = String::from("[");
    for index in 0..300 {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&format!(r#"{{"n":{index}}}"#));
    }
    output.push(']');
    output.push('\n');
    output.push_str(LEAVE);
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn earlier_json_values_do_not_hide_a_later_declaration() {
    let mut output = String::new();
    for index in 0..40 {
        output.push_str(&format!(r#"{{"noise":{index}}} "#));
    }
    for _ in 0..32 {
        output.push_str(BUDGET);
        output.push(' ');
    }
    output.push_str(LEAVE);
    let trace = decide_tool_surface(&output, Some(no_credential()))
        .await
        .expect("trace");
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn a_scan_cap_inside_a_multibyte_character_still_projects() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let mut output = String::from(LEAVE);
    output.push('\n');
    output.extend(std::iter::repeat_n(
        'x',
        65_535usize.saturating_sub(output.len()),
    ));
    assert_eq!(output.len(), 65_535);
    output.push('本');
    assert!(!output.is_char_boundary(65_536));
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "declared=form");
    assert_eq!(trace.form.expect("surface").openui, LEAVE_FORM);
}

#[tokio::test]
async fn braces_inside_one_value_do_not_hide_a_later_document() {
    let server = gateway(lexical("view")).await;
    let mut output = String::from("[");
    for index in 0..300 {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{}");
    }
    output.push(']');
    output.push('\n');
    output.push_str(BUDGET);
    let trace = decide_tool_surface(&output, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.output, "choice=view\nmodel=apofasi-lexical-9");
    assert_eq!(trace.form.expect("surface").openui, BUDGET_VIEW);
}

#[tokio::test]
async fn a_lexical_view_projects_openui_and_records_the_model() {
    let server = gateway(lexical("view")).await;
    let trace = decide_tool_surface(BUDGET, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;

    assert_eq!(trace.exit_code, 0);
    assert_eq!(trace.output, "choice=view\nmodel=apofasi-lexical-9");
    let form = trace.form.expect("surface");
    assert!(!form.interactive);
    assert_eq!(form.openui, BUDGET_VIEW);

    let body = posted_body(&server).await;
    assert!(body.get("model").is_none());
    assert_eq!(
        body["questions"]["surface"]["criteria"]["view"],
        "The user should see the data as an interface: text, metrics, or a chart. No input is required."
    );
    let state = body["state"].as_str().expect("state text");
    assert!(state.contains("预算"));
    assert!(state.contains("请确认金额"));
}

#[tokio::test]
async fn a_lexical_form_projects_a_submittable_card() {
    let server = gateway(lexical("form")).await;
    let trace = decide_tool_surface(BUDGET, Some(auth(&format!("{}/v1", server.uri()))))
        .await
        .expect("trace");
    server.verify().await;

    assert_eq!(trace.exit_code, 0);
    assert!(trace.output.starts_with("choice=form\n"));
    let form = trace.form.expect("surface");
    assert!(form.interactive);
    assert_eq!(form.openui, BUDGET_FORM);
}

#[tokio::test]
async fn a_chart_document_becomes_a_bar_chart_view() {
    let server = gateway(lexical("view")).await;
    let trace = decide_tool_surface(CHART, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    let form = trace.form.expect("surface");
    assert!(!form.interactive);
    assert_eq!(form.openui, CHART_VIEW);
}

#[tokio::test]
async fn prose_keeps_the_trace_and_does_not_invent_a_card() {
    let server = gateway(lexical("prose")).await;
    let trace = decide_tool_surface(BUDGET, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.exit_code, 0);
    assert_eq!(trace.output, "choice=prose\nmodel=apofasi-lexical-9");
    assert!(trace.form.is_none());
}

#[tokio::test]
async fn a_non_lexical_model_is_refused_in_the_trace() {
    let server = gateway(json!({
        "model": "supplier-model",
        "answers": { "surface": { "choice": "form" } }
    }))
    .await;
    let trace = decide_tool_surface(BUDGET, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.exit_code, 1);
    assert_eq!(
        trace.output,
        "refused: model `supplier-model` is not a lexical decision model"
    );
    assert!(trace.form.is_none());
}

#[tokio::test]
async fn an_unauthorized_gateway_is_visible_and_does_not_invent_a_card() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "code": "invalid_api_key" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let trace = decide_tool_surface(BUDGET, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.exit_code, 1);
    assert_eq!(trace.output, "unavailable: HTTP 401");
    assert!(trace.form.is_none());
}

#[tokio::test]
async fn ordinary_text_and_protocol_json_do_not_call_the_gateway() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let session = Some(auth(&server.uri()));
    assert!(decide_tool_surface("plain text", session.clone())
        .await
        .is_none());
    assert!(decide_tool_surface(
        r#"{"schema":"a3s.code.user-form.v1","status":"parked","permission_grant":false}"#,
        session,
    )
    .await
    .is_none());
    server.verify().await;
}

#[tokio::test]
async fn a_low_confidence_choice_does_not_render() {
    let server = gateway(json!({
        "model": "apofasi-lexical-9",
        "answers": { "surface": { "type": "choice", "choice": "form", "confidence": 0.12 } }
    }))
    .await;
    let trace = decide_tool_surface(BUDGET, Some(auth(&server.uri())))
        .await
        .expect("trace");
    server.verify().await;
    assert_eq!(trace.exit_code, 1);
    assert!(trace.output.starts_with("abstain: confidence 0.12"));
    assert!(trace.form.is_none());
}

#[tokio::test]
async fn an_unreachable_gateway_is_named_in_the_trace() {
    let trace = decide_tool_surface(BUDGET, Some(auth("http://127.0.0.1:1")))
        .await
        .expect("trace");
    assert_eq!(trace.exit_code, 1);
    assert_eq!(trace.output, "unavailable: gateway timed out");
    assert!(trace.form.is_none());
}
