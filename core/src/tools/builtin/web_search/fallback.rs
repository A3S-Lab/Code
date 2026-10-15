//! Provider-agnostic reporting for web search outcomes.

use super::{safe_search_result_url, sanitize_http_urls};
use crate::tools::types::ToolErrorKind;
use a3s_search::{EngineFailure, EngineOutcome, SearchResults};
use std::time::Duration;

pub(super) fn failure_reason(kind: &str) -> &'static str {
    match kind {
        "provider_quota" => "quota is exhausted",
        "provider_rate_limited" | "rate_limited" => "was rate limited",
        "provider_authentication" => "authentication failed",
        "provider_permission" | "permission_denied" => "denied access",
        "provider_unavailable" | "engine_suspended" | "circuit_open" => "is unavailable",
        "timeout" | "http_timeout" => "timed out",
        "provider_transport" | "network" | "http_connect" => "could not be reached",
        "provider_invalid_response" | "parse" | "http_decode" => "returned an invalid response",
        _ => "failed",
    }
}

pub(super) fn failure_summary(failures: &[EngineFailure]) -> String {
    let mut seen = std::collections::HashSet::new();
    failures
        .iter()
        .filter_map(|failure| {
            let engine = crate::text::truncate_utf8(&sanitize_http_urls(&failure.engine), 96)
                .trim()
                .to_string();
            let key = (engine.to_ascii_lowercase(), failure.kind.clone());
            (!engine.is_empty() && seen.insert(key))
                .then(|| format!("{engine} {}", failure_reason(&failure.kind)))
        })
        .take(4)
        .collect::<Vec<_>>()
        .join("; ")
}

pub(super) fn failure_metadata(failures: &[EngineFailure]) -> Vec<serde_json::Value> {
    failures
        .iter()
        .map(|failure| {
            serde_json::json!({
                "engine": crate::text::truncate_utf8(
                    &sanitize_http_urls(&failure.engine),
                    96,
                ),
                "provider": failure.provider.as_deref(),
                "kind": &failure.kind,
                "transient": failure.transient,
                "retry_after_seconds": failure.retry_after_seconds,
            })
        })
        .collect()
}

pub(super) fn outcome_metadata(outcomes: &[EngineOutcome]) -> Vec<serde_json::Value> {
    outcomes
        .iter()
        .map(|outcome| {
            serde_json::json!({
                "engine": crate::text::truncate_utf8(
                    &sanitize_http_urls(&outcome.engine),
                    96,
                ),
                "shortcut": crate::text::truncate_utf8(&outcome.shortcut, 32),
                "provider": outcome.provider.as_deref(),
                "kind": outcome.kind,
                "result_count": outcome.result_count,
                "duration_ms": outcome.duration_ms,
                "failure_kind": outcome.failure.as_ref().map(|failure| failure.kind.as_str()),
                "retry_after_seconds": outcome
                    .failure
                    .as_ref()
                    .and_then(|failure| failure.retry_after_seconds),
            })
        })
        .collect()
}

pub(super) fn tool_error_kind_for_failures(
    failures: &[EngineFailure],
    timeout: Duration,
) -> Option<ToolErrorKind> {
    if failures.is_empty() {
        return None;
    }

    if failures.iter().all(|failure| {
        matches!(
            failure.kind.as_str(),
            "provider_rate_limited" | "rate_limited"
        )
    }) {
        return Some(ToolErrorKind::RateLimited {
            retry_after_ms: None,
        });
    }

    failures
        .iter()
        .all(|failure| matches!(failure.kind.as_str(), "timeout" | "http_timeout"))
        .then(|| ToolErrorKind::Timeout {
            op: "web_search".to_string(),
            duration_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
}

pub(super) fn usable_result_count(results: &SearchResults) -> usize {
    results
        .items()
        .iter()
        .filter(|result| !safe_search_result_url(result).is_empty())
        .count()
}

pub(super) fn text_notice_note(notices: &[String]) -> String {
    if notices.is_empty() {
        return String::new();
    }
    let mut note = String::new();
    for notice in notices {
        note.push_str(&format!("\nNotice: {notice}\n"));
    }
    note
}
