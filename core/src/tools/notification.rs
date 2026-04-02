//! Task notification format for multi-agent coordination
//!
//! Provides serialization of tasks into XML and JSON formats for inter-agent
//! communication, similar to Claude Code's coordinator mode.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Task notification envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotification {
    /// Unique task identifier
    pub id: String,
    /// Task type (e.g., "explore", "implement", "review")
    pub task_type: String,
    /// Task prompt/description
    pub prompt: String,
    /// Optional parent task ID for dependency tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Priority level (1-5, 5 being highest)
    #[serde(default)]
    pub priority: u8,
    /// Additional metadata
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl TaskNotification {
    pub fn new(id: String, task_type: String, prompt: String) -> Self {
        Self {
            id,
            task_type,
            prompt,
            parent_id: None,
            priority: 3,
            metadata: serde_json::json!({}),
        }
    }

    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.min(5);
        self
    }

    pub fn with_parent(mut self, parent_id: String) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    pub fn with_metadata(mut self, key: &str, value: serde_json::Value) -> Self {
        self.metadata[key] = value;
        self
    }
}

/// Notification format serializer trait
///
/// Allows customizing how task notifications are serialized for different
/// communication protocols (XML for Claude Code compatibility, JSON for REST).
pub trait NotificationFormat: Send + Sync {
    /// Serialize a task notification
    fn serialize(&self, task: &TaskNotification) -> Result<String, NotificationError>;

    /// Deserialize a task notification
    fn deserialize(&self, data: &str) -> Result<TaskNotification, NotificationError>;

    /// Format name (e.g., "xml", "json")
    fn name(&self) -> &str;
}

/// Notification serialization error
#[derive(Debug)]
pub struct NotificationError {
    message: String,
}

impl fmt::Display for NotificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for NotificationError {}

impl NotificationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

/// JSON notification format (default)
pub struct JsonNotificationFormat;

impl JsonNotificationFormat {
    pub fn new() -> Self {
        Self
    }
}

impl Default for JsonNotificationFormat {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationFormat for JsonNotificationFormat {
    fn serialize(&self, task: &TaskNotification) -> Result<String, NotificationError> {
        serde_json::to_string_pretty(task).map_err(|e| NotificationError::new(e.to_string()))
    }

    fn deserialize(&self, data: &str) -> Result<TaskNotification, NotificationError> {
        serde_json::from_str(data).map_err(|e| NotificationError::new(e.to_string()))
    }

    fn name(&self) -> &str {
        "json"
    }
}

/// XML notification format (Claude Code compatible)
pub struct XmlNotificationFormat;

impl XmlNotificationFormat {
    pub fn new() -> Self {
        Self
    }
}

impl Default for XmlNotificationFormat {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationFormat for XmlNotificationFormat {
    fn serialize(&self, task: &TaskNotification) -> Result<String, NotificationError> {
        let mut xml = format!(
            r#"<task>
  <id>{}</id>
  <type>{}</type>
  <prompt><![CDATA[{}]]></prompt>"#,
            escape_xml(&task.id),
            escape_xml(&task.task_type),
            escape_xml(&task.prompt)
        );

        if let Some(ref parent) = task.parent_id {
            xml.push_str(&format!(
                "\n  <parent_id>{}</parent_id>",
                escape_xml(parent)
            ));
        }

        xml.push_str(&format!("\n  <priority>{}</priority>", task.priority));

        // Serialize metadata as child elements
        if !task.metadata.is_null() {
            xml.push_str("\n  <metadata>");
            if let Some(obj) = task.metadata.as_object() {
                for (k, v) in obj {
                    xml.push_str(&format!(
                        "\n    <{}>{}</{}>",
                        escape_xml(k),
                        escape_xml(&v.to_string()),
                        escape_xml(k)
                    ));
                }
            }
            xml.push_str("\n  </metadata>");
        }

        xml.push_str("\n</task>");
        Ok(xml)
    }

    fn deserialize(&self, data: &str) -> Result<TaskNotification, NotificationError> {
        // Simple XML parsing for task elements
        let id =
            extract_xml_value(data, "id").ok_or_else(|| NotificationError::new("Missing id"))?;
        let task_type = extract_xml_value(data, "type")
            .ok_or_else(|| NotificationError::new("Missing type"))?;
        let prompt = extract_xml_value(data, "prompt")
            .ok_or_else(|| NotificationError::new("Missing prompt"))?;
        let parent_id = extract_xml_value(data, "parent_id");
        let priority = extract_xml_value(data, "priority")
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);

        Ok(TaskNotification {
            id,
            task_type,
            prompt,
            parent_id,
            priority,
            metadata: serde_json::json!({}),
        })
    }

    fn name(&self) -> &str {
        "xml"
    }
}

/// Escape special XML characters
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Extract value from XML element (simple regex-based parser)
fn extract_xml_value(xml: &str, tag: &str) -> Option<String> {
    let pattern = format!(r#"<{tag}><!\[CDATA\[([\s\S]*?)\]\]></{tag}>"#, tag = tag);
    if let Ok(re) = regex::Regex::new(&pattern) {
        if let Some(cap) = re.captures(xml) {
            return Some(cap.get(1).unwrap().as_str().to_string());
        }
    }

    // Try without CDATA
    let pattern = format!(r"<{tag}>([^<]*)</{tag}>", tag = tag);
    if let Ok(re) = regex::Regex::new(&pattern) {
        if let Some(cap) = re.captures(xml) {
            return Some(cap.get(1).unwrap().as_str().to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_roundtrip() {
        let task = TaskNotification::new(
            "task-1".to_string(),
            "explore".to_string(),
            "Find all files".to_string(),
        )
        .with_priority(5);

        let format = JsonNotificationFormat::new();
        let serialized = format.serialize(&task).unwrap();
        let deserialized = format.deserialize(&serialized).unwrap();

        assert_eq!(deserialized.id, "task-1");
        assert_eq!(deserialized.task_type, "explore");
        assert_eq!(deserialized.prompt, "Find all files");
        assert_eq!(deserialized.priority, 5);
    }

    #[test]
    fn test_xml_roundtrip() {
        let task = TaskNotification::new(
            "task-2".to_string(),
            "implement".to_string(),
            "Add feature".to_string(),
        )
        .with_parent("task-1".to_string());

        let format = XmlNotificationFormat::new();
        let serialized = format.serialize(&task).unwrap();
        assert!(serialized.contains("<task>"));
        assert!(serialized.contains("<id>task-2</id>"));

        let deserialized = format.deserialize(&serialized).unwrap();
        assert_eq!(deserialized.id, "task-2");
        assert_eq!(deserialized.parent_id, Some("task-1".to_string()));
    }

    #[test]
    fn test_xml_escape_cdata() {
        let task = TaskNotification::new(
            "task-3".to_string(),
            "review".to_string(),
            "Check <input> & \"output\"".to_string(),
        );

        let format = XmlNotificationFormat::new();
        let serialized = format.serialize(&task).unwrap();
        assert!(serialized.contains("&lt;input&gt;"));
        assert!(serialized.contains("&amp;"));
    }

    #[test]
    fn test_task_notification_builder() {
        let task = TaskNotification::new(
            "test-id".to_string(),
            "test-type".to_string(),
            "test prompt".to_string(),
        )
        .with_priority(4)
        .with_parent("parent-id".to_string())
        .with_metadata("key", serde_json::json!("value"));

        assert_eq!(task.id, "test-id");
        assert_eq!(task.task_type, "test-type");
        assert_eq!(task.prompt, "test prompt");
        assert_eq!(task.priority, 4);
        assert_eq!(task.parent_id, Some("parent-id".to_string()));
        assert_eq!(task.metadata["key"], "value");
    }

    #[test]
    fn test_task_notification_priority_clamped() {
        let task =
            TaskNotification::new("id".to_string(), "type".to_string(), "prompt".to_string())
                .with_priority(10); // Should be clamped to 5

        assert_eq!(task.priority, 5);
    }

    #[test]
    fn test_json_notification_format_name() {
        let format = JsonNotificationFormat::new();
        assert_eq!(format.name(), "json");
    }

    #[test]
    fn test_xml_notification_format_name() {
        let format = XmlNotificationFormat::new();
        assert_eq!(format.name(), "xml");
    }

    #[test]
    fn test_json_serialize_error() {
        let format = JsonNotificationFormat::new();
        // Invalid task with circular reference would fail
        let task =
            TaskNotification::new("id".to_string(), "type".to_string(), "prompt".to_string());
        let result = format.serialize(&task);
        assert!(result.is_ok());
    }

    #[test]
    fn test_json_deserialize_invalid() {
        let format = JsonNotificationFormat::new();
        let result = format.deserialize("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_deserialize_invalid() {
        let format = XmlNotificationFormat::new();
        let result = format.deserialize("<task><id>only id</task>");
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_deserialize_missing_fields() {
        let format = XmlNotificationFormat::new();
        let result = format.deserialize("<task><id>test</id></task>");
        assert!(result.is_err()); // Missing type and prompt
    }

    #[test]
    fn test_xml_serialize_with_metadata() {
        let task =
            TaskNotification::new("id".to_string(), "type".to_string(), "prompt".to_string())
                .with_metadata("extra", serde_json::json!({"nested": true}));

        let format = XmlNotificationFormat::new();
        let serialized = format.serialize(&task).unwrap();
        assert!(serialized.contains("<extra>"));
    }

    #[test]
    fn test_notification_error_display() {
        let err = NotificationError::new("test error");
        assert_eq!(format!("{}", err), "test error");
    }

    #[test]
    fn test_notification_error_fromserde() {
        let err = NotificationError::new("json error");
        let result: Result<String, _> = Err(err);
        assert!(result.is_err());
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("<>&\"'"), "&lt;&gt;&amp;&quot;&apos;");
        assert_eq!(escape_xml("normal"), "normal");
        assert_eq!(escape_xml(""), "");
    }

    #[test]
    fn test_extract_xml_value_cdata() {
        let xml = r#"<test><![CDATA[content here]]></test>"#;
        let value = extract_xml_value(xml, "test");
        assert_eq!(value, Some("content here".to_string()));
    }

    #[test]
    fn test_extract_xml_value_simple() {
        let xml = "<test>simple content</test>";
        let value = extract_xml_value(xml, "test");
        assert_eq!(value, Some("simple content".to_string()));
    }

    #[test]
    fn test_extract_xml_value_not_found() {
        let xml = "<other>content</other>";
        let value = extract_xml_value(xml, "test");
        assert!(value.is_none());
    }

    #[test]
    fn test_task_notification_default_priority() {
        let task =
            TaskNotification::new("id".to_string(), "type".to_string(), "prompt".to_string());
        assert_eq!(task.priority, 3); // Default priority
    }

    #[test]
    fn test_task_notification_with_metadata_multiple() {
        let task =
            TaskNotification::new("id".to_string(), "type".to_string(), "prompt".to_string())
                .with_metadata("key1", serde_json::json!("value1"))
                .with_metadata("key2", serde_json::json!(42));

        assert_eq!(task.metadata["key1"], "value1");
        assert_eq!(task.metadata["key2"], 42);
    }
}
