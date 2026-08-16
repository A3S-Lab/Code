use super::digest::{measure, require_digest, require_optional_digest};
use super::{HarnessEvidenceError, ModelInputSnapshotV1};
use crate::llm::TokenUsage;
use serde::{Deserialize, Serialize};

pub const MODEL_USAGE_SNAPSHOT_V1_SCHEMA: &str = "a3s.code.model-usage-snapshot.v1";
const MODEL_USAGE_SNAPSHOT_DOMAIN: &str = "a3s.code.model-usage-snapshot.v1";

#[derive(Clone, Debug)]
pub(crate) struct ModelUsageBinding {
    call_sequence: u64,
    input_snapshot_digest: String,
    estimated_prompt_tokens: usize,
    tool_results: ToolResultContextUsageV1,
}

impl ModelUsageBinding {
    pub(crate) fn from_input(
        input: &ModelInputSnapshotV1,
        tool_results: ToolResultContextUsageV1,
    ) -> Self {
        Self {
            call_sequence: input.call_sequence,
            input_snapshot_digest: input.snapshot_digest.clone(),
            estimated_prompt_tokens: input.estimated_prompt_tokens,
            tool_results,
        }
    }

    pub(crate) fn call_sequence(&self) -> u64 {
        self.call_sequence
    }
}

/// Bounded measurements of Tool-result content visible to one model call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolResultContextUsageV1 {
    pub total_count: usize,
    pub unique_count: usize,
    pub repeated_count: usize,
    pub content_bytes: u64,
    pub repeated_content_bytes: u64,
    pub estimated_tokens: usize,
    pub repeated_estimated_tokens: usize,
    pub contents_digest: Option<String>,
    pub repeated_contents_digest: Option<String>,
}

impl ToolResultContextUsageV1 {
    pub fn validate(&self) -> Result<(), HarnessEvidenceError> {
        if self.unique_count > self.total_count
            || self.repeated_count != self.total_count.saturating_sub(self.unique_count)
            || (self.total_count > 0 && self.unique_count == 0)
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "unique and repeated Tool-result counts partition Tool results",
            ));
        }
        let no_tool_results = self.total_count == 0;
        if no_tool_results
            != (self.unique_count == 0
                && self.content_bytes == 0
                && self.estimated_tokens == 0
                && self.contents_digest.is_none())
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "Tool-result count, content usage, and digest agree",
            ));
        }
        let no_repeated_tool_results = self.repeated_count == 0;
        if no_repeated_tool_results
            != (self.repeated_content_bytes == 0
                && self.repeated_estimated_tokens == 0
                && self.repeated_contents_digest.is_none())
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "repeated Tool-result count, content usage, and digest agree",
            ));
        }
        if self.repeated_content_bytes > self.content_bytes
            || self.repeated_estimated_tokens > self.estimated_tokens
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "repeated Tool-result usage is bounded by total Tool-result usage",
            ));
        }
        require_optional_digest("contents_digest", self.contents_digest.as_deref())?;
        require_optional_digest(
            "repeated_contents_digest",
            self.repeated_contents_digest.as_deref(),
        )?;
        Ok(())
    }
}

/// Immutable per-call correlation between Code's prompt estimate and the
/// normalized usage report returned by an [`crate::llm::LlmClient`].
///
/// These values are context diagnostics, not a billing ledger. A client that
/// cannot observe provider usage may return zeroes, which are preserved rather
/// than replaced with Code's estimate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelUsageSnapshotV1 {
    pub schema: String,
    pub call_sequence: u64,
    pub input_snapshot_digest: String,
    pub estimated_prompt_tokens: usize,
    pub reported_prompt_tokens: usize,
    pub reported_completion_tokens: usize,
    pub reported_total_tokens: usize,
    pub reported_cache_read_tokens: Option<usize>,
    pub reported_cache_write_tokens: Option<usize>,
    pub tool_results: ToolResultContextUsageV1,
    pub snapshot_digest: String,
}

impl ModelUsageSnapshotV1 {
    pub fn from_input(
        input: &ModelInputSnapshotV1,
        tool_results: &ToolResultContextUsageV1,
        usage: &TokenUsage,
    ) -> Result<Self, HarnessEvidenceError> {
        input.validate()?;
        tool_results.validate()?;
        if tool_results.total_count != input.tool_result_count {
            return Err(HarnessEvidenceError::InvalidContents(
                "usage and input Tool-result counts agree",
            ));
        }
        Self::from_binding(
            &ModelUsageBinding::from_input(input, tool_results.clone()),
            usage,
        )
    }

    pub(crate) fn from_binding(
        binding: &ModelUsageBinding,
        usage: &TokenUsage,
    ) -> Result<Self, HarnessEvidenceError> {
        binding.tool_results.validate()?;
        let mut snapshot = Self {
            schema: MODEL_USAGE_SNAPSHOT_V1_SCHEMA.to_string(),
            call_sequence: binding.call_sequence,
            input_snapshot_digest: binding.input_snapshot_digest.clone(),
            estimated_prompt_tokens: binding.estimated_prompt_tokens,
            reported_prompt_tokens: usage.prompt_tokens,
            reported_completion_tokens: usage.completion_tokens,
            reported_total_tokens: usage.total_tokens,
            reported_cache_read_tokens: usage.cache_read_tokens,
            reported_cache_write_tokens: usage.cache_write_tokens,
            tool_results: binding.tool_results.clone(),
            snapshot_digest: String::new(),
        };
        snapshot.snapshot_digest = snapshot.expected_digest()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), HarnessEvidenceError> {
        if self.schema != MODEL_USAGE_SNAPSHOT_V1_SCHEMA {
            return Err(HarnessEvidenceError::UnsupportedSchema);
        }
        if self.call_sequence == 0 {
            return Err(HarnessEvidenceError::InvalidContents(
                "call_sequence is positive",
            ));
        }
        require_digest("input_snapshot_digest", &self.input_snapshot_digest)?;
        require_digest("snapshot_digest", &self.snapshot_digest)?;
        self.tool_results.validate()?;
        if self.snapshot_digest != self.expected_digest()? {
            return Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"));
        }
        Ok(())
    }

    pub fn validate_against(
        &self,
        input: &ModelInputSnapshotV1,
    ) -> Result<(), HarnessEvidenceError> {
        self.validate()?;
        input.validate()?;
        if self.call_sequence != input.call_sequence {
            return Err(HarnessEvidenceError::InvalidContents(
                "usage and input call sequences agree",
            ));
        }
        if self.input_snapshot_digest != input.snapshot_digest {
            return Err(HarnessEvidenceError::DigestMismatch(
                "input_snapshot_digest",
            ));
        }
        if self.estimated_prompt_tokens != input.estimated_prompt_tokens {
            return Err(HarnessEvidenceError::InvalidContents(
                "usage and input prompt estimates agree",
            ));
        }
        if self.tool_results.total_count != input.tool_result_count {
            return Err(HarnessEvidenceError::InvalidContents(
                "usage and input Tool-result counts agree",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, HarnessEvidenceError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            call_sequence: u64,
            input_snapshot_digest: &'a str,
            estimated_prompt_tokens: usize,
            reported_prompt_tokens: usize,
            reported_completion_tokens: usize,
            reported_total_tokens: usize,
            reported_cache_read_tokens: Option<usize>,
            reported_cache_write_tokens: Option<usize>,
            tool_results: &'a ToolResultContextUsageV1,
        }

        Ok(measure(
            MODEL_USAGE_SNAPSHOT_DOMAIN,
            &Identity {
                schema: &self.schema,
                call_sequence: self.call_sequence,
                input_snapshot_digest: &self.input_snapshot_digest,
                estimated_prompt_tokens: self.estimated_prompt_tokens,
                reported_prompt_tokens: self.reported_prompt_tokens,
                reported_completion_tokens: self.reported_completion_tokens,
                reported_total_tokens: self.reported_total_tokens,
                reported_cache_read_tokens: self.reported_cache_read_tokens,
                reported_cache_write_tokens: self.reported_cache_write_tokens,
                tool_results: &self.tool_results,
            },
        )?
        .digest)
    }
}
