//! Provider-agnostic fallback policy for web search.

use super::engines::{canonical_engine_shortcut, engine_tier, EngineTier};
use super::{safe_search_result_url, sanitize_http_urls};
use crate::tools::types::ToolErrorKind;
use a3s_search::{EngineFailure, EngineOutcome, SearchResults};
use std::time::Duration;

const DEFAULT_HTTP_TIER: [&str; 4] = ["ddg", "brave", "bing", "wiki"];
#[cfg(feature = "headless-search")]
const DEFAULT_HEADLESS_TIER: [&str; 4] = ["g", "baidu", "brave_browser", "bing_browser"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct EngineTierPlan {
    pub api: Vec<String>,
    pub http: Vec<String>,
    pub headless: Vec<String>,
}

#[cfg(feature = "headless-search")]
pub(super) fn automatic_tier_order() -> [EngineTier; 3] {
    [EngineTier::Headless, EngineTier::Http, EngineTier::Api]
}

#[cfg(not(feature = "headless-search"))]
pub(super) fn automatic_tier_order() -> [EngineTier; 2] {
    [EngineTier::Http, EngineTier::Api]
}

/// Resolve cascade tier order from SearchConfig, falling back to the product default.
pub(super) fn resolved_tier_order(config: Option<&crate::config::SearchConfig>) -> Vec<EngineTier> {
    let defaults = automatic_tier_order();
    let Some(order) = config.and_then(|config| config.cascade_order.as_ref()) else {
        return defaults.to_vec();
    };

    let mut tiers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for tier in order {
        let mapped = match tier {
            crate::config::SearchCascadeTier::Http => Some(EngineTier::Http),
            crate::config::SearchCascadeTier::Api => Some(EngineTier::Api),
            crate::config::SearchCascadeTier::Headless => {
                #[cfg(feature = "headless-search")]
                {
                    Some(EngineTier::Headless)
                }
                #[cfg(not(feature = "headless-search"))]
                {
                    None
                }
            }
        };
        let Some(mapped) = mapped else {
            continue;
        };
        if seen.insert(mapped) {
            tiers.push(mapped);
        }
    }

    if tiers.is_empty() {
        defaults.to_vec()
    } else {
        tiers
    }
}

#[cfg(test)]
mod cascade_order_tests {
    use super::{automatic_tier_order, resolved_tier_order, EngineTier};
    use crate::config::{SearchCascadeTier, SearchConfig};
    use std::collections::HashMap;

    #[cfg(feature = "headless-search")]
    #[test]
    fn resolved_tier_order_uses_config_and_falls_back_to_defaults() {
        assert_eq!(resolved_tier_order(None), automatic_tier_order().to_vec());

        let config = SearchConfig {
            timeout: 10,
            cascade_order: Some(vec![
                SearchCascadeTier::Api,
                SearchCascadeTier::Http,
                SearchCascadeTier::Api,
            ]),
            health: None,
            engines: HashMap::new(),
            headless: None,
        };
        assert_eq!(
            resolved_tier_order(Some(&config)),
            vec![EngineTier::Api, EngineTier::Http]
        );

        let empty = SearchConfig {
            cascade_order: Some(Vec::new()),
            ..config
        };
        assert_eq!(
            resolved_tier_order(Some(&empty)),
            automatic_tier_order().to_vec()
        );
    }
}

impl EngineTierPlan {
    pub fn is_empty(&self) -> bool {
        self.api.is_empty() && self.http.is_empty() && self.headless.is_empty()
    }
}

pub(super) fn configured_engine<'a>(
    config: &'a crate::config::SearchConfig,
    shortcut: &str,
) -> Option<&'a crate::config::SearchEngineConfig> {
    let normalized = shortcut.trim().to_ascii_lowercase();
    config.engines.get(shortcut).or_else(|| {
        let canonical = canonical_engine_shortcut(&normalized);
        config.engines.iter().find_map(|(name, engine)| {
            let normalized_name = name.trim().to_ascii_lowercase();
            canonical_engine_shortcut(&normalized_name)
                .eq_ignore_ascii_case(canonical)
                .then_some(engine)
        })
    })
}

pub(super) fn tiered_engine_plan(
    selected_engines: &[&str],
    config: Option<&crate::config::SearchConfig>,
    automatic_fallback: bool,
) -> EngineTierPlan {
    let mut plan = EngineTierPlan::default();
    for shortcut in selected_engines {
        add_planned_engine(&mut plan, shortcut, config);
    }

    if automatic_fallback && (!plan.api.is_empty() || !plan.http.is_empty()) {
        for shortcut in DEFAULT_HTTP_TIER {
            add_planned_engine(&mut plan, shortcut, config);
        }
        #[cfg(feature = "headless-search")]
        {
            for shortcut in DEFAULT_HEADLESS_TIER {
                add_planned_engine(&mut plan, shortcut, config);
            }
        }
    }

    plan
}

fn add_planned_engine(
    plan: &mut EngineTierPlan,
    shortcut: &str,
    config: Option<&crate::config::SearchConfig>,
) {
    let normalized = shortcut.trim().to_ascii_lowercase();
    let canonical = canonical_engine_shortcut(&normalized);
    if config
        .and_then(|config| configured_engine(config, canonical))
        .is_some_and(|engine| !engine.enabled)
    {
        return;
    }
    let target = match engine_tier(canonical) {
        Some(EngineTier::Api) => &mut plan.api,
        Some(EngineTier::Http) => &mut plan.http,
        #[cfg(feature = "headless-search")]
        Some(EngineTier::Headless) => &mut plan.headless,
        None => return,
    };
    if !target.iter().any(|current| current == canonical) {
        target.push(canonical.to_string());
    }
}

pub(super) fn tier_timeout(remaining: Duration, remaining_tiers: usize) -> Duration {
    if remaining_tiers == 0 || remaining.is_zero() {
        return remaining;
    }
    let divisor = u128::try_from(remaining_tiers)
        .unwrap_or(u128::MAX)
        .saturating_add(1);
    let milliseconds = (remaining.as_millis() / divisor).max(1);
    Duration::from_millis(u64::try_from(milliseconds).unwrap_or(u64::MAX)).min(remaining)
}

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
