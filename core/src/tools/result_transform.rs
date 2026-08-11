use super::ToolResultLossModeV1;
use crate::text::truncate_utf8;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TOOL_RESULT_TRANSFORM_SCHEMA_V1: &str = "a3s.code.tool-result-transform-policy.v1";
pub const TOOL_RESULT_TRANSFORM_ALGORITHM_V1: &str = "a3s.code.tool-result-transform.v1";
const MARKER_RESERVE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResultTransformPolicyV1 {
    pub schema: String,
    pub max_output_bytes: usize,
    pub head_bytes: usize,
    pub tail_bytes: usize,
    pub fold_repeated_lines: bool,
    pub repeated_line_threshold: usize,
    pub structured_sample_items: usize,
}

impl Default for ToolResultTransformPolicyV1 {
    fn default() -> Self {
        Self::conservative()
    }
}

impl ToolResultTransformPolicyV1 {
    pub fn conservative() -> Self {
        Self {
            schema: TOOL_RESULT_TRANSFORM_SCHEMA_V1.to_string(),
            max_output_bytes: super::MAX_OUTPUT_SIZE,
            head_bytes: super::MAX_OUTPUT_SIZE,
            tail_bytes: 0,
            fold_repeated_lines: false,
            repeated_line_threshold: 3,
            structured_sample_items: 0,
        }
    }

    pub fn context_efficient() -> Self {
        Self {
            schema: TOOL_RESULT_TRANSFORM_SCHEMA_V1.to_string(),
            max_output_bytes: super::MAX_OUTPUT_SIZE,
            head_bytes: 64 * 1024,
            tail_bytes: 32 * 1024,
            fold_repeated_lines: true,
            repeated_line_threshold: 3,
            structured_sample_items: 32,
        }
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema == TOOL_RESULT_TRANSFORM_SCHEMA_V1,
            "unsupported Tool result transform policy schema {:?}",
            self.schema
        );
        anyhow::ensure!(
            (1024..=super::MAX_OUTPUT_SIZE).contains(&self.max_output_bytes),
            "Tool result max_output_bytes must be between 1024 and {}",
            super::MAX_OUTPUT_SIZE
        );
        anyhow::ensure!(
            self.head_bytes > 0,
            "Tool result head_bytes must be positive"
        );
        let retained = self.head_bytes.saturating_add(self.tail_bytes);
        let valid_compatibility_profile = self.tail_bytes == 0 && retained == self.max_output_bytes;
        anyhow::ensure!(
            valid_compatibility_profile
                || retained.saturating_add(MARKER_RESERVE_BYTES) <= self.max_output_bytes,
            "Tool result head_bytes + tail_bytes must reserve {MARKER_RESERVE_BYTES} bytes for transformation evidence"
        );
        anyhow::ensure!(
            (2..=10_000).contains(&self.repeated_line_threshold),
            "Tool result repeated_line_threshold must be between 2 and 10000"
        );
        anyhow::ensure!(
            self.structured_sample_items <= 1024,
            "Tool result structured_sample_items must not exceed 1024"
        );
        Ok(())
    }
}

pub(crate) struct ToolResultTransform {
    pub content: String,
    pub loss_mode: ToolResultLossModeV1,
    pub retained_original_bytes: usize,
}

pub(crate) fn transform(output: &str, policy: &ToolResultTransformPolicyV1) -> ToolResultTransform {
    let mut content = output.to_string();
    let mut transformed = false;

    if policy.structured_sample_items > 0 && output.len() > policy.max_output_bytes {
        if let Some(sampled) = sample_structured(output, policy.structured_sample_items) {
            content = sampled;
            transformed = true;
        }
    }
    if policy.fold_repeated_lines {
        let folded = fold_repeated_lines(&content, policy.repeated_line_threshold);
        transformed |= folded != content;
        content = folded;
    }
    if content.len() <= policy.max_output_bytes {
        return ToolResultTransform {
            retained_original_bytes: if transformed { 0 } else { output.len() },
            content,
            loss_mode: if transformed {
                ToolResultLossModeV1::DeterministicTransform
            } else {
                ToolResultLossModeV1::None
            },
        };
    }

    let head = truncate_utf8(&content, policy.head_bytes);
    let tail = utf8_tail(&content, policy.tail_bytes);
    let omitted = content.len().saturating_sub(head.len() + tail.len());
    let marker = if policy.tail_bytes == 0 && !transformed {
        format!(
            "\n\n[tool output truncated: showing the first {} of {} bytes. Full output is retained as an immutable artifact.]",
            head.len(),
            content.len()
        )
    } else {
        format!(
            "\n\n[tool output bounded by {}: omitted {} bytes between retained head/tail regions]\n\n",
            TOOL_RESULT_TRANSFORM_ALGORITHM_V1, omitted
        )
    };
    let projected = if tail.is_empty() {
        format!("{head}{marker}")
    } else {
        format!("{head}{marker}{tail}")
    };
    ToolResultTransform {
        content: projected,
        loss_mode: if transformed {
            ToolResultLossModeV1::Composite
        } else if policy.tail_bytes == 0 {
            ToolResultLossModeV1::BoundedPreview
        } else {
            ToolResultLossModeV1::HeadTail
        },
        retained_original_bytes: if transformed {
            0
        } else {
            head.len() + tail.len()
        },
    }
}

fn utf8_tail(value: &str, max_bytes: usize) -> &str {
    if max_bytes == 0 || value.is_empty() {
        return "";
    }
    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn fold_repeated_lines(value: &str, threshold: usize) -> String {
    let lines = value.split_inclusive('\n').collect::<Vec<_>>();
    if lines.len() < threshold {
        return value.to_string();
    }
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < lines.len() {
        let mut end = index + 1;
        while end < lines.len() && lines[end] == lines[index] {
            end += 1;
        }
        let count = end - index;
        if count >= threshold {
            output.push_str(lines[index]);
            output.push_str(&format!(
                "[a3s repeated-line fold: {} additional exact copies omitted]\n",
                count - 1
            ));
        } else {
            for line in &lines[index..end] {
                output.push_str(line);
            }
        }
        index = end;
    }
    if output.len() < value.len() {
        output
    } else {
        value.to_string()
    }
}

fn sample_structured(value: &str, max_items: usize) -> Option<String> {
    let Value::Array(items) = serde_json::from_str::<Value>(value).ok()? else {
        return None;
    };
    if items.len() <= max_items {
        return None;
    }
    let head_count = max_items.div_ceil(2);
    let tail_count = max_items / 2;
    let mut sampled = items[..head_count].to_vec();
    sampled.extend_from_slice(&items[items.len() - tail_count..]);
    serde_json::to_string(&serde_json::json!({
        "$a3s_sample": {
            "schema": TOOL_RESULT_TRANSFORM_ALGORITHM_V1,
            "kind": "json_array",
            "original_items": items.len(),
            "retained_items": sampled.len(),
            "omitted_items": items.len() - sampled.len(),
        },
        "items": sampled,
    }))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_closed_and_valid() {
        ToolResultTransformPolicyV1::conservative()
            .validate()
            .unwrap();
        ToolResultTransformPolicyV1::context_efficient()
            .validate()
            .unwrap();
        let mut invalid = ToolResultTransformPolicyV1::context_efficient();
        invalid.schema = "future".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn context_profile_retains_utf8_head_and_tail() {
        let mut policy = ToolResultTransformPolicyV1::context_efficient();
        policy.max_output_bytes = 1024;
        policy.head_bytes = 256;
        policy.tail_bytes = 256;
        policy.structured_sample_items = 0;
        let output = format!("BEGIN-{}-END", "界".repeat(600));
        let transformed = transform(&output, &policy);
        assert_eq!(transformed.loss_mode, ToolResultLossModeV1::HeadTail);
        assert!(transformed.content.starts_with("BEGIN-"));
        assert!(transformed.content.ends_with("-END"));
        assert!(std::str::from_utf8(transformed.content.as_bytes()).is_ok());
    }

    #[test]
    fn folds_exact_runs_and_samples_large_json_arrays() {
        let policy = ToolResultTransformPolicyV1::context_efficient();
        let repeated = format!("{}\n", "same".repeat(32));
        let folded = transform(
            &format!("{repeated}{repeated}{repeated}{repeated}next\n"),
            &policy,
        );
        assert_eq!(
            folded.loss_mode,
            ToolResultLossModeV1::DeterministicTransform
        );
        assert!(folded.content.contains("3 additional exact copies"));

        let items = (0..20_000).map(Value::from).collect::<Vec<_>>();
        let sampled = transform(&serde_json::to_string(&items).unwrap(), &policy);
        assert!(matches!(
            sampled.loss_mode,
            ToolResultLossModeV1::DeterministicTransform | ToolResultLossModeV1::Composite
        ));
        assert!(sampled.content.contains("\"original_items\":20000"));
    }
}
