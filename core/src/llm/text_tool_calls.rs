//! Recover tool invocations that a model wrote into assistant text.
//!
//! Some providers return a finished text turn instead of structured
//! `tool_calls`. The invocations still have a stable markup shape. Executing
//! that markup is the tool call; leaving it in the transcript is a failed call.
//!
//! Dialects recovered here:
//! - attribute form: `<tool_call>name key="…" /></tool_call>`
//! - WorkBuddy bare: `<tool_call>name>…</name>` / `</tool_call>`
//! - WorkBuddy tagged (hy3/`auto`): `<tool_call:id>name">…</invoke>`
//! - Claude-style: `<invoke name="…">…</invoke>`
//! - DeepSeek DSML (V3.2/V4): `<｜DSML｜invoke name="…">…</｜DSML｜invoke>`
//!   (ASCII `||DSML||` accepted as a fallback for mangled streams)

use super::types::{ContentBlock, Message, ToolCall};
use serde_json::{Map, Value};

/// Canonical DeepSeek DSML namespace token (`｜` is U+FF5C).
const DSML_NS: &str = "\u{FF5C}DSML\u{FF5C}";
/// ASCII fallback seen when fullwidth pipes are mangled in transit/UI.
const DSML_NS_ASCII: &str = "||DSML||";

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
                let id = call
                    .id
                    .unwrap_or_else(|| format!("text-tool-{}", recovered.len() + 1));
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

/// Strip tool-protocol markup from assistant text without recovering calls.
///
/// Used by hosts that accumulate streamed deltas before recovery runs, so
/// Auto-review does not treat DSML/`<tool_call>` blobs as product prose.
pub fn strip_leaked_tool_protocol(text: &str) -> String {
    let (prose, _) = split_leaked_tool_calls(text);
    prose
}

struct ParsedCall {
    id: Option<String>,
    name: String,
    input: Value,
}

#[derive(Clone, Copy)]
enum MarkupKind {
    Tagged,
    OpenToolCall,
    Invoke,
    DsmlInvoke,
}

fn split_leaked_tool_calls(text: &str) -> (String, Vec<ParsedCall>) {
    let mut prose = String::new();
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some((kind, start)) = find_next_markup(rest) {
        prose.push_str(&rest[..start]);
        let at = &rest[start..];
        match take_markup(kind, at) {
            Some((call, consumed)) => {
                calls.push(call);
                rest = &at[consumed..];
            }
            None => {
                // Advance one byte so a malformed fragment cannot loop forever.
                let advance = at.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                prose.push_str(&at[..advance]);
                rest = &at[advance..];
            }
        }
    }
    prose.push_str(rest);
    let prose = scrub_wrapper_markup(&prose)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (prose, calls)
}

fn find_next_markup(text: &str) -> Option<(MarkupKind, usize)> {
    let tagged = text.find("<tool_call:").map(|i| (MarkupKind::Tagged, i));
    let open = text
        .find("<tool_call>")
        .map(|i| (MarkupKind::OpenToolCall, i));
    let invoke = text.find("<invoke ").map(|i| (MarkupKind::Invoke, i));
    let dsml = find_dsml_invoke(text).map(|i| (MarkupKind::DsmlInvoke, i));
    [tagged, open, invoke, dsml]
        .into_iter()
        .flatten()
        .min_by_key(|(_, i)| *i)
}

fn find_dsml_invoke(text: &str) -> Option<usize> {
    let full = format!("<{DSML_NS}invoke ");
    let ascii = format!("<{DSML_NS_ASCII}invoke ");
    [text.find(&full), text.find(&ascii)]
        .into_iter()
        .flatten()
        .min()
}

fn take_markup(kind: MarkupKind, at: &str) -> Option<(ParsedCall, usize)> {
    match kind {
        MarkupKind::Tagged => take_workbuddy_tagged(at),
        MarkupKind::OpenToolCall => {
            let after = &at["<tool_call>".len()..];
            if looks_like_workbuddy_bare(after) {
                take_workbuddy_bare(at)
            } else {
                take_attribute_tool_call(at)
            }
        }
        MarkupKind::Invoke => take_claude_invoke(at),
        MarkupKind::DsmlInvoke => take_dsml_invoke(at),
    }
}

/// `<｜DSML｜invoke name="search">…</｜DSML｜invoke>` (V3.2/V4).
fn take_dsml_invoke(at: &str) -> Option<(ParsedCall, usize)> {
    let ns = if at.starts_with(&format!("<{DSML_NS}invoke ")) {
        DSML_NS
    } else if at.starts_with(&format!("<{DSML_NS_ASCII}invoke ")) {
        DSML_NS_ASCII
    } else {
        return None;
    };
    let header_end = at.find('>')?;
    let header = &at[..=header_end];
    let name = xml_attr(header, "name")?;
    if !is_tool_name(&name) {
        return None;
    }
    let body_start = header_end + 1;
    let after_header = &at[body_start..];
    let close = format!("</{ns}invoke>");
    let body_end = after_header.find(&close)?;
    let body = &after_header[..body_end];
    let consumed = body_start + body_end + close.len();
    Some((
        ParsedCall {
            id: None,
            name,
            input: Value::Object(parse_dsml_parameter_tags(body, ns)),
        },
        consumed,
    ))
}

fn parse_dsml_parameter_tags(body: &str, ns: &str) -> Map<String, Value> {
    let mut params = Map::new();
    let open = format!("<{ns}parameter ");
    let close = format!("</{ns}parameter>");
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start..];
        let Some(header_end) = rest.find('>') else {
            break;
        };
        let header = &rest[..=header_end];
        let value_start = header_end + 1;
        let Some(value_end) = rest[value_start..].find(&close) else {
            break;
        };
        if let Some(name) = xml_attr(header, "name") {
            let raw = decode_xml_entities(rest[value_start..value_start + value_end].trim());
            let as_string = xml_attr(header, "string")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let value = if as_string {
                Value::String(raw)
            } else {
                parse_parameter_value(&raw)
            };
            params.insert(name, value);
        }
        rest = &rest[value_start + value_end + close.len()..];
    }
    if params.is_empty() {
        let trimmed = body.trim();
        if trimmed.starts_with('{') {
            if let Ok(Value::Object(map)) = serde_json::from_str(trimmed) {
                return map;
            }
        }
    }
    params
}

/// `<tool_call:call_1>ls"><parameter …></invoke>`
fn take_workbuddy_tagged(at: &str) -> Option<(ParsedCall, usize)> {
    if !at.starts_with("<tool_call:") {
        return None;
    }
    let after_prefix = &at["<tool_call:".len()..];
    let id_end = after_prefix.find('>')?;
    let id = after_prefix[..id_end].trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        return None;
    }
    let after_id = &after_prefix[id_end + 1..];
    let name_end = after_id.find("\">")?;
    let name = decode_xml_entities(after_id[..name_end].trim());
    if !is_tool_name(&name) {
        return None;
    }
    let body_start = name_end + "\">".len();
    let after_name = &after_id[body_start..];
    let body_end = after_name.find("</invoke>")?;
    let body = &after_name[..body_end];
    let consumed = "<tool_call:".len() + id_end + 1 + body_start + body_end + "</invoke>".len();
    Some((
        ParsedCall {
            id: Some(id.to_string()),
            name,
            input: Value::Object(parse_parameter_tags(body)),
        },
        consumed,
    ))
}

/// `<tool_call>write><parameter …></write>`
fn take_workbuddy_bare(at: &str) -> Option<(ParsedCall, usize)> {
    if !at.starts_with("<tool_call>") {
        return None;
    }
    let after = &at["<tool_call>".len()..];
    let name_end = after.find('>')?;
    let name = decode_xml_entities(after[..name_end].trim());
    if !is_tool_name(&name) {
        return None;
    }
    let body_start = name_end + 1;
    let body_region = &after[body_start..];
    let close_named = format!("</{name}>");
    let (body_end, close_len) = if let Some(end) = body_region.find(&close_named) {
        (end, close_named.len())
    } else {
        let end = body_region.find("</tool_call>")?;
        (end, "</tool_call>".len())
    };
    let body = &body_region[..body_end];
    let consumed = "<tool_call>".len() + body_start + body_end + close_len;
    Some((
        ParsedCall {
            id: None,
            name,
            input: Value::Object(parse_parameter_tags(body)),
        },
        consumed,
    ))
}

fn looks_like_workbuddy_bare(after_open: &str) -> bool {
    let trimmed = after_open.trim_start();
    let Some(gt) = trimmed.find('>') else {
        return false;
    };
    let name = trimmed[..gt].trim();
    is_tool_name(name) && !name.contains('=') && !name.contains(char::is_whitespace)
}

fn take_attribute_tool_call(at: &str) -> Option<(ParsedCall, usize)> {
    if !at.starts_with("<tool_call>") {
        return None;
    }
    let after = &at["<tool_call>".len()..];
    let (body, body_consumed) = take_call_body(after);
    let call = parse_call_body(body)?;
    Some((call, "<tool_call>".len() + body_consumed))
}

fn take_claude_invoke(at: &str) -> Option<(ParsedCall, usize)> {
    if !at.starts_with("<invoke ") {
        return None;
    }
    let header_end = at.find('>')?;
    let header = &at[..=header_end];
    let name = xml_attr(header, "name")?;
    if !is_tool_name(&name) {
        return None;
    }
    let body_start = header_end + 1;
    let after_header = &at[body_start..];
    let body_end = after_header.find("</invoke>")?;
    let body = &after_header[..body_end];
    let consumed = body_start + body_end + "</invoke>".len();
    Some((
        ParsedCall {
            id: None,
            name,
            input: Value::Object(parse_parameter_tags(body)),
        },
        consumed,
    ))
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
        id: None,
        name: name.to_string(),
        input: parse_attributes(args_src),
    })
}

fn parse_parameter_tags(body: &str) -> Map<String, Value> {
    let mut params = Map::new();
    let mut rest = body;
    while let Some(start) = rest.find("<parameter ") {
        rest = &rest[start..];
        let Some(header_end) = rest.find('>') else {
            break;
        };
        let header = &rest[..=header_end];
        let value_start = header_end + 1;
        let Some(value_end) = rest[value_start..].find("</parameter>") else {
            break;
        };
        if let Some(name) = xml_attr(header, "name") {
            let raw = decode_xml_entities(rest[value_start..value_start + value_end].trim());
            params.insert(name, parse_parameter_value(&raw));
        }
        rest = &rest[value_start + value_end + "</parameter>".len()..];
    }
    params
}

fn xml_attr(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(decode_xml_entities(&rest[..end]))
}

fn decode_xml_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn parse_parameter_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn scrub_wrapper_markup(prose: &str) -> String {
    let mut out = prose.to_string();
    for pattern in [
        "<function_calls>",
        "</function_calls>",
        "</tool_calls>",
        "</invoke>",
        "<minimax:tool_call>",
        "</minimax:tool_call>",
    ] {
        out = out.replace(pattern, "");
    }
    // Drop `<tool_calls:group_id>` open wrappers.
    while let Some(start) = out.find("<tool_calls:") {
        let Some(end) = out[start..].find('>') else {
            break;
        };
        out.replace_range(start..start + end + 1, "");
    }
    // Drop DSML outer wrappers: `<｜DSML｜tool_calls>` / `</｜DSML｜tool_calls>` etc.
    for ns in [DSML_NS, DSML_NS_ASCII] {
        for name in ["tool_calls", "function_calls", "calls"] {
            let open = format!("<{ns}{name}>");
            let close = format!("</{ns}{name}>");
            out = out.replace(&open, "").replace(&close, "");
        }
        out = out.replace(&format!("</{ns}invoke>"), "");
        out = out.replace(&format!("</{ns}parameter>"), "");
    }
    out
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

    #[test]
    fn recovers_workbuddy_tagged_tool_call_xml() {
        let mut message = Message::assistant(
            r#"I'll list the workspace.<tool_calls:group_1>
<tool_call:call_1>ls">
<parameter name="path">/work/a3s</parameter>
</invoke>
</function_calls>"#,
        );
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "ls");
        assert_eq!(calls[0].args["path"], "/work/a3s");
        assert_eq!(message.text(), "I'll list the workspace.");
        assert!(!message.text().contains("<tool_call"));
        assert!(!message.text().contains("</invoke>"));
    }

    #[test]
    fn recovers_workbuddy_bare_tool_call_xml_with_named_close() {
        let mut message = Message::assistant(
            r#"Let me save this to memory.<tool_call>write>
<parameter name="file_path">NOTE.md</parameter>
<parameter name="content">mem_wb_token_violet_91</parameter>
</write>"#,
        );
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write");
        assert_eq!(calls[0].args["file_path"], "NOTE.md");
        assert_eq!(calls[0].args["content"], "mem_wb_token_violet_91");
        assert_eq!(message.text(), "Let me save this to memory.");
    }

    #[test]
    fn recovers_claude_invoke_markup() {
        let mut message = Message::assistant(
            r#"Checking.<function_calls>
<invoke name="Read">
<parameter name="file_path">README.md</parameter>
</invoke>
</function_calls>"#,
        );
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "Read");
        assert_eq!(calls[0].args["file_path"], "README.md");
        assert_eq!(message.text(), "Checking.");
    }

    #[test]
    fn attribute_form_still_preferred_over_bare_false_positive() {
        let mut message =
            Message::assistant(r#"<tool_call>web_search query="today" limit="5" /></tool_call>"#);
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web_search");
        assert_eq!(calls[0].args["query"], "today");
        assert_eq!(calls[0].args["limit"], "5");
    }
    #[test]
    fn recovers_deepseek_dsml_v4_tool_calls() {
        let mut message = Message::assistant(
            "正在检索。\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"search\">\n<｜DSML｜parameter name=\"mode\" string=\"true\">glob</｜DSML｜parameter>\n<｜DSML｜parameter name=\"query\" string=\"true\">测试一下</｜DSML｜parameter>\n<｜DSML｜parameter name=\"path\" string=\"true\">.a3s/kb/wiki/science</｜DSML｜parameter>\n<｜DSML｜parameter name=\"limit\" string=\"true\">8</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>",
        );
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].args["mode"], "glob");
        assert_eq!(calls[0].args["query"], "测试一下");
        assert_eq!(calls[0].args["path"], ".a3s/kb/wiki/science");
        assert_eq!(calls[0].args["limit"], "8");
        assert_eq!(message.text(), "正在检索。");
        assert!(!message.text().contains("DSML"));
    }

    #[test]
    fn recovers_ascii_mangled_dsml_namespace() {
        let mut message = Message::assistant(
            r#"<||DSML||calls><||DSML||invoke name="search"><||DSML||parameter name="query" string="true">x</||DSML||parameter></||DSML||invoke></||DSML||calls>"#,
        );
        let calls = recover_text_tool_calls(&mut message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].args["query"], "x");
        assert!(message.text().is_empty());
    }

    #[test]
    fn strip_leaked_tool_protocol_drops_dsml_only_blobs() {
        let raw = r#"<｜DSML｜tool_calls><｜DSML｜invoke name="search"><｜DSML｜parameter name="query" string="true">q</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜tool_calls>"#;
        assert!(strip_leaked_tool_protocol(raw).is_empty());
        assert_eq!(strip_leaked_tool_protocol(&format!("hello {raw}")), "hello");
    }
}
