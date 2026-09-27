//! Exclusive flat MCP invocation forms; file documents use canonical keys only.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
};

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, Visitor},
};
use serde_json::Value;

const FIELDS: &[&str] = &["tool_name", "tool_input", "tool_input_file", "file"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InlineMcpInvocation {
    pub tool_name: String,
    pub tool_input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum UseToolInput {
    Inline(InlineMcpInvocation),
    ArgumentsFile {
        tool_name: String,
        tool_input_file: PathBuf,
    },
    InvocationFile {
        file: PathBuf,
    },
}

impl UseToolInput {
    pub fn target_name(&self) -> Option<&str> {
        match self {
            UseToolInput::Inline(input) => Some(&input.tool_name),
            UseToolInput::ArgumentsFile { tool_name, .. } => Some(tool_name),
            UseToolInput::InvocationFile { .. } => None,
        }
    }

    pub fn source_path(&self) -> Option<&Path> {
        match self {
            UseToolInput::Inline(_) => None,
            UseToolInput::ArgumentsFile {
                tool_input_file, ..
            } => Some(tool_input_file),
            UseToolInput::InvocationFile { file } => Some(file),
        }
    }

    /// Parse original model JSON before duplicate keys can be discarded.
    ///
    /// # Errors
    /// Rejects invalid JSON, ambiguous mapped keys, and mixed or missing forms.
    pub fn from_model_json(
        json: &str,
        reverse: &HashMap<String, String>,
    ) -> Result<Self, serde_json::Error> {
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let input = deserializer.deserialize_map(InputVisitor { reverse })?;
        deserializer.end()?;
        Ok(input)
    }

    /// Parse a canonical invocation document without legacy inline normalization.
    ///
    /// # Errors
    /// Rejects non-inline envelopes and non-object remote arguments.
    pub fn from_invocation_file(json: &str) -> Result<InlineMcpInvocation, String> {
        let input: UseToolInput = serde_json::from_str(json).map_err(|error| {
            format!(
                "Invalid canonical MCP invocation JSON at line {}, column {}",
                error.line(),
                error.column()
            )
        })?;
        match input {
            UseToolInput::Inline(input) if input.tool_input.is_object() => Ok(input),
            _ => Err("Invocation file requires canonical tool_name and object tool_input; file delegation is not supported".to_owned()),
        }
    }
}

/// Parse a complete remote argument object; nested strings remain strings.
///
/// # Errors
/// Rejects malformed JSON, trailing content, and non-object roots.
pub fn parse_arguments_file(json: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(json).map_err(|error| {
        format!(
            "Invalid MCP arguments JSON at line {}, column {}",
            error.line(),
            error.column()
        )
    })?;
    if !value.is_object() {
        return Err("MCP arguments file must contain a JSON object".to_owned());
    }
    Ok(value)
}

impl<'de> Deserialize<'de> for UseToolInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(InputVisitor {
            reverse: &HashMap::new(),
        })
    }
}

struct InputVisitor<'a> {
    reverse: &'a HashMap<String, String>,
}

impl<'de> Visitor<'de> for InputVisitor<'_> {
    type Value = UseToolInput;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("exactly one inline, arguments-file, or invocation-file MCP call")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let canonical = self.reverse.get(&key).map_or(key.as_str(), String::as_str);
            if FIELDS.contains(&canonical) {
                if values.contains_key(canonical) {
                    return Err(de::Error::custom(
                        "duplicate or ambiguous MCP wrapper field",
                    ));
                }
                values.insert(canonical.to_owned(), map.next_value::<Value>()?);
            } else {
                map.next_value::<de::IgnoredAny>()?;
            }
        }
        let has = |key| values.contains_key(key);
        let form = (
            has("tool_name"),
            has("tool_input"),
            has("tool_input_file"),
            has("file"),
        );
        match form {
            (true, true, false, false) => Ok(UseToolInput::Inline(InlineMcpInvocation {
                tool_name: serde_json::from_value(
                    values.remove("tool_name").unwrap_or(Value::Null),
                )
                .map_err(de::Error::custom)?,
                tool_input: values.remove("tool_input").unwrap_or(Value::Null),
            })),
            (true, false, true, false) => Ok(UseToolInput::ArgumentsFile {
                tool_name: serde_json::from_value(
                    values.remove("tool_name").unwrap_or(Value::Null),
                )
                .map_err(de::Error::custom)?,
                tool_input_file: parse_path(values.remove("tool_input_file"))?,
            }),
            (false, false, false, true) => Ok(UseToolInput::InvocationFile {
                file: parse_path(values.remove("file"))?,
            }),
            _ => Err(de::Error::custom(
                "use exactly one of {tool_name,tool_input}, {tool_name,tool_input_file}, or {file}",
            )),
        }
    }
}

fn parse_path<E: de::Error>(value: Option<Value>) -> Result<PathBuf, E> {
    match value {
        Some(Value::String(path)) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Err(E::custom("MCP source path must be a nonempty string")),
    }
}

impl schemars::JsonSchema for UseToolInput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UseToolInput".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        UseToolInput::input_schema(false)
    }
}

impl UseToolInput {
    pub(crate) fn input_schema(supports_file_input: bool) -> schemars::Schema {
        let tool_name =
            serde_json::json!({"type": "string", "description": "Discovered MCP target name"});
        let tool_input = serde_json::json!({"type": "object", "additionalProperties": true, "description": "Inline remote arguments; use the discovered input schema"});
        if !supports_file_input {
            return schemars::json_schema!({
                "type": "object",
                "properties": {"tool_name": tool_name, "tool_input": tool_input},
                "required": ["tool_name", "tool_input"]
            });
        }
        let tool_input_file = serde_json::json!({"type": "string", "minLength": 1, "description": "UTF-8 JSON file containing only the complete remote argument object"});
        let file = serde_json::json!({"type": "string", "minLength": 1, "description": "UTF-8 JSON file containing canonical tool_name and object tool_input"});
        // Root unions compile each branch without inheriting the root properties.
        schemars::json_schema!({
            "type": "object",
            "properties": {
                "tool_name": tool_name, "tool_input": tool_input,
                "tool_input_file": tool_input_file, "file": file
            },
            "oneOf": [
                {"type": "object", "properties": {"tool_name": tool_name, "tool_input": tool_input},
                 "required": ["tool_name", "tool_input"], "not": {"anyOf": [{"required": ["tool_input_file"]}, {"required": ["file"]}]}},
                {"type": "object", "properties": {"tool_name": tool_name, "tool_input_file": tool_input_file},
                 "required": ["tool_name", "tool_input_file"], "not": {"anyOf": [{"required": ["tool_input"]}, {"required": ["file"]}]}},
                {"type": "object", "properties": {"file": file},
                 "required": ["file"], "not": {"anyOf": [{"required": ["tool_name"]}, {"required": ["tool_input"]}, {"required": ["tool_input_file"]}]}}
            ]
        })
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
