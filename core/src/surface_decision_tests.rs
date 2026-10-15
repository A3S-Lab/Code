#![cfg(test)]

use super::{
    decide_tool_surface, decision_body, display_document, lexical_surface, openui_for,
    systemone_url, Surface,
};
use serde_json::json;

#[test]
fn protocol_and_plain_text_are_not_documents() {
    assert!(display_document("plain text").is_none());
    assert!(display_document("{\"ok\":true}").is_none());
    assert!(display_document(
        r#"{"schema":"a3s.code.user-form.v1","status":"parked","permission_grant":false}"#
    )
    .is_none());
}

#[test]
fn a_platform_card_is_one_document_shape() {
    let output = r#"已完成
```json
{"root":"leave_card","state":{"form":{"cm_content":"去医院","cm_type":"病假","days":2}}}
```"#;
    let document = display_document(output).expect("document");
    assert_eq!(document["title"], "leave_card");
    assert_eq!(document["body"]["cm_content"], "去医院");
    let form = openui_for(&document, Surface::Form).expect("form");
    assert!(form.contains("root = Card(\"leave_card\""));
    assert!(form.contains("Input("));
    assert!(form.contains("去医院"));
    assert!(form.contains("Button(\"submit\", \"提交\")"));
    let view = openui_for(&document, Surface::View).expect("view");
    assert!(view.contains("Metric("));
    assert!(view.contains("Text("));
    assert!(!view.contains("Button("));
}

#[test]
fn a_field_key_that_is_not_an_identifier_is_not_renamed() {
    let output = r#"{"root":"card","state":{"form":{"开始":"明天","days":2}}}"#;
    let document = display_document(output).expect("document");
    assert!(openui_for(&document, Surface::Form).is_none());
}

#[test]
fn a_tool_title_and_labels_become_the_card_copy() {
    let output = r#"{"root":"leave_card","title":"请假","state":{"labels":{"cm_content":"事由","cm_type":"请假类型"},"form":{"cm_content":"去医院","cm_type":"病假","days":2}}}"#;
    let document = display_document(output).expect("document");
    assert_eq!(document["title"], "请假");
    assert_eq!(document["labels"]["cm_content"], "事由");
    let form = openui_for(&document, Surface::Form).expect("form");
    assert!(form.contains("root = Card(\"请假\""));
    assert!(form.contains("事由"));
    assert!(form.contains("请假类型"));
    assert!(form.contains("Input(\"cm_content\", \"事由\""));
    assert!(form.contains("Input(\"days\", \"days\""));
}

#[test]
fn an_answer_envelope_unwraps_a_card_inside_prose() {
    let card = r#"{"root":"leave_card","state":{"form":{"cm_content":"去医院","cm_type":"病假","cm_start_time":"2026-10-10","cm_end_time":"2026-10-11"}}}"#;
    let answer = format!("您好，正在为您生成请假单...\n\n```json\n{card}\n```\n");
    let output = json!({
        "answer": answer,
        "questions": [],
        "docs": [],
        "id": ""
    })
    .to_string();
    let document = display_document(&output).expect("document");
    assert_eq!(document["title"], "leave_card");
    assert_eq!(document["body"]["cm_content"], "去医院");
    assert_eq!(document["body"]["cm_type"], "病假");
    let view = openui_for(&document, Surface::View).expect("view");
    assert!(view.contains("root = Card(\"leave_card\""));
    assert!(view.contains("去医院"));
    assert!(!view.contains("Button("));
}

#[test]
fn an_answer_envelope_unwraps_any_object() {
    let output = json!({
        "answer": r#"{"title":"旅行预算","days":5,"daily":300}"#
    })
    .to_string();
    let document = display_document(&output).expect("document");
    assert_eq!(document["title"], "旅行预算");
    assert_eq!(document["body"]["days"], 5);
}

#[test]
fn numbers_and_a_chart_use_the_catalog() {
    let output =
        r#"{"title":"本周","labels":["一","二","三"],"values":[2,5,3],"note":"已汇总完成"}"#;
    let document = display_document(output).expect("document");
    let view = openui_for(&document, Surface::View).expect("view");
    assert!(view.contains("BarChart([\"一\", \"二\", \"三\"], [2, 5, 3])"));
    assert!(view.contains("Text("));
    assert!(view.contains("已汇总完成"));
    assert!(!view.contains("labels"));
}

#[test]
fn a_bounded_value_becomes_a_slider_and_a_choice_becomes_a_select() {
    let output = r#"{
        "title":"设置",
        "limit":{"value":5,"min":1,"max":14,"step":1},
        "team":{"value":"设计","options":["设计","工程"]}
    }"#;
    let document = display_document(output).expect("document");
    let form = openui_for(&document, Surface::Form).expect("form");
    assert!(form.contains("Slider("));
    assert!(form.contains("Select("));
    assert!(form.contains("[\"设计\", \"工程\"]"));
}

#[test]
fn a_list_of_records_becomes_nested_cards() {
    let output = r#"[{"name":"甲","score":3},{"name":"乙","score":8}]"#;
    let document = display_document(output).expect("document");
    let view = openui_for(&document, Surface::View).expect("view");
    assert!(view.matches("Card(").count() >= 3);
    assert!(view.contains("Metric("));
}

#[test]
fn object_field_selection_and_control_identity_ignore_insertion_order() {
    fn document(reverse: bool) -> serde_json::Value {
        let mut fields = serde_json::Map::new();
        let mut keys: Vec<_> = (0..16).collect();
        if reverse {
            keys.reverse();
        }
        for key in keys {
            fields.insert(format!("field_{key:02}"), json!(key));
        }
        json!({"title": "bounded", "body": fields})
    }
    for surface in [Surface::Form, Surface::View] {
        let forward = openui_for(&document(false), surface).expect("forward surface");
        let reverse = openui_for(&document(true), surface).expect("reverse surface");
        assert_eq!(forward, reverse);
        for key in 0..12 {
            assert!(forward.contains(&format!("field_{key:02}")));
        }
        for key in 12..16 {
            assert!(!forward.contains(&format!("field_{key:02}")));
        }
        if surface == Surface::Form {
            assert!(forward.starts_with("n0 = Input(\"field_00\", \"field_00\", 0, \"number\")"));
            assert!(forward.contains("n11 = Input(\"field_11\", \"field_11\", 11, \"number\")"));
        }
    }
}

#[test]
fn nested_object_fields_are_sorted_without_reordering_records() {
    let body = serde_json::from_str::<serde_json::Value>(r#"[{"z":2,"a":1},{"z":4,"a":3}]"#)
        .expect("records");
    let view =
        openui_for(&json!({"title":"records", "body":body}), Surface::View).expect("record view");
    let expected = [
        "Metric(\"a\", 1, \"\")",
        "Metric(\"z\", 2, \"\")",
        "Metric(\"a\", 3, \"\")",
        "Metric(\"z\", 4, \"\")",
    ];
    let positions: Vec<_> = expected
        .iter()
        .map(|item| view.find(item).expect("original value"))
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn the_decision_asks_for_form_view_or_prose() {
    let document = display_document(r#"{"title":"预算","days":5,"daily":300}"#).expect("document");
    let body = decision_body(&document);
    assert!(body.get("model").is_none());
    assert_eq!(body["questions"]["surface"]["type"], "choice");
    assert!(body["questions"]["surface"]["criteria"]["form"].is_string());
    assert!(body["questions"]["surface"]["criteria"]["view"].is_string());
    assert!(body["questions"]["surface"]["criteria"]["prose"].is_string());
}

#[test]
fn only_a_lexical_form_or_view_is_accepted() {
    assert_eq!(
        lexical_surface(
            br#"{"model":"apofasi-lexical-1","answers":{"surface":{"choice":"form"}}}"#
        ),
        Some(Surface::Form)
    );
    assert_eq!(
        lexical_surface(
            br#"{"model":"apofasi-lexical-1","answers":{"surface":{"choice":"view"}}}"#
        ),
        Some(Surface::View)
    );
    assert!(lexical_surface(
        br#"{"model":"apofasi-lexical-1","answers":{"surface":{"choice":"prose"}}}"#
    )
    .is_none());
    assert!(lexical_surface(
        br#"{"model":"supplier-model","answers":{"surface":{"choice":"form"}}}"#
    )
    .is_none());
    assert!(lexical_surface(b"not-json").is_none());
}

#[test]
fn a_view_without_inputs_does_not_pretend_to_be_a_form() {
    let document = display_document(r#"{"title":"本周","labels":["一","二"],"values":[1,2]}"#)
        .expect("document");
    assert!(openui_for(&document, Surface::View).is_some());
    assert!(openui_for(&document, Surface::Form).is_none());
}

#[test]
fn systemone_url_uses_the_session_gateway_root() {
    assert_eq!(
        systemone_url("http://127.0.0.1:8080/v1"),
        "http://127.0.0.1:8080/v1/systemone"
    );
    assert_eq!(
        systemone_url("http://127.0.0.1:8080"),
        "http://127.0.0.1:8080/v1/systemone"
    );
}

#[tokio::test]
async fn a_document_without_a_credential_is_still_traced() {
    let trace = decide_tool_surface(
        r#"{"title":"预算","days":5,"daily":300}"#,
        Some(crate::llm::GatewayDecisionAuth {
            base_url: String::new(),
            api_key: String::new(),
        }),
    )
    .await
    .expect("trace");
    assert!(trace.form.is_none());
    assert_eq!(trace.exit_code, 1);
    assert_eq!(trace.output, "unavailable: no gateway credential");
    assert!(trace.args.get("questions").is_some());
}
