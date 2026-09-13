//! Product-transcript helpers for conversation messages.
//!
//! Model-wire history may carry composed Desktop context, planner chrome, and
//! runtime steering. The product transcript must prefer the human-authored
//! sentence. Keep this parser conservative: unwrap explicit bridge markers and
//! strip known host appendices; never invent paraphrases.

/// Extract the human-authored task from a possibly composed / planner-wrapped
/// prompt. Returns empty when the text is pure runtime steering with no human
/// sentence to recover.
pub fn product_user_text(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut working = normalized.as_str();

    working = strip_prefix_fence(working, "```open-reply-review-findings");

    let planner_markers = [
        "\nPlanner-optimized request:",
        "\n规划优化请求：",
        "\n规划优化请求:",
        "\n优化后的请求：",
        "\n优化后的请求:",
        // PrePlanning hooks dual-mark like the planner; keep only the Original /
        // human prefix for product transcript.
        "\nHook-modified planning task:",
        "\nPlanning hook guidance:",
    ];
    for marker in planner_markers {
        if let Some((before, _)) = working.split_once(marker) {
            working = before;
        }
    }

    if let Some((before, _)) = working.split_once("\n\nAutomatic subagent context:\n") {
        working = before;
    }

    // PrePrompt hooks may append model-only context. Never surface it in the
    // product user bubble — strip both well-formed blocks and a dangling open tag.
    working = strip_user_prompt_hook_context(working);

    // Prefer the last explicit human marker. Desktop follow-ups use
    // "Latest user message:"; first turns use "User task:".
    let user_markers = [
        "\nLatest user message:\n",
        "\nLatest user message:",
        "\n最新用户消息：",
        "\n最新用户消息:",
        "\nUser task:\n",
        "\nUser task:",
        "\n用户任务：",
        "\n用户任务:",
    ];
    let mut task = working;
    for marker in user_markers {
        if let Some((_, after)) = working.rsplit_once(marker) {
            task = after;
            break;
        }
    }
    if std::ptr::eq(task, working) {
        let leading_markers = [
            "Latest user message:\n",
            "Latest user message:",
            "最新用户消息：",
            "最新用户消息:",
            "User task:\n",
            "User task:",
            "用户任务：",
            "用户任务:",
        ];
        for marker in leading_markers {
            if let Some(after) = working.strip_prefix(marker) {
                task = after;
                break;
            }
        }
    }

    let trimmed = task
        .trim()
        .trim_start_matches("Original user request:")
        .trim_start_matches("Original user request：")
        .trim();

    if is_pure_runtime_steering(trimmed) || trimmed.is_empty() {
        String::new()
    } else {
        trimmed.to_string()
    }
}

/// True when `text` should be stored as a wire-only user turn.
pub fn is_wire_only_user_text(text: &str) -> bool {
    product_user_text(text).is_empty() && !text.trim().is_empty()
}

fn strip_prefix_fence<'a>(text: &'a str, fence: &str) -> &'a str {
    let trimmed = text.trim_start();
    if !trimmed.starts_with(fence) {
        return text;
    }
    let after_open = &trimmed[fence.len()..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    if let Some((_, rest)) = after_open.split_once("\n```") {
        let rest = rest.trim_start_matches('\n');
        if rest.is_empty() {
            text
        } else {
            rest
        }
    } else {
        text
    }
}

fn strip_user_prompt_hook_context(text: &str) -> &str {
    const OPEN: &str = "\n\n<user-prompt-hook-context>\n";
    if let Some(start) = text.find(OPEN) {
        // Hooks append this block at the end of the wire prompt. Keep the
        // human prefix whether or not the close tag is present.
        return text[..start].trim_end();
    }
    if let Some(start) = text.find("<user-prompt-hook-context>") {
        return text[..start].trim_end();
    }
    text
}

fn is_pure_runtime_steering(text: &str) -> bool {
    let text = text.trim_start();
    if text.is_empty() {
        return false;
    }
    (text.starts_with("Delegated plan step '") && text.contains(" failed:\n"))
        || (text.starts_with("Tool '")
            && (text.contains(" execution was REJECTED")
                || text.contains(" confirmation failed:")
                || text.contains(" cancelled by caller")))
        || text.starts_with("Delegated parallel plan wave completed with failures:")
        || (text.starts_with("Goal:") && text.contains("Execute the following plan step by step:"))
        || text.starts_with("Execute step ")
        || text.starts_with("Execute the following plan step by step:")
        || text.starts_with("[Context Summary:")
        || text.starts_with("Tool-use budget reached.")
        || text.starts_with('{') && text.contains("\"type\":\"parallel_results\"")
        || text.starts_with("Address each open review finding")
}

#[cfg(test)]
mod tests {
    use super::{is_wire_only_user_text, product_user_text};

    #[test]
    fn unwraps_desktop_user_task() {
        let text = "A3S Desktop workbench context:\n- Active workbench: Office.\n\nUser task:\n规划如何开发一个游戏引擎？";
        assert_eq!(product_user_text(text), "规划如何开发一个游戏引擎？");
        assert!(!is_wire_only_user_text(text));
    }

    #[test]
    fn unwraps_desktop_latest_user_message() {
        let text = "A3S Desktop workbench context:\n- Active workbench: Office.\n\nLatest user message:\n再试试";
        assert_eq!(product_user_text(text), "再试试");
    }

    #[test]
    fn unwraps_planner_wrapper_with_latest_user_message() {
        let text = "Original user request:\nA3S Desktop workbench context:\nLatest user message:\n再试试\n\nPlanner-optimized request:\n再试试\n当前请求仅表示“再试试”，需先澄清。";
        assert_eq!(product_user_text(text), "再试试");
    }

    #[test]
    fn strips_pre_prompt_hook_context_appendix() {
        let text = "Ship the release\n\n<user-prompt-hook-context>\npolicy note\n</user-prompt-hook-context>";
        assert_eq!(product_user_text(text), "Ship the release");
    }

    #[test]
    fn strips_pre_planning_hook_appendix() {
        let text = [
            "Original user request:",
            "A3S Desktop workbench context:",
            "User task:",
            "Ship the release",
            "",
            "Hook-modified planning task:",
            "Rewrite the release checklist first",
            "",
            "Planning hook guidance:",
            "Selected strategy: careful",
        ]
        .join("\n");
        assert_eq!(product_user_text(&text), "Ship the release");
    }

    #[test]
    fn keeps_human_skill_slash_spelling_from_desktop_compose() {
        let text = "A3S Desktop workbench context:\n- Active workbench: Office.\n\nUser task:\n/a3s-box run nginx";
        assert_eq!(product_user_text(text), "/a3s-box run nginx");
    }

    #[test]
    fn unwraps_planner_optimized_wrapper() {
        let text = "Original user request:\nA3S Desktop workbench context:\nUser task: 工作区有哪些文件？\nPlanner-optimized request:\nlist files";
        assert_eq!(product_user_text(text), "工作区有哪些文件？");
    }

    #[test]
    fn strips_auto_delegation_appendix() {
        let text = "Ship the release\n\nAutomatic subagent context:\n{\"tasks\":[]}\n\nUse the subagent findings as evidence, but make the final decision yourself.";
        assert_eq!(product_user_text(text), "Ship the release");
    }

    #[test]
    fn blanks_plan_execute_goal_chrome() {
        let text = "Goal: ship it\n\nExecute the following plan step by step:\n1. Draft\n2. Review";
        assert_eq!(product_user_text(text), "");
        assert!(is_wire_only_user_text(text));
    }

    #[test]
    fn blanks_plan_execute_step_chrome() {
        let text = "Execute step 2: Draft the architecture";
        assert_eq!(product_user_text(text), "");
        assert!(is_wire_only_user_text(text));
    }

    #[test]
    fn blanks_compaction_summary_chrome() {
        let text = "[Context Summary: prior turns compressed]\nKeep working.";
        assert_eq!(product_user_text(text), "");
        assert!(is_wire_only_user_text(text));
    }

    #[test]
    fn unwraps_open_reply_review_findings_fence() {
        let text =
            "```open-reply-review-findings\nDATA\n```\nUser task:\nFix the failing assertion";
        assert_eq!(product_user_text(text), "Fix the failing assertion");
    }

    #[test]
    fn message_constructors_segregate_wire_and_transcript() {
        use crate::llm::Message;

        let product = Message::user("hello");
        assert!(product.is_product_transcript());
        assert_eq!(product.transcript_display_text(), "hello");

        let wire = Message::user_wire("Execute step 1: Survey");
        assert!(!wire.is_product_transcript());
        assert_eq!(wire.text(), "Execute step 1: Survey");

        let dual = Message::user_for_model_with_transcript(
            "A3S Desktop workbench context:\n\nUser task:\n规划引擎",
            "规划引擎",
        );
        assert!(dual.is_product_transcript());
        assert_eq!(dual.transcript_display_text(), "规划引擎");
        assert!(dual.text().contains("User task:"));

        let address_reply = Message::assistant_wire("Finding f1: rebutted with tool exit 0");
        assert!(!address_reply.is_product_transcript());
        assert_eq!(
            address_reply.text(),
            "Finding f1: rebutted with tool exit 0"
        );
    }
}
