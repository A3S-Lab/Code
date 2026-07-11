use super::*;

/// Result of a non-streaming agent execution.
#[pyclass(name = "AgentResult")]
#[derive(Clone)]
pub(super) struct PyAgentResult {
    #[pyo3(get)]
    text: String,
    #[pyo3(get)]
    tool_calls_count: usize,
    #[pyo3(get)]
    prompt_tokens: usize,
    #[pyo3(get)]
    completion_tokens: usize,
    #[pyo3(get)]
    total_tokens: usize,
    #[pyo3(get)]
    verification_status: String,
    #[pyo3(get)]
    pending_verification_count: usize,
    #[pyo3(get)]
    failed_verification_count: usize,
    #[pyo3(get)]
    verification_report_count: usize,
    #[pyo3(get)]
    verification_summary_json: String,
    #[pyo3(get)]
    verification_summary_text: String,
}

#[pymethods]
impl PyAgentResult {
    fn __repr__(&self) -> String {
        format!(
            "AgentResult(text={:?}, tool_calls={}, tokens={}, verification={})",
            if self.text.len() > 80 {
                format!("{}...", truncate_utf8(&self.text, 80))
            } else {
                self.text.clone()
            },
            self.tool_calls_count,
            self.total_tokens,
            self.verification_status,
        )
    }

    fn __str__(&self) -> &str {
        &self.text
    }
}

impl From<RustAgentResult> for PyAgentResult {
    fn from(r: RustAgentResult) -> Self {
        let verification_summary = r.verification_summary();
        let verification_summary_json = verification_summary.to_value().to_string();
        let verification_summary_text = rust_format_verification_summary(&verification_summary);
        Self {
            text: r.text,
            tool_calls_count: r.tool_calls_count,
            prompt_tokens: r.usage.prompt_tokens,
            completion_tokens: r.usage.completion_tokens,
            total_tokens: r.usage.total_tokens,
            verification_status: verification_status_label(verification_summary.status),
            pending_verification_count: verification_summary.pending_required_check_count,
            failed_verification_count: verification_summary.failed_check_count,
            verification_report_count: verification_summary.report_count,
            verification_summary_json,
            verification_summary_text,
        }
    }
}

fn verification_status_label(status: RustVerificationStatus) -> String {
    match status {
        RustVerificationStatus::Passed => "passed",
        RustVerificationStatus::Failed => "failed",
        RustVerificationStatus::NeedsReview => "needs_review",
        RustVerificationStatus::Skipped => "skipped",
    }
    .to_string()
}

#[pyfunction]
pub(super) fn format_verification_summary(
    py: Python<'_>,
    summary: &Bound<'_, PyAny>,
) -> PyResult<String> {
    let summary_json = if let Ok(summary_json) = summary.extract::<String>() {
        summary_json
    } else {
        let json_mod = py.import("json")?;
        json_mod.call_method1("dumps", (summary,))?.extract()?
    };
    let summary: RustVerificationSummary = serde_json::from_str(&summary_json)
        .map_err(|e| PyValueError::new_err(format!("Invalid verification summary: {e}")))?;
    Ok(rust_format_verification_summary(&summary))
}
