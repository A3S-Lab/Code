//! Recover tool invocations that a model wrote into assistant text.
//!
//! Some providers return a finished text turn instead of structured
//! `tool_calls`. The invocations still have a stable markup shape. Executing
//! that markup is the tool call; leaving it in the transcript is a failed call.

use super::types::{ContentBlock, Message, ToolCall};
use serde_json::{Map, Value};

/// Turn leaked `<tool_call>` markup into structured tool-use blocks.
///
/// Existing structured calls are kept. Markup is removed from the visible
/// text either way, so a protocol fragment is never the answer.
pub(crate) fn recover_text_tool_calls(message: &mut Message) -> Vec<ToolCall> {
    if message.role != "assistant" {
        return message.tool_calls();
    }
    let already_structured = message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }));
    let mut recovered = Vec::new();
    let mut next_content = Vec::with_capacity(message.content.len());
    for block in message.content.drain(..) {
        let ContentBlock::Text { text } = block else {
            next_content.push(block);
            continue;
        };
        let (prose, calls) = split_leaked_tool_calls(&text);
        if !already_structured {
            for call in calls {
                let id = format!("text-tool-{}", recovered.len() + 1);
                recovered.push(ToolCall {
                    id: id.clone(),
                    name: call.name.clone(),
                    args: call.input.clone(),
                });
                next_content.push(ContentBlock::ToolUse {
                    id,
                    name: call.name,
                    input: call.input,
                });
            }
        }
        if !prose.is_empty() {
            next_content.push(ContentBlock::Text { text: prose });
        }
    }
    message.content = next_content;
    if already_structured {
        message.tool_calls()
    } else {
        recovered
    }
}

struct ParsedCall {
    name: String,
    input: Value,
}

fn split_leaked_tool_calls(text: &str) -> (String, Vec<ParsedCall>) {
    let mut prose = String::new();
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<tool_call>") {
        prose.push_str(&rest[..start]);
        let after = &rest[start + "<tool_call>".len()..];
        let (body, consumed) = take_call_body(after);
        if let Some(call) = parse_call_body(body) {
            calls.push(call);
        } else {
            prose.push_str("<tool_call>");
            prose.push_str(body);
        }
        rest = &after[consumed..];
    }
    prose.push_str(rest);
    let prose = prose
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (prose, calls)
}

fn take_call_body(after_open: &str) -> (&str, usize) {
    if let Some(end) = after_open.find("</tool_call>") {
        let body = after_open[..end].trim().trim_end_matches('/').trim();
        return (body, end + "</tool_call>".len());
    }
    if let Some(end) = after_open.find("/>") {
        return (after_open[..end].trim(), end + 2);
    }
    if let Some(end) = after_open.find("<tool_call>") {
        return (after_open[..end].trim(), end);
    }
    (after_open.trim(), after_open.len())
}

fn parse_call_body(body: &str) -> Option<ParsedCall> {
    let body = body.trim().trim_end_matches('/').trim();
    if body.is_empty() {
        return None;
    }
    let mut parts = body.split_whitespace();
    let name = parts.next()?.trim();
    if !is_tool_name(name) {
        return None;
    }
    let args_src = body[name.len()..].trim();
    Some(ParsedCall {
        name: name.to_string(),
        input: parse_attributes(args_src),
    })
}

fn is_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn parse_attributes(source: &str) -> Value {
    let mut map = Map::new();
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let key_start = index;
        while index < bytes.len() && is_attr_name_byte(bytes[index]) {
            index += 1;
        }
        if key_start == index || index >= bytes.len() || bytes[index] != b'=' {
            break;
        }
        let key = source[key_start..index].to_string();
        index += 1;
        let (value, next) = parse_attr_value(source, index);
        map.insert(key, value);
        index = next;
    }
    Value::Object(map)
}

fn is_attr_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn parse_attr_value(source: &str, start: usize) -> (Value, usize) {
    let bytes = source.as_bytes();
    if start >= bytes.len() {
        return (Value::String(String::new()), start);
    }
    let quote = bytes[start];
    if quote == b'"' || quote == b'\'' {
        let mut index = start + 1;
        let mut value = String::new();
        while index < bytes.len() {
            let ch = source[index..].chars().next().unwrap_or('\0');
            if ch == '\\' {
                let rest = &source[index + 1..];
                if let Some(next) = rest.chars().next() {
                    value.push(next);
                    index += ch.len_utf8() + next.len_utf8();
                    continue;
                }
            }
            if ch as u8 == quote && ch.is_ascii() {
                return (Value::String(value), index + 1);
            }
            value.push(ch);
            index += ch.len_utf8();
        }
        return (Value::String(value), source.len());
    }
    if quote == b'[' || quote == b'{' {
        if let Some(end) = matching_closer(source, start) {
            let raw = &source[start..=end];
            if let Ok(value) = serde_json::from_str(raw) {
                return (value, end + 1);
            }
            return (Value::String(raw.to_string()), end + 1);
        }
    }
    let mut index = start;
    while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    let raw = &source[start..index];
    (bare_value(raw), index)
}

fn matching_closer(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let open = bytes.get(start).copied()?;
    let close = if open == b'[' { b']' } else { b'}' };
    let mut depth = 0;
    let mut index = start;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn bare_value(raw: &str) -> Value {
    if raw == "true" {
        Value::Bool(true)
    } else if raw == "false" {
        Value::Bool(false)
    } else if raw == "null" {
        Value::Null
    } else if let Ok(number) = raw.parse::<i64>() {
        Value::from(number)
    } else if let Ok(number) = raw.parse::<f64>() {
        serde_json::Number::from_f64(number)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(raw.to_string()))
    } else {
        Value::String(raw.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_self_closing_markup_and_strips_it_from_prose() {
        let mut message = Message::assistant(
            "我来帮你查看今天的热搜。<tool_call>web_search query=\"今天热搜榜\" limit=\"15\" engines=[\"baidu\", \"sogou\"] timeout=\"30\" /></tool_call>",
        );
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web_search");
        assert_eq!(calls[0].args["query"], "今天热搜榜");
        assert_eq!(calls[0].args["limit"], "15");
        assert_eq!(calls[0].args["engines"][0], "baidu");
        assert_eq!(message.text(), "我来帮你查看今天的热搜。");
        assert!(message
            .tool_calls()
            .iter()
            .any(|call| call.name == "web_search"));
    }

    #[test]
    fn does_not_duplicate_structured_tool_calls() {
        let mut message = Message::assistant("<tool_call>web_search query=\"x\" /></tool_call>");
        message.content.push(ContentBlock::ToolUse {
            id: "call-1".to_string(),
            name: "web_search".to_string(),
            input: serde_json::json!({"query": "x"}),
        });
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call-1");
        assert!(message.text().is_empty());
    }
}
