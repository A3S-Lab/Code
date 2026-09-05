use super::ToolResultLossModeV1;
use crate::text::truncate_utf8;
use anyhow::Result;
use serde::de::{Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::VecDeque;

pub const TOOL_RESULT_TRANSFORM_SCHEMA_V1: &str = "a3s.code.tool-result-transform-policy.v1";
pub const TOOL_RESULT_TRANSFORM_ALGORITHM_V1: &str = "a3s.code.tool-result-transform.v1";
pub const TOOL_RESULT_TRANSFORM_BINDING_SCHEMA_V1: &str =
    "a3s.code.tool-result-transform-binding.v1";
pub const TOOL_RESULT_TRANSFORM_BINDING_METADATA_KEY: &str = "a3s_tool_result_transform_binding";
pub const TOOL_RESULT_TRANSFORM_POLICY_DIGEST_DOMAIN_V1: &str =
    "a3s.code.tool-result-transform-policy-digest.v1";
const MARKER_RESERVE_BYTES: usize = 512;
const MAX_STRUCTURED_SAMPLE_ITEMS: usize = 1024;

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
            self.structured_sample_items <= MAX_STRUCTURED_SAMPLE_ITEMS,
            "Tool result structured_sample_items must not exceed {MAX_STRUCTURED_SAMPLE_ITEMS}"
        );
        Ok(())
    }

    /// Return the stable, domain-separated identity of this exact policy.
    pub fn policy_digest(&self) -> Result<String> {
        self.validate()?;
        canonical_digest(TOOL_RESULT_TRANSFORM_POLICY_DIGEST_DOMAIN_V1, self)
    }
}

/// Bounded evidence that binds one Tool result to its exact deterministic
/// transform algorithm and policy without copying Cloud or provider identity
/// into Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResultTransformBindingV1 {
    pub schema: String,
    pub transform_algorithm: String,
    pub policy_digest: String,
    pub binding_digest: String,
}

impl ToolResultTransformBindingV1 {
    pub fn from_policy(policy: &ToolResultTransformPolicyV1) -> Result<Self> {
        let mut binding = Self {
            schema: TOOL_RESULT_TRANSFORM_BINDING_SCHEMA_V1.to_string(),
            transform_algorithm: TOOL_RESULT_TRANSFORM_ALGORITHM_V1.to_string(),
            policy_digest: policy.policy_digest()?,
            binding_digest: String::new(),
        };
        binding.binding_digest = binding.expected_digest()?;
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema == TOOL_RESULT_TRANSFORM_BINDING_SCHEMA_V1,
            "unsupported Tool result transform binding schema {:?}",
            self.schema
        );
        anyhow::ensure!(
            self.transform_algorithm == TOOL_RESULT_TRANSFORM_ALGORITHM_V1,
            "unsupported Tool result transform algorithm {:?}",
            self.transform_algorithm
        );
        anyhow::ensure!(
            valid_sha256(&self.policy_digest),
            "Tool result transform policy_digest must be canonical lowercase SHA-256"
        );
        anyhow::ensure!(
            valid_sha256(&self.binding_digest),
            "Tool result transform binding_digest must be canonical lowercase SHA-256"
        );
        anyhow::ensure!(
            self.binding_digest == self.expected_digest()?,
            "Tool result transform binding_digest does not bind the exact algorithm and policy"
        );
        Ok(())
    }

    pub fn validate_for_policy(&self, policy: &ToolResultTransformPolicyV1) -> Result<()> {
        self.validate()?;
        anyhow::ensure!(
            self.policy_digest == policy.policy_digest()?,
            "Tool result transform binding does not match the exact policy"
        );
        Ok(())
    }

    fn expected_digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            schema: &'a str,
            transform_algorithm: &'a str,
            policy_digest: &'a str,
        }

        canonical_digest(
            TOOL_RESULT_TRANSFORM_BINDING_SCHEMA_V1,
            &DigestInput {
                schema: &self.schema,
                transform_algorithm: &self.transform_algorithm,
                policy_digest: &self.policy_digest,
            },
        )
    }
}

fn canonical_digest(domain: &str, value: &impl Serialize) -> Result<String> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        anyhow::anyhow!("could not encode Tool result transform identity: {error}")
    })?;
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

pub(crate) struct ToolResultTransform {
    pub content: String,
    pub loss_mode: ToolResultLossModeV1,
    pub retained_original_bytes: usize,
}

pub(crate) fn transform(output: &str, policy: &ToolResultTransformPolicyV1) -> ToolResultTransform {
    // Avoid copying an unmodified result before we know that a transform is
    // needed. Tool output is untrusted and can be substantially larger than
    // the model-facing budget.
    let mut content: Cow<'_, str> = Cow::Borrowed(output);
    let mut transformed = false;

    if policy.structured_sample_items > 0 && output.len() > policy.max_output_bytes {
        if let Some(sampled) = sample_structured(output, policy.structured_sample_items) {
            content = Cow::Owned(sampled);
            transformed = true;
        }
    }
    if policy.fold_repeated_lines {
        if let Some(folded) = fold_repeated_lines(content.as_ref(), policy.repeated_line_threshold)
        {
            content = Cow::Owned(folded);
            transformed = true;
        }
    }
    if content.len() <= policy.max_output_bytes {
        return ToolResultTransform {
            retained_original_bytes: if transformed { 0 } else { output.len() },
            content: content.into_owned(),
            loss_mode: if transformed {
                ToolResultLossModeV1::DeterministicTransform
            } else {
                ToolResultLossModeV1::None
            },
        };
    }

    let head = truncate_utf8(content.as_ref(), policy.head_bytes);
    let tail = utf8_tail(content.as_ref(), policy.tail_bytes);
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

fn fold_repeated_lines(value: &str, threshold: usize) -> Option<String> {
    let mut lines = value.split_inclusive('\n').peekable();
    let mut output = String::new();
    let mut folded_run = false;
    let mut cursor = 0;

    while let Some(line) = lines.next() {
        let line_start = cursor;
        cursor += line.len();
        let mut count = 1;
        while let Some(next) = lines.peek().copied() {
            if next != line {
                break;
            }
            let Some(next) = lines.next() else {
                break;
            };
            cursor += next.len();
            count += 1;
        }
        if count >= threshold {
            if !folded_run {
                // Delay allocation until a fold is actually found. The
                // original prefix is copied exactly once at that point.
                output.push_str(&value[..line_start]);
            }
            output.push_str(line);
            output.push_str(&format!(
                "[a3s repeated-line fold: {} additional exact copies omitted]\n",
                count - 1
            ));
            folded_run = true;
        } else if folded_run {
            output.push_str(line);
        }
    }

    if folded_run && output.len() < value.len() {
        Some(output)
    } else {
        None
    }
}

fn sample_structured(value: &str, max_items: usize) -> Option<String> {
    // Consume one array element at a time. Keeping only the head sample and a
    // bounded tail avoids materializing an attacker-controlled `Vec<Value>`.
    let mut deserializer = serde_json::Deserializer::from_str(value);
    let sample = deserializer
        .deserialize_any(JsonArraySampler::new(max_items))
        .ok()??;
    deserializer.end().ok()?;
    let (original_items, sampled) = sample;
    serde_json::to_string(&serde_json::json!({
        "$a3s_sample": {
            "schema": TOOL_RESULT_TRANSFORM_ALGORITHM_V1,
            "kind": "json_array",
            "original_items": original_items,
            "retained_items": sampled.len(),
            "omitted_items": original_items - sampled.len(),
        },
        "items": sampled,
    }))
    .ok()
}

struct JsonArraySampler {
    max_items: usize,
}

impl JsonArraySampler {
    fn new(max_items: usize) -> Self {
        Self { max_items }
    }
}

impl<'de> Visitor<'de> for JsonArraySampler {
    type Value = Option<(usize, Vec<Value>)>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        // `validate` normally enforces this bound before a policy reaches the
        // transform. Clamp again at the parser boundary so an invalid or
        // host-constructed policy cannot request an enormous preallocation.
        let max_items = self.max_items.min(MAX_STRUCTURED_SAMPLE_ITEMS);
        let head_count = max_items.div_ceil(2);
        let tail_count = max_items / 2;
        let mut head = Vec::with_capacity(head_count);
        let mut tail = VecDeque::with_capacity(tail_count);
        let mut original_items = 0;

        while let Some(item) = sequence.next_element::<Value>()? {
            original_items += 1;
            if head.len() < head_count {
                head.push(item);
            } else if tail_count > 0 {
                if tail.len() == tail_count {
                    tail.pop_front();
                }
                tail.push_back(item);
            }
        }

        if original_items <= max_items {
            return Ok(None);
        }
        head.extend(tail);
        Ok(Some((original_items, head)))
    }
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
    fn binding_is_stable_and_rejects_policy_or_evidence_drift() {
        let policy = ToolResultTransformPolicyV1::context_efficient();
        let binding = ToolResultTransformBindingV1::from_policy(&policy).unwrap();

        assert_eq!(
            binding,
            ToolResultTransformBindingV1::from_policy(&policy).unwrap()
        );
        assert_eq!(
            binding.policy_digest,
            "sha256:645f65e5d39e3f7aa77fade21ae2daa1e8ccbbc7a0775c94a7f2c38ec5f5b32d"
        );
        assert_eq!(
            binding.binding_digest,
            "sha256:906e9931692fa7860b7acb5fc0bb5c329f19aeb04976c913750893ad99cd5a27"
        );
        binding.validate_for_policy(&policy).unwrap();

        let mut drifted_policy = policy.clone();
        drifted_policy.structured_sample_items += 1;
        assert!(binding.validate_for_policy(&drifted_policy).is_err());

        let mut drifted_binding = binding;
        drifted_binding.policy_digest = format!("sha256:{}", "0".repeat(64));
        assert!(drifted_binding.validate().is_err());
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

    #[test]
    fn folding_preserves_lines_before_the_first_repeated_run() {
        let policy = ToolResultTransformPolicyV1::context_efficient();
        let repeated = "same".repeat(32);
        let output = format!("prefix\n{repeated}\n{repeated}\n{repeated}\nsuffix\n");
        let transformed = transform(&output, &policy);

        assert_eq!(
            transformed.loss_mode,
            ToolResultLossModeV1::DeterministicTransform
        );
        assert!(transformed
            .content
            .starts_with(&format!("prefix\n{repeated}\n")));
        assert!(transformed.content.contains("2 additional exact copies"));
        assert!(transformed.content.ends_with("suffix\n"));
    }

    #[test]
    fn structured_sampling_keeps_a_bounded_working_set_for_large_arrays() {
        let policy = ToolResultTransformPolicyV1::context_efficient();
        let items = (0..250_000).map(Value::from).collect::<Vec<_>>();
        let output = serde_json::to_string(&items).unwrap();
        let transformed = transform(&output, &policy);

        assert!(transformed.content.contains("\"original_items\":250000"));
        assert!(transformed.content.contains("\"retained_items\":32"));
        assert_eq!(
            transformed.loss_mode,
            ToolResultLossModeV1::DeterministicTransform
        );
    }
}
