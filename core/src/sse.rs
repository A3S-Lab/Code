//! Server-Sent Events parsing helpers.

/// Return the value of an SSE `data:` field.
///
/// The SSE field grammar allows `data:<value>` and `data: <value>`. When a
/// single space follows the colon, it is ignored by the parser.
pub(crate) fn data_field_value(line: &str) -> Option<&str> {
    let value = line.strip_prefix("data:")?;
    Some(value.strip_prefix(' ').unwrap_or(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_field_value_accepts_optional_space() {
        assert_eq!(
            data_field_value("data: {\"ok\":true}"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            data_field_value("data:{\"ok\":true}"),
            Some("{\"ok\":true}")
        );
        assert_eq!(data_field_value("data: [DONE]"), Some("[DONE]"));
        assert_eq!(data_field_value("data:[DONE]"), Some("[DONE]"));
    }

    #[test]
    fn data_field_value_ignores_non_data_fields() {
        assert_eq!(data_field_value("event: message"), None);
        assert_eq!(data_field_value("id: 1"), None);
    }
}
