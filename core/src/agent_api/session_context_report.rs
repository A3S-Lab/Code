//! Session-level context and cost view.
//!
//! Provider usage comes from the session totals; tool-result weight comes from
//! the session fact log, so the view matches what the model actually received.

use crate::llm::TokenUsage;

const HEAVIEST_SHOWN: usize = 5;

/// One of the heaviest tool results the model received in this session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeavyToolResult {
    pub tool: String,
    pub tool_call_id: String,
    pub bytes: usize,
}

/// Tokens, cache share and tool-result weight for one session.
#[derive(Debug, Clone)]
pub struct SessionContextReport {
    pub usage: TokenUsage,
    pub tool_results: usize,
    pub tool_result_bytes: usize,
    /// Results whose tool, arguments and text already appeared in the session.
    pub repeated_results: usize,
    pub repeated_bytes: usize,
    /// Largest results first, at most five.
    pub heaviest: Vec<HeavyToolResult>,
}

impl SessionContextReport {
    pub(super) fn from_parts(
        usage: TokenUsage,
        weights: Vec<crate::fact_control::ToolResultWeight>,
    ) -> Self {
        let tool_result_bytes = weights.iter().map(|weight| weight.bytes).sum();
        let repeats = weights.iter().filter(|weight| weight.repeat);
        let repeated_results = repeats.clone().count();
        let repeated_bytes = repeats.map(|weight| weight.bytes).sum();
        let tool_results = weights.len();
        let mut heaviest = weights
            .into_iter()
            .map(|weight| HeavyToolResult {
                tool: weight.tool,
                tool_call_id: weight.tool_call_id,
                bytes: weight.bytes,
            })
            .collect::<Vec<_>>();
        heaviest.sort_by_key(|heavy| std::cmp::Reverse(heavy.bytes));
        heaviest.truncate(HEAVIEST_SHOWN);
        Self {
            usage,
            tool_results,
            tool_result_bytes,
            repeated_results,
            repeated_bytes,
            heaviest,
        }
    }

    /// Share of prompt tokens the provider served from its cache, when reported.
    pub fn cache_read_share(&self) -> Option<f64> {
        let read = self.usage.cache_read_tokens?;
        (self.usage.prompt_tokens > 0).then(|| read as f64 / self.usage.prompt_tokens as f64)
    }

    pub fn render(&self) -> String {
        let usage = &self.usage;
        let cache = match self.cache_read_share() {
            Some(share) => format!(
                "{} read ({:.1}% of prompt), {} written",
                usage.cache_read_tokens.unwrap_or(0),
                share * 100.0,
                usage.cache_write_tokens.unwrap_or(0)
            ),
            None => "not reported by the provider".to_string(),
        };
        let mut text = format!(
            "Tokens:  {} prompt, {} completion, {} total\n\
             Cache:   {cache}\n\
             Results: {} tool results, {} bytes; {} repeated ({} bytes)",
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens,
            self.tool_results,
            self.tool_result_bytes,
            self.repeated_results,
            self.repeated_bytes,
        );
        for heavy in &self.heaviest {
            text.push_str(&format!(
                "\n  {:>9} B  {} ({})",
                heavy.bytes, heavy.tool, heavy.tool_call_id
            ));
        }
        text
    }
}
