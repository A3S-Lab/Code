use super::{CodeConfig, ConfigSection, ModelConfig, ProviderConfig, StorageBackend};
use crate::mcp::{McpServerConfig, McpTransportConfig, OAuthConfig};
use crate::queue::{
    LaneHandlerConfig, PriorityBoostConfig, RateLimitConfig, RetryPolicyConfig, SessionLane,
    TaskHandlerMode,
};
use a3s_acl::{Block, Document, Value};
use std::collections::HashMap;
use std::path::Path;

pub(super) struct SingleEntry {
    pub(super) names: &'static [&'static str],
    pub(super) text: Option<String>,
}

pub(super) fn render_single_section(
    section: ConfigSection,
    config: &CodeConfig,
    original: &CodeConfig,
    document: &Document,
) -> Vec<SingleEntry> {
    match section {
        ConfigSection::DefaultModel => vec![scalar_entry(
            &["default_model", "defaultModel"],
            "default_model",
            config.default_model.as_deref().map(string),
        )],
        ConfigSection::ModelRuntime => vec![
            scalar_entry(
                &["thinking_budget", "thinkingBudget"],
                "thinking_budget",
                config.thinking_budget.map(number),
            ),
            scalar_entry(
                &[
                    "llm_api_timeout_ms",
                    "llmApiTimeoutMs",
                    "api_timeout_ms",
                    "model_api_timeout_ms",
                ],
                "llm_api_timeout_ms",
                config.llm_api_timeout_ms.map(number),
            ),
        ],
        ConfigSection::Execution => render_execution(config, document),
        ConfigSection::Storage => render_storage(config, original, document),
        ConfigSection::Memory => vec![block_entry(&["memory"], render_memory(config, document))],
        ConfigSection::Queue => vec![block_entry(&["queue"], render_queue(config, document))],
        ConfigSection::Search => vec![block_entry(&["search"], render_search(config, document))],
        ConfigSection::Os => vec![scalar_entry(
            &["os"],
            "os",
            config.os.as_ref().map(|os| string(&os.address)),
        )],
        ConfigSection::DocumentParser => vec![block_entry(
            &["document_parser", "documentParser"],
            render_document_parser(config, document),
        )],
        ConfigSection::Providers | ConfigSection::McpServers => Vec::new(),
    }
}

pub(super) fn render_labeled_section(
    section: ConfigSection,
    config: &CodeConfig,
    original: &CodeConfig,
    document: &Document,
) -> Vec<(String, String)> {
    match section {
        ConfigSection::Providers => config
            .providers
            .iter()
            .map(|provider| {
                let block = render_provider(provider, original, document);
                (provider.name.clone(), generate_block(block))
            })
            .collect(),
        ConfigSection::McpServers => config
            .mcp_servers
            .iter()
            .map(|server| {
                let block = render_mcp_server(server, original, document);
                (server.name.clone(), generate_block(block))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn render_execution(config: &CodeConfig, document: &Document) -> Vec<SingleEntry> {
    vec![
        scalar_entry(
            &["skill_dirs", "skills", "skillDirs"],
            "skill_dirs",
            (!config.skill_dirs.is_empty()).then(|| path_list(&config.skill_dirs)),
        ),
        scalar_entry(
            &["agent_dirs", "agentDirs"],
            "agent_dirs",
            (!config.agent_dirs.is_empty()).then(|| path_list(&config.agent_dirs)),
        ),
        scalar_entry(
            &["max_tool_rounds", "maxToolRounds"],
            "max_tool_rounds",
            config.max_tool_rounds.map(number),
        ),
        scalar_entry(
            &["max_parallel_tasks", "maxParallelTasks"],
            "max_parallel_tasks",
            config.max_parallel_tasks.map(number),
        ),
        scalar_entry(
            &["auto_parallel", "autoParallel", "auto_parallel_enabled"],
            "auto_parallel",
            config.auto_parallel.map(Value::Bool),
        ),
        block_entry(
            &["auto_delegation", "autoDelegation"],
            Some(render_auto_delegation(config, document)),
        ),
    ]
}

fn render_auto_delegation(config: &CodeConfig, document: &Document) -> Block {
    let mut block = base_block(
        document,
        &["auto_delegation", "autoDelegation"],
        None,
        "auto_delegation",
    );
    set_attr(
        &mut block,
        &["enabled"],
        "enabled",
        Some(Value::Bool(config.auto_delegation.enabled)),
    );
    set_attr(
        &mut block,
        &["auto_parallel", "autoParallel", "parallel"],
        "auto_parallel",
        Some(Value::Bool(config.auto_delegation.auto_parallel)),
    );
    set_attr(
        &mut block,
        &[
            "allow_manual_delegation",
            "allowManualDelegation",
            "manual_delegation",
            "manualDelegation",
        ],
        "allow_manual_delegation",
        Some(Value::Bool(config.auto_delegation.allow_manual_delegation)),
    );
    set_attr(
        &mut block,
        &["min_confidence", "minConfidence"],
        "min_confidence",
        Some(float32(config.auto_delegation.min_confidence)),
    );
    set_attr(
        &mut block,
        &["max_tasks", "maxTasks"],
        "max_tasks",
        Some(number(config.auto_delegation.max_tasks)),
    );
    block
}

fn render_storage(
    config: &CodeConfig,
    original: &CodeConfig,
    document: &Document,
) -> Vec<SingleEntry> {
    let original_storage_url = find_block(document, &["storage_url", "storageUrl"], None)
        .and_then(|block| find_attr(block, &["storage_url", "storageUrl"]));
    vec![
        scalar_entry(
            &["storage_backend", "storageBackend"],
            "storage_backend",
            Some(string(match config.storage_backend {
                StorageBackend::Memory => "memory",
                StorageBackend::File => "file",
                StorageBackend::Custom => "custom",
            })),
        ),
        scalar_entry(
            &["sessions_dir", "sessionsDir"],
            "sessions_dir",
            config.sessions_dir.as_deref().map(path_value),
        ),
        scalar_entry(
            &["memory_dir", "memoryDir"],
            "memory_dir",
            config.memory_dir.as_deref().map(path_value),
        ),
        scalar_entry(
            &["storage_url", "storageUrl"],
            "storage_url",
            preserved_string(
                config.storage_url.as_deref(),
                original.storage_url.as_deref(),
                original_storage_url,
            ),
        ),
    ]
}

fn render_memory(config: &CodeConfig, document: &Document) -> Option<Block> {
    let memory = config.memory.as_ref()?;
    let mut block = base_block(document, &["memory"], None, "memory");
    set_attr(
        &mut block,
        &["max_short_term", "maxShortTerm"],
        "max_short_term",
        Some(number(memory.max_short_term)),
    );
    set_attr(
        &mut block,
        &["max_working", "maxWorking"],
        "max_working",
        Some(number(memory.max_working)),
    );
    set_attr(
        &mut block,
        &["prune_interval_secs", "pruneIntervalSecs"],
        "prune_interval_secs",
        Some(number(memory.prune_interval_secs)),
    );
    set_attr(
        &mut block,
        &["llm_extraction", "llmExtraction"],
        "llm_extraction",
        Some(Value::Bool(memory.llm_extraction)),
    );
    set_attr(
        &mut block,
        &["llm_extraction_max_items", "llmExtractionMaxItems"],
        "llm_extraction_max_items",
        Some(number(memory.llm_extraction_max_items)),
    );
    set_attr(
        &mut block,
        &[
            "llm_extraction_max_input_chars",
            "llmExtractionMaxInputChars",
        ],
        "llm_extraction_max_input_chars",
        Some(number(memory.llm_extraction_max_input_chars)),
    );
    remove_attrs(
        &mut block,
        &["relevance", "prune", "prune_policy", "prunePolicy"],
    );
    replace_nested(
        &mut block,
        &["relevance"],
        Some(simple_block(
            "relevance",
            [
                ("decay_days", float32(memory.relevance.decay_days)),
                (
                    "importance_weight",
                    float32(memory.relevance.importance_weight),
                ),
                ("recency_weight", float32(memory.relevance.recency_weight)),
            ],
        )),
    );
    replace_nested(
        &mut block,
        &["prune", "prune_policy", "prunePolicy"],
        memory.prune_policy.as_ref().map(|policy| {
            simple_block(
                "prune_policy",
                [
                    ("max_age_days", number(policy.max_age_days)),
                    (
                        "min_importance_to_keep",
                        float32(policy.min_importance_to_keep),
                    ),
                    ("max_items", number(policy.max_items)),
                ],
            )
        }),
    );
    Some(block)
}

fn render_queue(config: &CodeConfig, document: &Document) -> Option<Block> {
    let queue = config.queue.as_ref()?;
    let mut block = base_block(document, &["queue"], None, "queue");
    set_attr(
        &mut block,
        &["control_max_concurrency", "controlMaxConcurrency"],
        "control_max_concurrency",
        Some(number(queue.control_max_concurrency)),
    );
    set_attr(
        &mut block,
        &["query_max_concurrency", "queryMaxConcurrency"],
        "query_max_concurrency",
        Some(number(queue.query_max_concurrency)),
    );
    set_attr(
        &mut block,
        &["execute_max_concurrency", "executeMaxConcurrency"],
        "execute_max_concurrency",
        Some(number(queue.execute_max_concurrency)),
    );
    set_attr(
        &mut block,
        &["generate_max_concurrency", "generateMaxConcurrency"],
        "generate_max_concurrency",
        Some(number(queue.generate_max_concurrency)),
    );
    set_attr(
        &mut block,
        &["enable_dlq", "enableDlq"],
        "enable_dlq",
        Some(Value::Bool(queue.enable_dlq)),
    );
    set_attr(
        &mut block,
        &["dlq_max_size", "dlqMaxSize"],
        "dlq_max_size",
        queue.dlq_max_size.map(number),
    );
    set_attr(
        &mut block,
        &["enable_metrics", "enableMetrics"],
        "enable_metrics",
        Some(Value::Bool(queue.enable_metrics)),
    );
    set_attr(
        &mut block,
        &["enable_alerts", "enableAlerts"],
        "enable_alerts",
        Some(Value::Bool(queue.enable_alerts)),
    );
    set_attr(
        &mut block,
        &["default_timeout_ms", "defaultTimeoutMs"],
        "default_timeout_ms",
        queue.default_timeout_ms.map(number),
    );
    set_attr(
        &mut block,
        &["storage_path", "storagePath"],
        "storage_path",
        queue.storage_path.as_deref().map(path_value),
    );
    set_attr(
        &mut block,
        &["pressure_threshold", "pressureThreshold"],
        "pressure_threshold",
        queue.pressure_threshold.map(number),
    );
    set_attr(
        &mut block,
        &["lane_timeouts", "laneTimeouts"],
        "lane_timeouts",
        (!queue.lane_timeouts.is_empty()).then(|| lane_timeout_object(&queue.lane_timeouts)),
    );
    replace_nested(
        &mut block,
        &["lane_handlers", "laneHandlers"],
        (!queue.lane_handlers.is_empty()).then(|| lane_handlers_block(&queue.lane_handlers)),
    );
    replace_nested(
        &mut block,
        &["retry_policy", "retryPolicy"],
        queue.retry_policy.as_ref().map(retry_policy_block),
    );
    replace_nested(
        &mut block,
        &["rate_limit", "rateLimit"],
        queue.rate_limit.as_ref().map(rate_limit_block),
    );
    replace_nested(
        &mut block,
        &["priority_boost", "priorityBoost"],
        queue.priority_boost.as_ref().map(priority_boost_block),
    );
    Some(block)
}

fn lane_handlers_block(handlers: &HashMap<SessionLane, LaneHandlerConfig>) -> Block {
    let mut block = empty_block("lane_handlers");
    for lane in lane_order() {
        if let Some(handler) = handlers.get(&lane) {
            block.blocks.push(simple_block(
                lane_name(lane),
                [
                    (
                        "mode",
                        string(match handler.mode {
                            TaskHandlerMode::Internal => "internal",
                            TaskHandlerMode::External => "external",
                            TaskHandlerMode::Hybrid => "hybrid",
                        }),
                    ),
                    ("timeout_ms", number(handler.timeout_ms)),
                ],
            ));
        }
    }
    block
}

fn lane_timeout_object(timeouts: &HashMap<SessionLane, u64>) -> Value {
    Value::Object(
        lane_order()
            .into_iter()
            .filter_map(|lane| {
                timeouts
                    .get(&lane)
                    .map(|timeout| (lane_name(lane).to_string(), number(*timeout)))
            })
            .collect(),
    )
}

fn retry_policy_block(policy: &RetryPolicyConfig) -> Block {
    let mut block = simple_block(
        "retry_policy",
        [
            ("strategy", string(&policy.strategy)),
            ("max_retries", number(policy.max_retries)),
            ("initial_delay_ms", number(policy.initial_delay_ms)),
        ],
    );
    set_attr(
        &mut block,
        &["fixed_delay_ms"],
        "fixed_delay_ms",
        policy.fixed_delay_ms.map(number),
    );
    block
}

fn rate_limit_block(rate_limit: &RateLimitConfig) -> Block {
    let mut block = simple_block(
        "rate_limit",
        [("limit_type", string(&rate_limit.limit_type))],
    );
    set_attr(
        &mut block,
        &["max_operations"],
        "max_operations",
        rate_limit.max_operations.map(number),
    );
    block
}

fn priority_boost_block(priority: &PriorityBoostConfig) -> Block {
    let mut block = simple_block("priority_boost", [("strategy", string(&priority.strategy))]);
    set_attr(
        &mut block,
        &["deadline_ms"],
        "deadline_ms",
        priority.deadline_ms.map(number),
    );
    block
}

fn render_search(config: &CodeConfig, document: &Document) -> Option<Block> {
    let search = config.search.as_ref()?;
    let mut block = base_block(document, &["search"], None, "search");
    set_attr(
        &mut block,
        &["timeout"],
        "timeout",
        Some(number(search.timeout)),
    );
    replace_nested(
        &mut block,
        &["health"],
        search.health.as_ref().map(|health| {
            simple_block(
                "health",
                [
                    ("max_failures", number(health.max_failures)),
                    ("suspend_seconds", number(health.suspend_seconds)),
                ],
            )
        }),
    );
    replace_nested(
        &mut block,
        &["headless"],
        search.headless.as_ref().map(|headless| {
            let mut headless_block = simple_block(
                "headless",
                [
                    (
                        "backend",
                        string(if headless.backend.is_lightpanda() {
                            "lightpanda"
                        } else {
                            "chrome"
                        }),
                    ),
                    ("max_tabs", number(headless.max_tabs)),
                    ("launch_args", string_list(&headless.launch_args)),
                ],
            );
            set_attr(
                &mut headless_block,
                &["browser_path"],
                "browser_path",
                headless.browser_path.as_deref().map(string),
            );
            set_attr(
                &mut headless_block,
                &["proxy_url"],
                "proxy_url",
                headless.proxy_url.as_deref().map(string),
            );
            headless_block
        }),
    );
    let mut engine_block = empty_block("engine");
    let mut engines = search.engines.iter().collect::<Vec<_>>();
    engines.sort_by(|left, right| left.0.cmp(right.0));
    for (name, engine) in engines {
        let mut item = simple_block(
            name,
            [
                ("enabled", Value::Bool(engine.enabled)),
                ("weight", Value::Number(engine.weight)),
            ],
        );
        set_attr(
            &mut item,
            &["timeout"],
            "timeout",
            engine.timeout.map(number),
        );
        engine_block.blocks.push(item);
    }
    replace_nested(
        &mut block,
        &["engine"],
        (!engine_block.blocks.is_empty()).then_some(engine_block),
    );
    Some(block)
}

fn render_document_parser(config: &CodeConfig, document: &Document) -> Option<Block> {
    let parser = config.document_parser.as_ref()?;
    let mut block = base_block(
        document,
        &["document_parser", "documentParser"],
        None,
        "document_parser",
    );
    set_attr(
        &mut block,
        &["enabled"],
        "enabled",
        Some(Value::Bool(parser.enabled)),
    );
    set_attr(
        &mut block,
        &["max_file_size_mb", "maxFileSizeMb"],
        "max_file_size_mb",
        Some(number(parser.max_file_size_mb)),
    );
    replace_nested(
        &mut block,
        &["cache"],
        parser.cache.as_ref().map(|cache| {
            let mut cache_block = simple_block("cache", [("enabled", Value::Bool(cache.enabled))]);
            set_attr(
                &mut cache_block,
                &["directory"],
                "directory",
                cache.directory.as_deref().map(path_value),
            );
            cache_block
        }),
    );
    replace_nested(&mut block, &["ocr"], None);
    Some(block)
}

fn render_provider(provider: &ProviderConfig, original: &CodeConfig, document: &Document) -> Block {
    let previous = original
        .providers
        .iter()
        .find(|item| item.name == provider.name);
    let mut block = base_block(document, &["providers"], Some(&provider.name), "providers");
    block.labels = vec![provider.name.clone()];
    let original_api_key = find_attr(&block, &["api_key", "apiKey"]).cloned();
    set_attr(
        &mut block,
        &["api_key", "apiKey"],
        "api_key",
        preserved_string(
            provider.api_key.as_deref(),
            previous.and_then(|provider| provider.api_key.as_deref()),
            original_api_key.as_ref(),
        ),
    );
    set_attr(
        &mut block,
        &["base_url", "baseUrl"],
        "base_url",
        provider.base_url.as_deref().map(string),
    );
    set_attr(
        &mut block,
        &["session_id_header", "sessionIdHeader"],
        "session_id_header",
        provider.session_id_header.as_deref().map(string),
    );
    let original_headers = find_attr(&block, &["headers"]).cloned();
    set_attr(
        &mut block,
        &["headers"],
        "headers",
        preserved_string_map_if_configured(
            &provider.headers,
            previous.map(|provider| &provider.headers),
            original_headers.as_ref(),
        ),
    );

    let original_models = block
        .blocks
        .iter()
        .filter(|child| child.name == "models")
        .cloned()
        .collect::<Vec<_>>();
    block.blocks.retain(|child| child.name != "models");
    for model in &provider.models {
        block.blocks.push(render_model(
            model,
            previous.and_then(|provider| provider.models.iter().find(|item| item.id == model.id)),
            original_models
                .iter()
                .find(|item| item.labels.first() == Some(&model.id)),
        ));
    }
    block
}

fn render_model(
    model: &ModelConfig,
    previous: Option<&ModelConfig>,
    original: Option<&Block>,
) -> Block {
    let mut block = original.cloned().unwrap_or_else(|| empty_block("models"));
    block.name = "models".to_string();
    block.labels = vec![model.id.clone()];
    set_attr(&mut block, &["name"], "name", Some(string(&model.name)));
    set_attr(
        &mut block,
        &["family"],
        "family",
        (!model.family.is_empty()).then(|| string(&model.family)),
    );
    let original_api_key = find_attr(&block, &["api_key", "apiKey"]).cloned();
    set_attr(
        &mut block,
        &["api_key", "apiKey"],
        "api_key",
        preserved_string(
            model.api_key.as_deref(),
            previous.and_then(|model| model.api_key.as_deref()),
            original_api_key.as_ref(),
        ),
    );
    set_attr(
        &mut block,
        &["base_url", "baseUrl"],
        "base_url",
        model.base_url.as_deref().map(string),
    );
    set_attr(
        &mut block,
        &["session_id_header", "sessionIdHeader"],
        "session_id_header",
        model.session_id_header.as_deref().map(string),
    );
    let original_headers = find_attr(&block, &["headers"]).cloned();
    set_attr(
        &mut block,
        &["headers"],
        "headers",
        preserved_string_map_if_configured(
            &model.headers,
            previous.map(|model| &model.headers),
            original_headers.as_ref(),
        ),
    );
    set_attr(
        &mut block,
        &["attachment"],
        "attachment",
        Some(Value::Bool(model.attachment)),
    );
    set_attr(
        &mut block,
        &["reasoning"],
        "reasoning",
        Some(Value::Bool(model.reasoning)),
    );
    set_attr(
        &mut block,
        &["tool_call", "toolCall"],
        "tool_call",
        Some(Value::Bool(model.tool_call)),
    );
    set_attr(
        &mut block,
        &["temperature"],
        "temperature",
        Some(Value::Bool(model.temperature)),
    );
    set_attr(
        &mut block,
        &["release_date", "releaseDate"],
        "release_date",
        model.release_date.as_deref().map(string),
    );
    set_attr(
        &mut block,
        &["modalities"],
        "modalities",
        Some(Value::Object(vec![
            ("input".to_string(), string_list(&model.modalities.input)),
            ("output".to_string(), string_list(&model.modalities.output)),
        ])),
    );
    set_attr(
        &mut block,
        &["cost"],
        "cost",
        Some(Value::Object(vec![
            ("input".to_string(), Value::Number(model.cost.input)),
            ("output".to_string(), Value::Number(model.cost.output)),
            (
                "cache_read".to_string(),
                Value::Number(model.cost.cache_read),
            ),
            (
                "cache_write".to_string(),
                Value::Number(model.cost.cache_write),
            ),
        ])),
    );
    remove_attrs(&mut block, &["maxTokens", "contextTokens"]);
    set_attr(
        &mut block,
        &["limit"],
        "limit",
        Some(Value::Object(vec![
            ("context".to_string(), number(model.limit.context)),
            ("output".to_string(), number(model.limit.output)),
        ])),
    );
    block
}

fn render_mcp_server(
    server: &McpServerConfig,
    original: &CodeConfig,
    document: &Document,
) -> Block {
    let previous = original
        .mcp_servers
        .iter()
        .find(|item| item.name == server.name);
    let mut block = base_block(
        document,
        &["mcp_servers", "mcpServers", "mcp_server"],
        Some(&server.name),
        "mcp_servers",
    );
    block.labels = vec![server.name.clone()];
    let original_headers = find_attr(&block, &["headers"]).cloned();
    remove_attrs(
        &mut block,
        &["transport", "command", "args", "url", "headers"],
    );
    match &server.transport {
        McpTransportConfig::Stdio { command, args } => {
            block
                .attributes
                .insert("transport".to_string(), string("stdio"));
            block
                .attributes
                .insert("command".to_string(), string(command));
            block
                .attributes
                .insert("args".to_string(), string_list(args));
        }
        McpTransportConfig::Http { url, headers } => {
            block
                .attributes
                .insert("transport".to_string(), string("http"));
            block.attributes.insert("url".to_string(), string(url));
            if let Some(headers) = preserved_string_map_if_configured(
                headers,
                previous.and_then(|server| transport_headers(&server.transport)),
                original_headers.as_ref(),
            ) {
                block.attributes.insert("headers".to_string(), headers);
            }
        }
        McpTransportConfig::StreamableHttp { url, headers } => {
            block
                .attributes
                .insert("transport".to_string(), string("streamable-http"));
            block.attributes.insert("url".to_string(), string(url));
            if let Some(headers) = preserved_string_map_if_configured(
                headers,
                previous.and_then(|server| transport_headers(&server.transport)),
                original_headers.as_ref(),
            ) {
                block.attributes.insert("headers".to_string(), headers);
            }
        }
    }
    set_attr(
        &mut block,
        &["enabled"],
        "enabled",
        Some(Value::Bool(server.enabled)),
    );
    let original_env = find_attr(&block, &["env"]).cloned();
    set_attr(
        &mut block,
        &["env"],
        "env",
        preserved_string_map_if_configured(
            &server.env,
            previous.map(|server| &server.env),
            original_env.as_ref(),
        ),
    );
    set_attr(
        &mut block,
        &["tool_timeout_secs", "toolTimeoutSecs"],
        "tool_timeout_secs",
        Some(number(server.tool_timeout_secs)),
    );
    let original_oauth = find_nested(&block, &["oauth"], None).cloned();
    replace_nested(
        &mut block,
        &["oauth"],
        server.oauth.as_ref().map(|oauth| {
            render_oauth(
                oauth,
                previous.and_then(|server| server.oauth.as_ref()),
                original_oauth.as_ref(),
            )
        }),
    );
    block
}

fn render_oauth(
    oauth: &OAuthConfig,
    previous: Option<&OAuthConfig>,
    original: Option<&Block>,
) -> Block {
    let mut block = original.cloned().unwrap_or_else(|| empty_block("oauth"));
    block.name = "oauth".to_string();
    set_attr(
        &mut block,
        &["auth_url", "authUrl"],
        "auth_url",
        Some(string(&oauth.auth_url)),
    );
    set_attr(
        &mut block,
        &["token_url", "tokenUrl"],
        "token_url",
        Some(string(&oauth.token_url)),
    );
    set_attr(
        &mut block,
        &["client_id", "clientId"],
        "client_id",
        Some(string(&oauth.client_id)),
    );
    let original_client_secret = find_attr(&block, &["client_secret", "clientSecret"]).cloned();
    set_attr(
        &mut block,
        &["client_secret", "clientSecret"],
        "client_secret",
        preserved_string(
            oauth.client_secret.as_deref(),
            previous.and_then(|oauth| oauth.client_secret.as_deref()),
            original_client_secret.as_ref(),
        ),
    );
    set_attr(
        &mut block,
        &["scopes"],
        "scopes",
        Some(string_list(&oauth.scopes)),
    );
    set_attr(
        &mut block,
        &["redirect_uri", "redirectUri"],
        "redirect_uri",
        Some(string(&oauth.redirect_uri)),
    );
    let original_access_token = find_attr(&block, &["access_token", "accessToken"]).cloned();
    set_attr(
        &mut block,
        &["access_token", "accessToken"],
        "access_token",
        preserved_string(
            oauth.access_token.as_deref(),
            previous.and_then(|oauth| oauth.access_token.as_deref()),
            original_access_token.as_ref(),
        ),
    );
    block
}

fn transport_headers(transport: &McpTransportConfig) -> Option<&HashMap<String, String>> {
    match transport {
        McpTransportConfig::Http { headers, .. }
        | McpTransportConfig::StreamableHttp { headers, .. } => Some(headers),
        McpTransportConfig::Stdio { .. } => None,
    }
}

fn preserved_string(
    updated: Option<&str>,
    previous: Option<&str>,
    original: Option<&Value>,
) -> Option<Value> {
    if updated == previous && matches!(original, Some(Value::Call(_, _))) {
        return original.cloned();
    }
    updated.map(string)
}

fn preserved_string_map(
    updated: &HashMap<String, String>,
    previous: Option<&HashMap<String, String>>,
    original: Option<&Value>,
) -> Value {
    let original_pairs: &[(String, Value)] = match original {
        Some(Value::Object(pairs)) => pairs.as_slice(),
        _ => &[],
    };
    let mut pairs = Vec::new();
    let mut entries = updated.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    for (key, value) in entries {
        let original_value = original_pairs
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value);
        let previous_value = previous
            .and_then(|values| values.get(key))
            .map(String::as_str);
        pairs.push((
            key.clone(),
            preserved_string(Some(value), previous_value, original_value)
                .unwrap_or_else(|| string(value)),
        ));
    }
    for (key, value) in original_pairs {
        if !updated.contains_key(key)
            && previous.is_some_and(|values| !values.contains_key(key))
            && matches!(value, Value::Call(_, _))
        {
            pairs.push((key.clone(), value.clone()));
        }
    }
    Value::Object(pairs)
}

fn preserved_string_map_if_configured(
    updated: &HashMap<String, String>,
    previous: Option<&HashMap<String, String>>,
    original: Option<&Value>,
) -> Option<Value> {
    let value = preserved_string_map(updated, previous, original);
    match &value {
        Value::Object(pairs) if pairs.is_empty() => None,
        _ => Some(value),
    }
}

fn scalar_entry(
    names: &'static [&'static str],
    canonical: &str,
    value: Option<Value>,
) -> SingleEntry {
    SingleEntry {
        names,
        text: value.map(|value| format!("{canonical} = {}\n", render_value(&value))),
    }
}

fn block_entry(names: &'static [&'static str], block: Option<Block>) -> SingleEntry {
    SingleEntry {
        names,
        text: block.map(generate_block),
    }
}

fn generate_block(block: Block) -> String {
    a3s_acl::generate_acl(&Document {
        blocks: vec![block],
    })
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(value) => format!("\"{}\"", escape_string(value)),
        Value::Number(value) if value.fract() == 0.0 => format!("{}", *value as i64),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(render_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(items) => format!(
            "{{ {} }}",
            items
                .iter()
                .map(|(key, value)| format!("{key} = {}", render_value(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Null => "null".to_string(),
        Value::Call(name, args) => format!(
            "{name}({})",
            args.iter().map(render_value).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn escape_string(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            _ => vec![ch],
        })
        .collect()
}

fn base_block(document: &Document, names: &[&str], label: Option<&str>, canonical: &str) -> Block {
    let mut block = find_block(document, names, label)
        .cloned()
        .unwrap_or_else(|| empty_block(canonical));
    block.name = canonical.to_string();
    if let Some(label) = label {
        block.labels = vec![label.to_string()];
    }
    block
}

fn find_block<'a>(
    document: &'a Document,
    names: &[&str],
    label: Option<&str>,
) -> Option<&'a Block> {
    document.blocks.iter().find(|block| {
        names.contains(&block.name.as_str())
            && label
                .map(|label| block.labels.first().map(String::as_str) == Some(label))
                .unwrap_or(block.labels.is_empty())
    })
}

fn find_nested<'a>(block: &'a Block, names: &[&str], label: Option<&str>) -> Option<&'a Block> {
    block.blocks.iter().find(|child| {
        names.contains(&child.name.as_str())
            && label
                .map(|label| child.labels.first().map(String::as_str) == Some(label))
                .unwrap_or(child.labels.is_empty())
    })
}

fn find_attr<'a>(block: &'a Block, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| block.attributes.get(*name))
}

fn set_attr(block: &mut Block, aliases: &[&str], canonical: &str, value: Option<Value>) {
    remove_attrs(block, aliases);
    if let Some(value) = value {
        block.attributes.insert(canonical.to_string(), value);
    }
}

fn remove_attrs(block: &mut Block, aliases: &[&str]) {
    block
        .attributes
        .retain(|key, _| !aliases.contains(&key.as_str()));
}

fn replace_nested(block: &mut Block, aliases: &[&str], value: Option<Block>) {
    block
        .blocks
        .retain(|child| !aliases.contains(&child.name.as_str()));
    if let Some(value) = value {
        block.blocks.push(value);
    }
}

fn empty_block(name: &str) -> Block {
    Block {
        name: name.to_string(),
        labels: Vec::new(),
        blocks: Vec::new(),
        attributes: HashMap::new(),
    }
}

fn simple_block<const N: usize>(name: &str, values: [(&str, Value); N]) -> Block {
    Block {
        name: name.to_string(),
        labels: Vec::new(),
        blocks: Vec::new(),
        attributes: values
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    }
}

fn string(value: &str) -> Value {
    Value::String(value.to_string())
}

fn float32(value: f32) -> Value {
    let value = value
        .to_string()
        .parse::<f64>()
        .unwrap_or_else(|_| f64::from(value));
    Value::Number(value)
}

trait AclNumber {
    fn to_acl_number(self) -> f64;
}

macro_rules! impl_acl_number {
    ($($type:ty),+ $(,)?) => {
        $(
            impl AclNumber for $type {
                fn to_acl_number(self) -> f64 {
                    self as f64
                }
            }
        )+
    };
}

impl_acl_number!(u32, u64, usize);

fn number(value: impl AclNumber) -> Value {
    Value::Number(value.to_acl_number())
}
fn path_value(value: &Path) -> Value {
    string(&value.to_string_lossy())
}
fn path_list(values: &[std::path::PathBuf]) -> Value {
    Value::List(values.iter().map(|value| path_value(value)).collect())
}
fn string_list(values: &[String]) -> Value {
    Value::List(values.iter().map(|value| string(value)).collect())
}
fn lane_order() -> [SessionLane; 4] {
    [
        SessionLane::Control,
        SessionLane::Query,
        SessionLane::Execute,
        SessionLane::Generate,
    ]
}
fn lane_name(lane: SessionLane) -> &'static str {
    match lane {
        SessionLane::Control => "control",
        SessionLane::Query => "query",
        SessionLane::Execute => "execute",
        SessionLane::Generate => "generate",
    }
}
