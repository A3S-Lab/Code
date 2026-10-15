//! Doubled fullwidth bars and a space before the tag name are still DSML.

use a3s_code_core::llm::recover_leaked_tool_calls;

#[test]
fn doubled_bar_dsml_with_a_space_is_a_bash_call() {
    let raw = "<\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} calls>\n<\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} invoke name=\"bash\">\n<\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} parameter name=\"command\" string=\"true\">sleep 30</\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} parameter>\n<\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} parameter name=\"timeout\" string=\"false\">600</\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} parameter>\n</\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} invoke>\n</\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} calls>";
    let calls = recover_leaked_tool_calls(raw);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "bash");
    assert_eq!(calls[0].2["command"], "sleep 30");
    assert_eq!(calls[0].2["timeout"], 600);
}

#[test]
fn doubled_bar_dsml_without_timeout_is_a_bash_call() {
    let raw = "<\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} calls>\n<\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} invoke name=\"bash\">\n<\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} parameter name=\"command\" string=\"true\">sleep 30</\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} parameter>\n</\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} invoke>\n</\u{FF5C}\u{FF5C}DSML\u{FF5C}\u{FF5C} calls>";
    assert_eq!(raw.chars().count(), 162);
    assert_eq!(raw.chars().filter(|ch| *ch == '\u{FF5C}').count(), 24);
    let calls = recover_leaked_tool_calls(raw);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "bash");
    assert_eq!(calls[0].2["command"], "sleep 30");
    assert!(calls[0].2.get("timeout").is_none());
}
