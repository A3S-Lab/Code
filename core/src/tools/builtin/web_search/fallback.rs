//! Provider-agnostic fallback policy for web search.

use super::engines::canonical_engine_shortcut;
use super::{safe_search_result_url, sanitize_http_urls};
use crate::tools::types::ToolErrorKind;
use a3s_search::{EngineFailure, SearchResults};
use std::time::Duration;

const FALLBACK_ENGINE_ORDER: [&str; 4] = ["ddg", "brave", "bing", "wiki"];

pub(super) fn configured_engine<'a>(
    config: &'a crate::config::SearchConfig,
    shortcut: &str,
) -> Option<&'a crate::config::SearchEngineConfig> {
    config.engines.get(shortcut).or_else(|| {
        let canonical = canonical_engine_shortcut(shortcut);
        config.engines.iter().find_map(|(name, engine)| {
            canonical_engine_shortcut(name)
                .eq_ignore_ascii_case(canonical)
                .then_some(engine)
        })
    })
}

pub(super) fn fallback_engine_shortcuts(
    attempted_engines: &[String],
    config: Option<&crate::config::SearchConfig>,
) -> Vec<&'static str> {
    FALLBACK_ENGINE_ORDER
        .iter()
        .copied()
        .filter(|candidate| {
            let candidate = canonical_engine_shortcut(candidate);
            !attempted_engines.iter().any(|attempted| {
                canonical_engine_shortcut(attempted).eq_ignore_ascii_case(candidate)
            }) && config
                .and_then(|config| configured_engine(config, candidate))
                .is_none_or(|engine| engine.enabled)
        })
        .collect()
}

pub(super) fn fallback_engine_names(engines: &[&str]) -> String {
    let names = engines
        .iter()
        .map(|engine| match *engine {
            "ddg" => "DuckDuckGo",
            "brave" => "Brave",
            "bing" => "Bing",
            "wiki" => "Wikipedia",
            other => other,
        })
        .collect::<Vec<_>>();
    match names.as_slice() {
        [] => String::new(),
        [name] => (*name).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => names.join(", "),
    }
}

pub(super) fn primary_search_timeout(total: Duration, has_fallback: bool) -> Duration {
    if !has_fallback {
        return total;
    }
    let total_ms = u64::try_from(total.as_millis()).unwrap_or(u64::MAX);
    let reserve_ms = (total_ms / 3).clamp(500, 5_000).min(total_ms / 2);
    Duration::from_millis(total_ms.saturating_sub(reserve_ms).max(1))
}

pub(super) fn failure_reason(kind: &str) -> &'static str {
    match kind {
        "provider_quota" => "quota is exhausted",
        "provider_rate_limited" | "rate_limited" => "was rate limited",
        "provider_authentication" => "authentication failed",
        "provider_permission" | "permission_denied" => "denied access",
        "provider_unavailable" | "engine_suspended" => "is unavailable",
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

pub(super) fn usable_result_engines(results: &SearchResults) -> Vec<String> {
    let mut engines = results
        .items()
        .iter()
        .filter(|result| !safe_search_result_url(result).is_empty())
        .flat_map(|result| result.engines.iter().cloned())
        .collect::<Vec<_>>();
    engines.sort_unstable();
    engines.dedup();
    engines
}

pub(super) fn merge_search_results(target: &mut SearchResults, source: &SearchResults) {
    for result in source.items() {
        target.add_result(result.clone());
    }
    for suggestion in source.suggestions() {
        target.add_suggestion(suggestion.clone());
    }
    for answer in source.answers() {
        target.add_answer(answer.clone());
    }
    for image in source.images() {
        target.add_image(image.clone());
    }
    for report in source.reports() {
        target.add_report(report.clone());
    }
    if source.failures().is_empty() {
        for (engine, error) in source.errors() {
            target.add_error(engine.clone(), error.clone());
        }
    } else {
        for failure in source.failures() {
            target.add_failure(failure.clone());
        }
    }
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
