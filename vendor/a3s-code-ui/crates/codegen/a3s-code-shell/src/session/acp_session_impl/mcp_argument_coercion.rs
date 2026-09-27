//! Coerce a proven string, object, or array mismatch in MCP arguments.
//! The original call stays unchanged. An unsafe mismatch is not partially applied.
use crate::session::mcp_servers::MCP_TOOL_NAME_DELIMITER;
use crate::session::tool_index::ToolMetadata;
use a3s_code_tools::implementations::use_tool::{USE_TOOL_NAME, UseToolInput};
use a3s_code_tools::registry::types::FinalizedToolset;
use a3s_code_tools::types::tool_io::ToolInput;
use serde::de::{DeserializeSeed, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fmt;
pub(super) const MCP_ARGUMENT_COERCION_MAX_DEPTH: usize = 32;
pub(super) const MCP_ARGUMENT_COERCION_MAX_NODES: usize = 10000;
const DEPTH_CAP: &str = "MCP_ARGUMENT_COERCION_MAX_DEPTH";
const NODE_CAP: &str = "MCP_ARGUMENT_COERCION_MAX_NODES";
const PLEASE: &str = "Please provide the correct schema next call.";
const REMINDER_BODY_MAX_CHARS: usize = 2000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConversionKind {
    ObjectToString,
    ArrayToString,
    StringToObject,
    StringToArray,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FieldNote {
    pub field: String,
    pub kind: ConversionKind,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CoercionSuccess {
    pub value: Value,
    pub notes: Vec<FieldNote>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExpectedType {
    String,
    Object,
    Array,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActualValue {
    String,
    Object,
    Array,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CoercionFailure {
    Unsafe {
        field: String,
        expected: ExpectedType,
        actual: ActualValue,
    },
    WalkCap {
        field: String,
        expected: Option<ExpectedType>,
        actual: Option<ActualValue>,
        cap_name: &'static str,
    },
}
pub(super) struct CoercionRequest<'a> {
    pub schema: &'a Value,
    pub wire_name: &'a str,
    pub qualified_name: &'a str,
    pub tool_input: &'a mut ToolInput,
    pub raw_input: &'a mut Value,
    pub toolset: &'a FinalizedToolset,
    pub reminder_tag: &'a str,
}
struct WalkState {
    nodes: usize,
    notes: Vec<FieldNote>,
}
pub(super) fn input_schema_for<'a>(
    tools: &'a [ToolMetadata],
    qualified_name: &str,
) -> Option<&'a Value> {
    tools
        .iter()
        .find(|tool| tool.qualified_name == qualified_name)
        .map(|tool| &tool.input_schema)
}
pub(super) fn target_name(wire_name: &str, tool_input: &ToolInput) -> Option<String> {
    match tool_input {
        ToolInput::UseTool(UseToolInput::Inline(invocation)) => Some(invocation.tool_name.clone()),
        ToolInput::MCPTool(_) if wire_name.contains(MCP_TOOL_NAME_DELIMITER) => {
            Some(wire_name.to_owned())
        }
        _ => None,
    }
}
pub(super) fn coerce_mcp_arguments(
    schema: &Value,
    value: Value,
) -> Result<CoercionSuccess, CoercionFailure> {
    let mut state = WalkState {
        nodes: 0,
        notes: Vec::new(),
    };
    let value = walk(&mut state, schema, value, "", 0, true)?;
    Ok(CoercionSuccess {
        value,
        notes: state.notes,
    })
}
pub(super) fn apply_mcp_argument_coercion(mut request: CoercionRequest<'_>) -> Option<String> {
    let walked = walked_value(request.wire_name, request.tool_input)?;
    let reminder_tag = request.reminder_tag;
    match coerce_mcp_arguments(request.schema, walked) {
        Ok(success) if success.notes.is_empty() => None,
        Ok(success) => {
            let qualified_name = request.qualified_name;
            let wire_name = request.wire_name;
            if !write_coerced(&mut request, success.value) {
                return Some(wrap(
                    could_not_arguments(qualified_name, reminder_tag),
                    reminder_tag,
                ));
            }
            render_coercion_reminder(&success.notes, qualified_name, wire_name, reminder_tag)
        }
        Err(failure) => Some(render_coercion_failure(
            &failure,
            request.qualified_name,
            request.wire_name,
            reminder_tag,
        )),
    }
}
pub(super) fn render_coercion_reminder(
    notes: &[FieldNote],
    tool_name: &str,
    wire_name: &str,
    reminder_tag: &str,
) -> Option<String> {
    let sentences = notes
        .iter()
        .map(|note| success_line(note, tool_name, wire_name, reminder_tag))
        .collect::<Vec<_>>();
    let first = sentences.first()?.clone();
    let mut lines = sentences;
    let last = lines.last_mut()?;
    last.push(' ');
    last.push_str(PLEASE);
    let joined = lines.join("\n");
    let body = if notes.len() > 1 && joined.chars().count() > REMINDER_BODY_MAX_CHARS {
        let omitted = notes.len() - 1;
        format!("{first} and {omitted} more fields. {PLEASE}")
    } else {
        joined
    };
    Some(wrap(body, reminder_tag))
}
pub(super) fn render_coercion_failure(
    failure: &CoercionFailure,
    tool_name: &str,
    wire_name: &str,
    reminder_tag: &str,
) -> String {
    let body = match failure {
        CoercionFailure::Unsafe {
            field,
            expected,
            actual,
        } if !field.is_empty() => could_not_field(
            tool_name,
            wire_name,
            field,
            *expected,
            *actual,
            reminder_tag,
        ),
        CoercionFailure::Unsafe { .. } => could_not_arguments(tool_name, reminder_tag),
        CoercionFailure::WalkCap {
            field,
            expected: Some(expected),
            actual: Some(actual),
            ..
        } if !field.is_empty() => could_not_field(
            tool_name,
            wire_name,
            field,
            *expected,
            *actual,
            reminder_tag,
        ),
        CoercionFailure::WalkCap { .. } => could_not_arguments(tool_name, reminder_tag),
    };
    wrap(body, reminder_tag)
}
pub(super) fn append_coercion_reminder(prompt_text: String, note: Option<&str>) -> String {
    match note {
        Some(note) if !prompt_text.is_empty() => format!("{prompt_text}\n\n{note}"),
        Some(note) => note.to_owned(),
        None => prompt_text,
    }
}
fn walked_value(wire_name: &str, tool_input: &ToolInput) -> Option<Value> {
    match tool_input {
        ToolInput::UseTool(UseToolInput::Inline(invocation)) => Some(invocation.tool_input.clone()),
        ToolInput::MCPTool(mcp) if wire_name.contains(MCP_TOOL_NAME_DELIMITER) => {
            Some(mcp.tool_input.clone())
        }
        _ => None,
    }
}
fn write_coerced(request: &mut CoercionRequest<'_>, coerced: Value) -> bool {
    let toolset = request.toolset;
    let wire_name = request.wire_name;
    match request.tool_input {
        ToolInput::UseTool(UseToolInput::Inline(invocation)) => {
            if !request.raw_input.is_object() {
                return false;
            }
            let mut projected = invocation.clone();
            projected.tool_input = coerced.clone();
            let Ok(model_arguments) =
                toolset.model_mcp_arguments(wire_name, &UseToolInput::Inline(projected))
            else {
                return false;
            };
            invocation.tool_input = coerced;
            *request.raw_input = model_arguments;
            true
        }
        ToolInput::MCPTool(mcp) => {
            *request.raw_input = coerced.clone();
            mcp.tool_input = coerced;
            true
        }
        _ => false,
    }
}
fn walk(
    state: &mut WalkState,
    schema: &Value,
    value: Value,
    path: &str,
    depth: usize,
    is_root: bool,
) -> Result<Value, CoercionFailure> {
    enter(state, path, depth)?;
    let Some(schema_type) = eligible_type(schema) else {
        charge_descendants(state, &value, depth).map_err(|cap_name| budget_stop(path, cap_name))?;
        return Ok(value);
    };
    match (schema_type, value) {
        ("string", Value::Object(_)) if is_root => Err(unsafe_mismatch(
            path,
            ExpectedType::String,
            ActualValue::Object,
        )),
        ("string", Value::Array(_)) if is_root => Err(unsafe_mismatch(
            path,
            ExpectedType::String,
            ActualValue::Array,
        )),
        ("string", Value::Object(map)) => stringify_value(
            state,
            Value::Object(map),
            path,
            depth,
            ExpectedType::String,
            ActualValue::Object,
            ConversionKind::ObjectToString,
        ),
        ("string", Value::Array(items)) => stringify_value(
            state,
            Value::Array(items),
            path,
            depth,
            ExpectedType::String,
            ActualValue::Array,
            ConversionKind::ArrayToString,
        ),
        ("string", other) => accept_unchanged(state, other, path, depth),
        ("object", Value::String(text)) => match parse_container(&text, true) {
            Some(Value::Object(map)) => {
                let walk_properties = schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some();
                if !walk_properties
                    && let Err(cap_name) = charge_children(state, map.values(), depth)
                {
                    return Err(cap_failure(path, schema, &Value::String(text), cap_name));
                }
                state.notes.push(FieldNote {
                    field: path.to_owned(),
                    kind: ConversionKind::StringToObject,
                });
                if !walk_properties {
                    return Ok(Value::Object(map));
                }
                walk_object(state, schema, map, path, depth)
            }
            _ => Err(unsafe_mismatch(
                path,
                ExpectedType::Object,
                ActualValue::String,
            )),
        },
        ("object", Value::Object(map)) => walk_object(state, schema, map, path, depth),
        ("object", Value::Array(_)) => Err(unsafe_mismatch(
            path,
            ExpectedType::Object,
            ActualValue::Array,
        )),
        ("object", other) => accept_unchanged(state, other, path, depth),
        ("array", Value::String(text)) => match parse_container(&text, false) {
            Some(Value::Array(items)) => {
                let walk_items = schema.get("items").is_some_and(Value::is_object);
                if !walk_items && let Err(cap_name) = charge_children(state, items.iter(), depth) {
                    return Err(cap_failure(path, schema, &Value::String(text), cap_name));
                }
                state.notes.push(FieldNote {
                    field: path.to_owned(),
                    kind: ConversionKind::StringToArray,
                });
                if !walk_items {
                    return Ok(Value::Array(items));
                }
                walk_array(state, schema, items, path, depth)
            }
            _ => Err(unsafe_mismatch(
                path,
                ExpectedType::Array,
                ActualValue::String,
            )),
        },
        ("array", Value::Array(items)) => walk_array(state, schema, items, path, depth),
        ("array", Value::Object(_)) => Err(unsafe_mismatch(
            path,
            ExpectedType::Array,
            ActualValue::Object,
        )),
        ("array", other) => accept_unchanged(state, other, path, depth),
        (_, other) => accept_unchanged(state, other, path, depth),
    }
}
fn accept_unchanged(
    state: &mut WalkState,
    value: Value,
    path: &str,
    depth: usize,
) -> Result<Value, CoercionFailure> {
    charge_descendants(state, &value, depth).map_err(|cap_name| budget_stop(path, cap_name))?;
    Ok(value)
}
fn walk_object(
    state: &mut WalkState,
    schema: &Value,
    mut object: Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<Value, CoercionFailure> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        charge_children(state, object.values(), depth)
            .map_err(|cap_name| budget_stop(path, cap_name))?;
        return Ok(Value::Object(object));
    };
    for (key, child) in &object {
        if properties.contains_key(key) {
            continue;
        }
        charge_children(state, std::iter::once(child), depth)
            .map_err(|cap_name| budget_stop(path, cap_name))?;
    }
    for (key, property_schema) in properties {
        let Some(child) = object.get(key).cloned() else {
            continue;
        };
        let child_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        refuse_child(state, property_schema, &child, &child_path, depth)?;
        let walked = walk(state, property_schema, child, &child_path, depth + 1, false)?;
        object.insert(key.clone(), walked);
    }
    Ok(Value::Object(object))
}
fn walk_array(
    state: &mut WalkState,
    schema: &Value,
    items: Vec<Value>,
    path: &str,
    depth: usize,
) -> Result<Value, CoercionFailure> {
    let item_schema = match schema.get("items") {
        Some(item_schema) if item_schema.is_object() => item_schema,
        _ => {
            charge_children(state, items.iter(), depth)
                .map_err(|cap_name| budget_stop(path, cap_name))?;
            return Ok(Value::Array(items));
        }
    };
    let mut walked = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let child_path = format!("{path}[{index}]");
        refuse_child(state, item_schema, &item, &child_path, depth)?;
        walked.push(walk(
            state,
            item_schema,
            item,
            &child_path,
            depth + 1,
            false,
        )?);
    }
    Ok(Value::Array(walked))
}
fn refuse_child(
    state: &WalkState,
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
) -> Result<(), CoercionFailure> {
    if depth + 1 >= MCP_ARGUMENT_COERCION_MAX_DEPTH {
        return Err(cap_failure(path, schema, value, DEPTH_CAP));
    }
    if state.nodes >= MCP_ARGUMENT_COERCION_MAX_NODES {
        return Err(cap_failure(path, schema, value, NODE_CAP));
    }
    Ok(())
}
fn stringify_value(
    state: &mut WalkState,
    value: Value,
    path: &str,
    depth: usize,
    expected: ExpectedType,
    actual: ActualValue,
    kind: ConversionKind,
) -> Result<Value, CoercionFailure> {
    if let Err(cap_name) = charge_descendants(state, &value, depth) {
        return Err(CoercionFailure::WalkCap {
            field: path.to_owned(),
            expected: Some(expected),
            actual: Some(actual),
            cap_name,
        });
    }
    let text =
        serde_json::to_string(&value).map_err(|_| unsafe_mismatch(path, expected, actual))?;
    state.notes.push(FieldNote {
        field: path.to_owned(),
        kind,
    });
    Ok(Value::String(text))
}
fn charge_children<'a>(
    state: &mut WalkState,
    children: impl IntoIterator<Item = &'a Value>,
    depth: usize,
) -> Result<(), &'static str> {
    for child in children {
        if depth + 1 >= MCP_ARGUMENT_COERCION_MAX_DEPTH {
            return Err(DEPTH_CAP);
        }
        if state.nodes >= MCP_ARGUMENT_COERCION_MAX_NODES {
            return Err(NODE_CAP);
        }
        state.nodes += 1;
        charge_descendants(state, child, depth + 1)?;
    }
    Ok(())
}
fn charge_descendants(
    state: &mut WalkState,
    value: &Value,
    depth: usize,
) -> Result<(), &'static str> {
    match value {
        Value::Array(items) => charge_children(state, items, depth),
        Value::Object(map) => charge_children(state, map.values(), depth),
        _ => Ok(()),
    }
}
fn budget_stop(path: &str, cap_name: &'static str) -> CoercionFailure {
    CoercionFailure::WalkCap {
        field: path.to_owned(),
        expected: None,
        actual: None,
        cap_name,
    }
}
fn enter(state: &mut WalkState, path: &str, depth: usize) -> Result<(), CoercionFailure> {
    if depth >= MCP_ARGUMENT_COERCION_MAX_DEPTH {
        return Err(CoercionFailure::WalkCap {
            field: path.to_owned(),
            expected: None,
            actual: None,
            cap_name: DEPTH_CAP,
        });
    }
    if state.nodes >= MCP_ARGUMENT_COERCION_MAX_NODES {
        return Err(CoercionFailure::WalkCap {
            field: path.to_owned(),
            expected: None,
            actual: None,
            cap_name: NODE_CAP,
        });
    }
    state.nodes += 1;
    Ok(())
}
fn eligible_type(schema: &Value) -> Option<&str> {
    let object = schema.as_object()?;
    if object.contains_key("anyOf")
        || object.contains_key("oneOf")
        || object.contains_key("allOf")
        || object.contains_key("$ref")
        || object.contains_key("enum")
        || object.contains_key("const")
    {
        return None;
    }
    object.get("type").and_then(Value::as_str)
}
fn parse_container(text: &str, want_object: bool) -> Option<Value> {
    let text = text.trim();
    let mut deserializer = serde_json::Deserializer::from_str(text);
    if deserializer.deserialize_any(UniqueObjectKeys).is_err() || deserializer.end().is_err() {
        return None;
    }
    let parsed = serde_json::from_str::<Value>(text).ok()?;
    match (&parsed, want_object) {
        (Value::Object(_), true) | (Value::Array(_), false) => Some(parsed),
        _ => None,
    }
}
struct UniqueObjectKeys;
impl<'de> DeserializeSeed<'de> for UniqueObjectKeys {
    type Value = ();
    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueObjectKeys)
    }
}
impl<'de> Visitor<'de> for UniqueObjectKeys {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON value")
    }
    fn visit_bool<E: Error>(self, _value: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: Error>(self, _value: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: Error>(self, _value: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: Error>(self, _value: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: Error>(self, _value: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(UniqueObjectKeys)?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key) {
                return Err(Error::custom("duplicate key"));
            }
            map.next_value_seed(UniqueObjectKeys)?;
        }
        Ok(())
    }
}
fn mismatch_at(schema: &Value, value: &Value) -> Option<(ExpectedType, ActualValue)> {
    match (eligible_type(schema)?, value) {
        ("string", Value::Object(_)) => Some((ExpectedType::String, ActualValue::Object)),
        ("string", Value::Array(_)) => Some((ExpectedType::String, ActualValue::Array)),
        ("object", Value::String(_)) => Some((ExpectedType::Object, ActualValue::String)),
        ("object", Value::Array(_)) => Some((ExpectedType::Object, ActualValue::Array)),
        ("array", Value::String(_)) => Some((ExpectedType::Array, ActualValue::String)),
        ("array", Value::Object(_)) => Some((ExpectedType::Array, ActualValue::Object)),
        _ => None,
    }
}
fn cap_failure(
    path: &str,
    schema: &Value,
    value: &Value,
    cap_name: &'static str,
) -> CoercionFailure {
    let (expected, actual) = mismatch_at(schema, value)
        .map(|(expected, actual)| (Some(expected), Some(actual)))
        .unwrap_or((None, None));
    CoercionFailure::WalkCap {
        field: path.to_owned(),
        expected,
        actual,
        cap_name,
    }
}
fn unsafe_mismatch(path: &str, expected: ExpectedType, actual: ActualValue) -> CoercionFailure {
    CoercionFailure::Unsafe {
        field: path.to_owned(),
        expected,
        actual,
    }
}
fn success_line(note: &FieldNote, tool_name: &str, wire_name: &str, tag: &str) -> String {
    let requested = requested_tool(tool_name, wire_name, tag);
    let (expected, provided, sent) = conversion_words(note.kind);
    if note.field.is_empty() {
        format!(
            "{requested} required the arguments to be {expected}, yet {provided} was provided. The system coerced and sent your value as {sent}."
        )
    } else {
        let field = escape_schema_text(&note.field, tag);
        format!(
            "{requested} required `{field}` to be {expected}, yet {provided} was provided. The system coerced and sent your value as {sent}."
        )
    }
}
fn could_not_field(
    tool_name: &str,
    wire_name: &str,
    field: &str,
    expected: ExpectedType,
    actual: ActualValue,
    tag: &str,
) -> String {
    let requested = requested_tool(tool_name, wire_name, tag);
    let expected = expected_word(expected);
    let provided = actual_word(actual);
    let field = escape_schema_text(field, tag);
    format!(
        "{requested} required `{field}` to be {expected}, yet {provided} was provided. The system could not coerce this value. {PLEASE}"
    )
}
fn could_not_arguments(tool_name: &str, tag: &str) -> String {
    let tool_name = escape_schema_text(tool_name, tag);
    format!("The system could not coerce the arguments for `{tool_name}`. {PLEASE}")
}
fn requested_tool(tool_name: &str, wire_name: &str, tag: &str) -> String {
    let tool_name = escape_schema_text(tool_name, tag);
    if wire_name == USE_TOOL_NAME {
        format!("The requested tool `{tool_name}` for this `use_tool` call")
    } else {
        format!("The requested tool `{tool_name}`")
    }
}
fn escape_schema_text(text: &str, tag: &str) -> String {
    super::reminders::escape_reminder_tags(text, tag)
}
fn conversion_words(kind: ConversionKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        ConversionKind::ObjectToString => ("a string", "a JSON object", "a JSON-encoded string"),
        ConversionKind::ArrayToString => ("a string", "a JSON array", "a JSON-encoded string"),
        ConversionKind::StringToObject => ("an object", "a JSON-encoded string", "an object"),
        ConversionKind::StringToArray => ("an array", "a JSON-encoded string", "an array"),
    }
}
fn expected_word(expected: ExpectedType) -> &'static str {
    match expected {
        ExpectedType::String => "a string",
        ExpectedType::Object => "an object",
        ExpectedType::Array => "an array",
    }
}
fn actual_word(actual: ActualValue) -> &'static str {
    match actual {
        ActualValue::String => "a string",
        ActualValue::Object => "a JSON object",
        ActualValue::Array => "a JSON array",
    }
}
fn wrap(body: String, tag: &str) -> String {
    a3s_code_tools::reminders::wrap_reminder_with_tag(&body, tag)
}
#[cfg(test)]
#[path = "mcp_argument_coercion_tests.rs"]
mod tests;
