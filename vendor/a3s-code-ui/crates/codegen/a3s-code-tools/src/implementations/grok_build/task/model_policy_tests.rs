use super::*;

#[test]
fn exported_schema_hides_model_only_under_inherited_selection() {
    let schema =
        serde_json::to_value(schemars::schema_for!(a3s_tool_types::TaskToolInput)).unwrap();
    let has = |schema: &serde_json::Value, name: &str| schema["properties"].get(name).is_some();

    let selectable = exported_input_schema(&schema, TaskModelSelection::Selectable);
    assert!(has(&selectable, "model"));

    let hidden = exported_input_schema(&schema, TaskModelSelection::Inherited);
    assert!(!has(&hidden, "model"));
    assert!(has(&hidden, "prompt"));
}
