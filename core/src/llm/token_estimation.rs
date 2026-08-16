use super::{ContentBlock, Message, ToolDefinition, ToolResultContent, ToolResultContentField};

/// Cheap internal prompt-token estimator shared by governed LLM and compaction paths.
pub(crate) fn estimate_prompt_tokens(
    messages: &[Message],
    system: Option<&str>,
    tools: &[ToolDefinition],
) -> usize {
    let mut chars = system.map(str::len).unwrap_or(0);
    for message in messages {
        chars = chars.saturating_add(message.role.len() + 4);
        chars = chars.saturating_add(message.reasoning_content.as_ref().map_or(0, String::len));
        for block in &message.content {
            chars = chars.saturating_add(match block {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::Image { source } => source.data.len(),
                ContentBlock::ToolUse { id, name, input } => id
                    .len()
                    .saturating_add(name.len())
                    .saturating_add(input.to_string().len()),
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => tool_use_id
                    .len()
                    .saturating_add(tool_result_content_chars(content)),
            });
        }
    }
    for tool in tools {
        chars = chars
            .saturating_add(tool.name.len())
            .saturating_add(tool.description.len())
            .saturating_add(tool.parameters.to_string().len())
            .saturating_add(16);
    }
    chars.saturating_add(3) / 4
}

pub(crate) fn estimate_message_tokens(messages: &[Message]) -> usize {
    estimate_prompt_tokens(messages, None, &[])
}

pub(crate) fn estimate_tool_result_contents_tokens<'a>(
    contents: impl IntoIterator<Item = &'a ToolResultContentField>,
) -> usize {
    let chars = contents.into_iter().fold(0usize, |total, content| {
        total.saturating_add(tool_result_content_chars(content))
    });
    chars.saturating_add(3) / 4
}

fn tool_result_content_chars(content: &ToolResultContentField) -> usize {
    match content {
        ToolResultContentField::Text(text) => text.len(),
        ToolResultContentField::Blocks(blocks) => blocks.iter().fold(0usize, |total, block| {
            total.saturating_add(match block {
                ToolResultContent::Text { text } => text.len(),
                ToolResultContent::Image { source } => source.data.len(),
            })
        }),
    }
}
