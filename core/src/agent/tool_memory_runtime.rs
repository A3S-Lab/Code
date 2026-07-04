use super::{AgentEvent, AgentLoop};
use a3s_memory::MemoryItem;
use tokio::sync::mpsc;

const MAX_MEMORY_PROMPT_CHARS: usize = 1_200;
const MAX_MEMORY_OUTPUT_CHARS: usize = 2_400;

impl AgentLoop {
    pub(super) async fn remember_tool_result(
        &self,
        effective_prompt: &str,
        tool_name: &str,
        output: &str,
        exit_code: i32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        let Some(ref memory) = self.config.memory else {
            return;
        };

        if exit_code == 0 && should_skip_success_memory(tool_name, output) {
            return;
        }
        if exit_code == 0 && memory.llm_extraction_enabled() {
            return;
        }

        let tools_used = [tool_name.to_string()];
        let prompt = compact_for_memory(effective_prompt, MAX_MEMORY_PROMPT_CHARS);
        let output = compact_for_memory(output, MAX_MEMORY_OUTPUT_CHARS);
        let remember_result = if exit_code == 0 {
            memory
                .remember_success_item(&prompt, &tools_used, &output)
                .await
        } else {
            memory
                .remember_failure_item(&prompt, &output, &tools_used)
                .await
        };

        match remember_result {
            Ok(item) => {
                if let Some(tx) = event_tx {
                    let memory_type = memory_type_label(&item).to_string();
                    tx.send(AgentEvent::MemoryStored {
                        memory_id: item.id,
                        memory_type,
                        importance: item.importance,
                        tags: item.tags,
                    })
                    .await
                    .ok();
                }
            }
            Err(e) => {
                tracing::warn!("Failed to store memory after tool execution: {}", e);
            }
        }
    }
}

fn memory_type_label(item: &MemoryItem) -> &'static str {
    match item.memory_type {
        a3s_memory::MemoryType::Episodic => "episodic",
        a3s_memory::MemoryType::Semantic => "semantic",
        a3s_memory::MemoryType::Procedural => "procedural",
        a3s_memory::MemoryType::Working => "working",
    }
}

fn compact_for_memory(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push_str("\n... (truncated for memory)");
    out
}

fn should_skip_success_memory(tool_name: &str, output: &str) -> bool {
    if output.trim().is_empty() {
        return true;
    }

    let name = tool_name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "read" | "grep" | "glob" | "ls" | "web_fetch" | "web_search"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_for_memory_preserves_short_text() {
        assert_eq!(compact_for_memory("  ok  ", 10), "ok");
    }

    #[test]
    fn compact_for_memory_truncates_long_text() {
        let text = "abcdef";
        let compact = compact_for_memory(text, 3);
        assert!(compact.starts_with("abc"));
        assert!(compact.contains("truncated for memory"));
    }

    #[test]
    fn read_only_success_memory_is_skipped() {
        assert!(should_skip_success_memory("grep", "match"));
        assert!(!should_skip_success_memory("bash", "built"));
    }
}
