#![cfg(test)]

use super::*;

#[test]
fn oversized_newest_result_reaches_summarizer_with_bounded_head_and_tail() {
    let latest = format!("HEAD-CONTEXT-71{}TAIL-CONTEXT-82", "x".repeat(10_000));
    let lines = vec!["tool\nolder result".into(), format!("tool\n{latest}")];
    let pruned = prune_folded_tool_outputs(&lines, 64);
    assert_eq!(pruned[0], format!("tool\n{PRUNED_MARKER}"));
    let body = pruned[1].strip_prefix("tool\n").unwrap();
    assert!(body.len() <= 64 * 4);
    assert!(body.contains("HEAD-CONTEXT-71"));
    assert!(body.contains("TAIL-CONTEXT-82"));
    assert!(body.contains("output compacted"));
}

#[test]
fn oversized_newest_error_preserves_role_and_utf8_boundaries() {
    let body = format!("头部事实{}尾部事实", "汉字🙂".repeat(200));
    let pruned = prune_folded_tool_outputs(&[format!("tool-error\n{body}")], 64);
    let body = pruned[0].strip_prefix("tool-error\n").unwrap();
    assert!(body.len() <= 64 * 4);
    assert!(body.starts_with("头部事实"));
    assert!(body.ends_with("尾部事实"));
}

#[test]
fn tiny_positive_protection_budget_is_still_a_hard_content_bound() {
    for budget in [1, 2, 8, 16] {
        let pruned = prune_folded_tool_outputs(&[format!("tool\n{}", "🙂".repeat(300))], budget);
        assert!(pruned[0].strip_prefix("tool\n").unwrap().len() <= budget * 4);
    }
}

#[test]
fn zero_budget_prunes_and_fitting_newest_result_keeps_existing_policy() {
    let lines = vec![
        format!("tool\n{}", "x".repeat(10_000)),
        "tool\nrecent".into(),
    ];
    let pruned = prune_folded_tool_outputs(&lines, 64);
    assert_eq!(pruned[0], format!("tool\n{PRUNED_MARKER}"));
    assert_eq!(pruned[1], lines[1]);
    assert!(prune_folded_tool_outputs(&lines, 0)
        .iter()
        .all(|line| line == &format!("tool\n{PRUNED_MARKER}")));
}

#[test]
fn exact_budget_preserves_original_output_bytes() {
    let lines = vec!["user\nquestion".into(), "tool\n12345678".into()];
    assert_eq!(prune_folded_tool_outputs(&lines, 2), lines);
}
