//! Obligations that survive compaction even when the summary model drops them.
//!
//! Standing project and personal instructions stay out of the summarizer input
//! and are copied back unchanged. A later explicit `Revision:` replaces the
//! active goal; the previous goal remains history. Unrevoked user constraints
//! and required steps that are still open are taken from the transcript, not
//! from the summary's claim that the work is finished.

use crate::llm::{ContentBlock, Message};

const STANDING_HEADER: &str = "# Instructions (personal + project AGENTS.md chain)";
const STANDING_END: &str = "<!-- /standing -->";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CarriedObligations {
    pub standing: Vec<String>,
    pub active_goal: Option<String>,
    pub earlier_goal: Option<String>,
    pub constraints: Vec<String>,
    pub open_steps: Vec<String>,
    pub completed_steps: Vec<String>,
}

#[derive(Clone, Debug)]
struct Step {
    text: String,
    open: bool,
}

/// Text the next model call sees: standing instructions, then each compacted
/// message. This is the session system prefix plus the messages compaction
/// just produced.
pub(crate) fn next_turn_context(
    prefix_instructions: &str,
    compacted_messages: &[String],
) -> String {
    let mut parts = Vec::new();
    let prefix = prefix_instructions.trim();
    if !prefix.is_empty() {
        parts.push(prefix.to_string());
    }
    for message in compacted_messages {
        let text = message.trim();
        if text.is_empty() {
            continue;
        }
        if parts.last().is_some_and(|existing| existing == text) {
            continue;
        }
        parts.push(text.to_string());
    }
    parts.join("\n\n")
}

pub(crate) fn obligations_from_messages(messages: &[Message]) -> CarriedObligations {
    let mut carried = CarriedObligations::default();
    let mut revisions = Vec::new();
    for message in messages {
        absorb_message(&mut carried, &mut revisions, message);
    }
    finish(&mut carried, messages, &revisions);
    carried
}

pub(crate) fn obligations_from_folded(lines: &[String]) -> CarriedObligations {
    let mut carried = CarriedObligations::default();
    let mut revisions = Vec::new();
    for line in lines {
        absorb_folded(&mut carried, &mut revisions, line);
    }
    let messages = lines
        .iter()
        .filter_map(|line| folded_as_message(line))
        .collect::<Vec<_>>();
    finish(&mut carried, &messages, &revisions);
    carried
}

/// Put carried obligations after the summary model returns. The model's own
/// Goal, constraint, and step sections are removed so a contradictory summary
/// cannot replace the transcript.
pub(crate) fn seal_summary(model_summary: &str, carried: &CarriedObligations) -> String {
    let body = strip_model_sections(model_summary);
    let mut parts = Vec::new();
    for block in &carried.standing {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        parts.push(format!("{block}\n\n{STANDING_END}"));
    }
    if let Some(goal) = nonempty(carried.active_goal.as_deref()) {
        parts.push(format!("## Goal\n{goal}"));
    }
    if let Some(earlier) = nonempty(carried.earlier_goal.as_deref()) {
        parts.push(format!("## Earlier goal\n{earlier}"));
    }
    if !carried.constraints.is_empty() {
        parts.push(format!(
            "## Constraints\n{}",
            bullet_list(&carried.constraints)
        ));
    }
    if !carried.open_steps.is_empty() {
        parts.push(format!(
            "## Open steps\n{}",
            bullet_list(&carried.open_steps)
        ));
    }
    if !carried.completed_steps.is_empty() {
        parts.push(format!(
            "## Completed steps\n{}",
            bullet_list(&carried.completed_steps)
        ));
    }
    if let Some(body) = nonempty(Some(body.trim())) {
        if body.starts_with("## ") {
            parts.push(body.to_string());
        } else {
            parts.push(format!("## Summary\n{body}"));
        }
    }
    parts.join("\n\n")
}

/// A message that is only the standing instruction document. Summarizer input
/// skips these so the model cannot rewrite them.
pub(crate) fn is_pure_standing_message(message: &Message) -> bool {
    is_pure_standing(&message.text())
}

pub(crate) fn is_pure_standing_line(line: &str) -> bool {
    let body = folded_body(line);
    is_pure_standing(body)
}

/// Fallback summaries must stay short. A huge first user turn is not reattached
/// as the goal; structured obligations still are.
pub(crate) fn cap_fallback_goals(carried: &mut CarriedObligations) {
    const MAX_FALLBACK_GOAL: usize = 2_000;
    if carried
        .active_goal
        .as_ref()
        .is_some_and(|goal| goal.len() > MAX_FALLBACK_GOAL)
    {
        carried.active_goal = None;
    }
    if carried
        .earlier_goal
        .as_ref()
        .is_some_and(|goal| goal.len() > MAX_FALLBACK_GOAL)
    {
        carried.earlier_goal = None;
    }
}

fn finish(carried: &mut CarriedObligations, messages: &[Message], revisions: &[String]) {
    let baseline = baseline_goal(messages, revisions);
    if let Some(revision) = revisions.last().cloned() {
        carried.earlier_goal = baseline.filter(|goal| goal != &revision);
        carried.active_goal = Some(revision);
    } else {
        carried.active_goal = baseline;
    }
}

fn absorb_message(
    carried: &mut CarriedObligations,
    revisions: &mut Vec<String>,
    message: &Message,
) {
    let text = message.text();
    if let Some(block) = extract_standing(&text) {
        push_unique(&mut carried.standing, block);
    }
    if !is_pure_standing(&text) {
        absorb_sealed_sections(carried, &text);
        if message.role == "user" && message.is_product_transcript() {
            absorb_user_prose(carried, revisions, &text);
        }
    }
    for block in &message.content {
        if let ContentBlock::ToolUse { name, input, .. } = block {
            if name == "update_plan" {
                if let Some(steps) = steps_from_plan_value(input) {
                    apply_steps(carried, &steps);
                }
            }
        }
    }
    // `/compact` renders model.turn calls as assistant prose (`Tool call …`)
    // and turns a kept `assistant\ntool update_plan` line into the same role.
    // The sealed summary is the only text the next turn keeps.
    if message.role == "assistant" {
        absorb_plan_prose(carried, &text);
    }
}

fn absorb_folded(carried: &mut CarriedObligations, revisions: &mut Vec<String>, line: &str) {
    if let Some(json) = line.strip_prefix("assistant\ntool update_plan\n") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
            if let Some(steps) = steps_from_plan_value(&value) {
                apply_steps(carried, &steps);
            }
        }
        return;
    }
    let (role, body) = split_folded(line);
    if let Some(block) = extract_standing(body) {
        push_unique(&mut carried.standing, block);
    }
    if is_pure_standing(body) {
        return;
    }
    absorb_sealed_sections(carried, body);
    if role == "user" {
        absorb_user_prose(carried, revisions, body);
    }
}

fn absorb_sealed_sections(carried: &mut CarriedObligations, text: &str) {
    if let Some(section) = section_body(text, "## Constraints") {
        carried.constraints = bullet_items(&section);
    }
    if text.contains("\n## Open steps")
        || text.contains("\n## Completed steps")
        || text.starts_with("## Open steps")
        || text.starts_with("## Completed steps")
    {
        carried.open_steps = section_body(text, "## Open steps")
            .map(|section| bullet_items(&section))
            .unwrap_or_default();
        carried.completed_steps = section_body(text, "## Completed steps")
            .map(|section| bullet_items(&section))
            .unwrap_or_default();
    }
    if let Some(earlier) = section_body(text, "## Earlier goal") {
        if !earlier.is_empty() {
            carried.earlier_goal = Some(earlier);
        }
    }
}

fn absorb_user_prose(carried: &mut CarriedObligations, revisions: &mut Vec<String>, text: &str) {
    if let Some(steps) = required_steps(text) {
        apply_steps(carried, &steps);
    }
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(revision) = revision_text(line) {
            revisions.push(revision);
            continue;
        }
        match constraint_edit(line) {
            Some(ConstraintEdit::Add(constraint)) => {
                push_unique(&mut carried.constraints, constraint)
            }
            Some(ConstraintEdit::Revoke(token)) => {
                carried
                    .constraints
                    .retain(|constraint| !constraint_matches(constraint, &token));
            }
            None => {}
        }
    }
    if let Some(body) = section_body(text, "## Revision") {
        if !body.is_empty() {
            revisions.push(body);
        }
    }
}

fn apply_steps(carried: &mut CarriedObligations, steps: &[Step]) {
    carried.open_steps = steps
        .iter()
        .filter(|step| step.open)
        .map(|step| step.text.clone())
        .collect();
    carried.completed_steps = steps
        .iter()
        .filter(|step| !step.open)
        .map(|step| step.text.clone())
        .collect();
}

fn required_steps(text: &str) -> Option<Vec<Step>> {
    if !text.to_ascii_lowercase().contains("required steps") {
        return None;
    }
    let mut steps = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(item) = line
            .strip_prefix("- [ ] ")
            .or_else(|| line.strip_prefix("* [ ] "))
        {
            let item = item.trim();
            if !item.is_empty() {
                steps.push(Step {
                    text: item.to_string(),
                    open: true,
                });
            }
        } else if let Some(item) = line
            .strip_prefix("- [x] ")
            .or_else(|| line.strip_prefix("- [X] "))
            .or_else(|| line.strip_prefix("* [x] "))
            .or_else(|| line.strip_prefix("* [X] "))
        {
            let item = item.trim();
            if !item.is_empty() {
                steps.push(Step {
                    text: item.to_string(),
                    open: false,
                });
            }
        }
    }
    (!steps.is_empty()).then_some(steps)
}

fn absorb_plan_prose(carried: &mut CarriedObligations, text: &str) {
    let trimmed = text.trim();
    if let Some(json) = trimmed.strip_prefix("tool update_plan\n") {
        if let Some(steps) = steps_from_plan_json(json) {
            apply_steps(carried, &steps);
            return;
        }
    }
    for line in trimmed.lines() {
        let Some(rest) = line.trim().strip_prefix("Tool call update_plan (") else {
            continue;
        };
        let Some((_, json)) = rest.split_once("): ") else {
            continue;
        };
        if let Some(steps) = steps_from_plan_json(json) {
            apply_steps(carried, &steps);
        }
    }
}

fn steps_from_plan_json(json: &str) -> Option<Vec<Step>> {
    let value = serde_json::from_str::<serde_json::Value>(json.trim()).ok()?;
    steps_from_plan_value(&value)
}

fn steps_from_plan_value(input: &serde_json::Value) -> Option<Vec<Step>> {
    let rows = input.get("plan")?.as_array()?;
    if rows.is_empty() {
        return None;
    }
    let mut steps = Vec::new();
    for row in rows {
        let text = row
            .get("step")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|step| !step.is_empty())?;
        let status = row
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("pending");
        steps.push(Step {
            text: text.to_string(),
            open: step_is_open(status),
        });
    }
    Some(steps)
}

fn step_is_open(status: &str) -> bool {
    !matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "completed" | "complete" | "done" | "skipped" | "cancelled" | "canceled"
    )
}

enum ConstraintEdit {
    Add(String),
    Revoke(String),
}

fn constraint_edit(line: &str) -> Option<ConstraintEdit> {
    if let Some(rest) = strip_label(line, "Revoke:") {
        let rest = rest.trim().trim_end_matches('.').trim();
        return (!rest.is_empty()).then(|| ConstraintEdit::Revoke(rest.to_string()));
    }
    if let Some(rest) = strip_label(line, "Constraint:") {
        let rest = rest.trim();
        return (!rest.is_empty()).then(|| ConstraintEdit::Add(line.trim().to_string()));
    }
    if strip_label(line, "Must not:").is_some() || strip_label(line, "Must:").is_some() {
        return Some(ConstraintEdit::Add(line.trim().to_string()));
    }
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("must not ") || lower.starts_with("must ") {
        return Some(ConstraintEdit::Add(line.trim().to_string()));
    }
    None
}

fn constraint_matches(constraint: &str, token: &str) -> bool {
    let constraint = constraint.trim();
    let token = token.trim();
    if constraint.eq_ignore_ascii_case(token) {
        return true;
    }
    let unlabeled = constraint
        .split_once(':')
        .map(|(_, rest)| rest.trim().trim_end_matches('.').trim())
        .unwrap_or(constraint);
    unlabeled.eq_ignore_ascii_case(token)
        || constraint
            .to_ascii_lowercase()
            .contains(&token.to_ascii_lowercase())
}

fn revision_text(line: &str) -> Option<String> {
    let rest = strip_label(line, "Revision:")?.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

fn strip_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let line = line.trim();
    // Labels are ASCII. A multibyte character that overlaps `label.len()`
    // cannot be that prefix, and slicing there panics.
    if line.len() < label.len() || !line.is_char_boundary(label.len()) {
        return None;
    }
    if line[..label.len()].eq_ignore_ascii_case(label) {
        Some(&line[label.len()..])
    } else {
        None
    }
}

fn extract_standing(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let without_marker = trimmed
        .strip_prefix(crate::prompts::CONTEXT_SUMMARY_PREFIX.trim())
        .map(str::trim)
        .unwrap_or(trimmed);
    let start = without_marker.find(STANDING_HEADER)?;
    let from = &without_marker[start..];
    let end = from
        .find(&format!("\n{STANDING_END}"))
        .or_else(|| from.find("\n## Goal"))
        .unwrap_or(from.len());
    let block = from[..end].trim();
    (!block.is_empty()).then(|| block.to_string())
}

fn is_pure_standing(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with(STANDING_HEADER) && !trimmed.contains("\n## ")
}

fn section_body(text: &str, heading: &str) -> Option<String> {
    let start = text.find(heading)?;
    let after = &text[start + heading.len()..];
    let after = after.strip_prefix('\r').unwrap_or(after);
    let after = after.strip_prefix('\n').unwrap_or(after);
    let end = after
        .find("\n## ")
        .or_else(|| after.find("\n# "))
        .unwrap_or(after.len());
    Some(after[..end].trim().to_string())
}

fn bullet_items(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            let item = line.strip_prefix("- ")?.trim();
            (!item.is_empty()).then(|| item.to_string())
        })
        .collect()
}

fn bullet_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_model_sections(text: &str) -> String {
    let mut working = text.trim().to_string();
    if let Some(block) = extract_standing(&working) {
        working = working.replace(&block, "");
        working = working.replace(STANDING_END, "");
    }
    for heading in [
        "## Goal",
        "## Earlier goal",
        "## Constraints",
        "## Open steps",
        "## Completed steps",
        "## Summary",
        "## Revision",
    ] {
        working = strip_heading(&working, heading);
    }
    working
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_heading(text: &str, heading: &str) -> String {
    let Some(start) = text.find(heading) else {
        return text.to_string();
    };
    let after = &text[start + heading.len()..];
    let after = after.strip_prefix('\r').unwrap_or(after);
    let after = after.strip_prefix('\n').unwrap_or(after);
    let rest_rel = after
        .find("\n## ")
        .or_else(|| after.find("\n# "))
        .unwrap_or(after.len());
    let prefix = text[..start].trim();
    let suffix = if rest_rel < after.len() {
        after[rest_rel..].trim_start_matches('\n').trim()
    } else {
        ""
    };
    [prefix, suffix]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn folded_body(line: &str) -> &str {
    split_folded(line).1
}

fn split_folded(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_prefix("user\n") {
        ("user", body)
    } else if let Some(body) = line.strip_prefix("assistant\n") {
        ("assistant", body)
    } else if let Some(body) = line.strip_prefix("tool-error\n") {
        ("tool", body)
    } else if let Some(body) = line.strip_prefix("tool\n") {
        ("tool", body)
    } else {
        ("user", line)
    }
}

fn folded_as_message(line: &str) -> Option<Message> {
    if line.starts_with("assistant\ntool ")
        || line.starts_with("tool\n")
        || line.starts_with("tool-error\n")
    {
        return None;
    }
    let (role, body) = split_folded(line);
    if role != "user" {
        return None;
    }
    Some(Message::user(body))
}

fn push_unique(items: &mut Vec<String>, item: String) {
    if item.is_empty() || items.iter().any(|existing| existing == &item) {
        return;
    }
    items.push(item);
}

fn nonempty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

/// Goal pin used when no later revision exists, and as history when one does.
/// Revision lines and pure standing documents are not the baseline.
pub(crate) fn baseline_goal(messages: &[Message], revisions: &[String]) -> Option<String> {
    let filtered = messages
        .iter()
        .filter(|message| {
            let text = message.text();
            !is_pure_standing(&text) && revision_text_in(&text).is_none()
        })
        .cloned()
        .collect::<Vec<_>>();
    let goal = crate::compaction::extract_pinned_goal(&filtered)?;
    if revisions.iter().any(|revision| revision == &goal) {
        None
    } else {
        Some(goal)
    }
}

fn revision_text_in(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(revision) = revision_text(line) {
            return Some(revision);
        }
    }
    section_body(text, "## Revision").filter(|body| !body.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_scan_survives_a_multibyte_char_before_the_label_width() {
        // U+2013 occupies bytes 7..10, so a 9-byte ASCII label must not slice it.
        let unicode = format!("abcdefg\u{2013}more");
        assert!(!unicode.is_char_boundary(9));
        let carried = obligations_from_messages(&[Message::user(&unicode)]);
        assert!(carried.constraints.is_empty());
        let carried = obligations_from_folded(&[format!("user\n{unicode}")]);
        assert!(carried.constraints.is_empty());

        let carried = obligations_from_messages(&[Message::user("Revision: keep the pin")]);
        assert_eq!(carried.active_goal.as_deref(), Some("keep the pin"));
    }

    #[test]
    fn seam_admission_compaction_carry() {
        let standing = format!(
            "{STANDING_HEADER}\nKeep the repository building.\n{STANDING_END}"
        );
        let user = "\
Revision: ship the permission seam
Constraint: do not skip the completion gate
Constraint: drop this later
Revoke: drop this later
Required steps:
- [ ] admit the policy on the next run
- [x] trace the call sites";
        let carried = obligations_from_messages(&[Message::user(&standing), Message::user(user)]);
        assert!(carried
            .standing
            .iter()
            .any(|block| block.contains("Keep the repository building.")));
        assert_eq!(
            carried.active_goal.as_deref(),
            Some("ship the permission seam")
        );
        assert!(carried
            .constraints
            .iter()
            .any(|constraint| constraint.contains("do not skip the completion gate")));
        assert!(carried
            .constraints
            .iter()
            .all(|constraint| !constraint.to_ascii_lowercase().contains("drop this later")));
        assert_eq!(
            carried.open_steps,
            vec!["admit the policy on the next run".to_string()]
        );
        assert!(carried
            .completed_steps
            .iter()
            .any(|step| step == "trace the call sites"));

        let sealed = seal_summary(
            "The task is finished and the constraints are gone.",
            &carried,
        );
        assert!(sealed.contains("Keep the repository building."));
        assert!(sealed.contains("ship the permission seam"));
        assert!(sealed.contains("do not skip the completion gate"));
        assert!(sealed.contains("admit the policy on the next run"));
        assert!(!sealed.to_ascii_lowercase().contains("drop this later"));
    }
}
