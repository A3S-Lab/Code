//! Bridge from Code's search configuration to A3S Search's product plan.

use crate::config::SearchCascadeTier;
use a3s_search::providers::{
    A3sConfig, AliyunConfig, AnySearchConfig, BochaConfig, BuiltinProvider, CredentialSource,
    DeepSeekConfig, DoubaoConfig, FirecrawlConfig, TavilyConfig, TencentConfig, TinyFishConfig,
};
use a3s_search::{
    EngineEntry, EngineFailure, EngineTier, HealthEntry, ProviderEntry, ProviderSettings,
    SearchError,
};

pub(super) fn provider_setup_failure(
    provider: BuiltinProvider,
    error: &SearchError,
) -> EngineFailure {
    EngineFailure::new(provider.id(), error.kind(), error.to_string())
        .with_provider(provider.id())
        .with_transient(error.is_transient())
}

/// Code's configuration as A3S Search configuration. A provider whose
/// configuration is invalid is left out and reported as a typed failure, so a
/// bad endpoint never silently falls back to the provider's default.
pub(crate) fn bridge_search_config(
    config: &crate::config::SearchConfig,
) -> (a3s_search::SearchConfig, Vec<EngineFailure>) {
    let mut bridged = a3s_search::SearchConfig::new();
    bridged.timeout = config.timeout;
    bridged.health = config.health.as_ref().map(|health| HealthEntry {
        max_failures: health.max_failures,
        suspend_seconds: health.suspend_seconds,
    });
    let mut failures = Vec::new();
    for (name, engine) in &config.engines {
        let shortcut = name.trim().to_ascii_lowercase();
        let Some(provider) = BuiltinProvider::from_id(&shortcut) else {
            bridged.engines.insert(
                shortcut,
                EngineEntry {
                    enabled: engine.enabled,
                    weight: engine.weight,
                    timeout: engine.timeout,
                },
            );
            continue;
        };
        match provider_settings(provider, engine) {
            Ok(settings) => {
                let mut entry = ProviderEntry::new(settings);
                entry.enabled = engine.enabled;
                entry.weight = engine.weight;
                entry.timeout = engine.timeout;
                bridged.providers.insert(provider.id().to_string(), entry);
            }
            Err(error) => failures.push(provider_setup_failure(provider, &error)),
        }
    }
    (bridged, failures)
}

/// Listed tiers run first; tiers the list omits keep the default order after them.
pub(super) fn tier_order(order: &[SearchCascadeTier]) -> Vec<EngineTier> {
    let mut tiers = Vec::new();
    let listed = order.iter().map(|tier| match tier {
        SearchCascadeTier::Api => EngineTier::Api,
        SearchCascadeTier::Http => EngineTier::HttpRss,
        SearchCascadeTier::Headless => EngineTier::Headless,
    });
    for tier in listed.chain(EngineTier::default_order()) {
        if !tiers.contains(&tier) {
            tiers.push(tier);
        }
    }
    tiers
}

fn with_api_key<T>(
    config: T,
    api_key: Option<&str>,
    apply: impl FnOnce(T, CredentialSource) -> T,
) -> T {
    match api_key {
        Some(api_key) => apply(config, CredentialSource::value(api_key)),
        None => config,
    }
}

fn parse_optional_endpoint(
    provider: &str,
    endpoint: Option<&str>,
) -> a3s_search::Result<Option<url::Url>> {
    let Some(endpoint) = endpoint.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    url::Url::parse(endpoint).map(Some).map_err(|_| {
        SearchError::Other(format!(
            "{provider} endpoint is not a valid URL: {endpoint}"
        ))
    })
}

fn provider_settings(
    provider: BuiltinProvider,
    engine: &crate::config::SearchEngineConfig,
) -> a3s_search::Result<ProviderSettings> {
    let api_key = engine
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let project = engine
        .project
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let endpoint = parse_optional_endpoint(provider.id(), engine.endpoint.as_deref())?;

    match provider {
        BuiltinProvider::AnySearch => {
            let mut provider_config = AnySearchConfig::new()?;
            if let Some(endpoint) = endpoint {
                provider_config = provider_config.with_endpoint(endpoint)?;
                // Fixture endpoints are anonymous unless an explicit key is set.
                if api_key.is_none() {
                    provider_config = provider_config.with_api_key(CredentialSource::none());
                }
            }
            if let Some(api_key) = api_key {
                provider_config = provider_config.with_api_key(CredentialSource::value(api_key));
            }
            Ok(ProviderSettings::AnySearch(provider_config))
        }
        BuiltinProvider::Tavily => {
            let mut provider_config = TavilyConfig::new()?;
            if let Some(endpoint) = endpoint {
                provider_config = provider_config.with_endpoint(endpoint)?;
                if api_key.is_none() {
                    provider_config = provider_config.with_api_key(CredentialSource::none());
                }
            }
            if let Some(api_key) = api_key {
                provider_config = provider_config.with_api_key(CredentialSource::value(api_key));
            }
            if let Some(project) = project {
                provider_config = provider_config.with_project(CredentialSource::value(project));
            }
            Ok(ProviderSettings::Tavily(provider_config))
        }
        BuiltinProvider::TinyFish => Ok(ProviderSettings::TinyFish(with_api_key(
            TinyFishConfig::new()?,
            api_key,
            TinyFishConfig::with_api_key,
        ))),
        BuiltinProvider::Bocha => Ok(ProviderSettings::Bocha(with_api_key(
            BochaConfig::new()?,
            api_key,
            BochaConfig::with_api_key,
        ))),
        BuiltinProvider::Aliyun => Ok(ProviderSettings::Aliyun(with_api_key(
            AliyunConfig::new()?,
            api_key,
            AliyunConfig::with_api_key,
        ))),
        BuiltinProvider::Tencent => Ok(ProviderSettings::Tencent(with_api_key(
            TencentConfig::new()?,
            api_key,
            TencentConfig::with_api_key,
        ))),
        BuiltinProvider::Firecrawl => Ok(ProviderSettings::Firecrawl(with_api_key(
            FirecrawlConfig::new()?,
            api_key,
            FirecrawlConfig::with_api_key,
        ))),
        BuiltinProvider::Doubao => {
            let mut provider_config =
                with_api_key(DoubaoConfig::new()?, api_key, DoubaoConfig::with_api_key);
            if let Some(authoritative_only) = engine.authoritative_only {
                provider_config = provider_config.with_authoritative_only(authoritative_only);
            }
            if let Some(query_rewrite) = engine.query_rewrite {
                provider_config = provider_config.with_query_rewrite(query_rewrite);
            }
            Ok(ProviderSettings::Doubao(provider_config))
        }
        BuiltinProvider::DeepSeek => Ok(ProviderSettings::DeepSeek(with_api_key(
            DeepSeekConfig::new()?,
            api_key,
            DeepSeekConfig::with_api_key,
        ))),
        BuiltinProvider::A3s => {
            let mut provider_config = A3sConfig::new()?;
            if let Some(endpoint) = endpoint {
                provider_config = provider_config.with_endpoint(endpoint)?;
            }
            Ok(ProviderSettings::A3s(with_api_key(
                provider_config,
                api_key,
                A3sConfig::with_api_key,
            )))
        }
        // BuiltinProvider is non_exhaustive; refuse unknown future providers fail-closed.
        _ => Err(SearchError::Other(
            "unsupported builtin search provider".into(),
        )),
    }
}
