use super::provider::{
    apply_model_caps, ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig,
};
use super::{AutoDelegationConfig, CodeConfig, OsConfig, StorageBackend};
use crate::error::{CodeError, Result};
use crate::llm::LlmConfig;
use crate::mcp::McpServerConfig;
use crate::memory::MemoryConfig;
use crate::queue::SessionQueueConfig;
use crate::task_scheduler::TaskSchedulerConfig;
use a3s_memory::{PrunePolicy, RelevanceConfig};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ============================================================================
// ACL Parsing Helpers
// ============================================================================

fn acl_attr<'a>(block: &'a a3s_acl::Block, keys: &[&str]) -> Option<&'a a3s_acl::Value> {
    keys.iter().find_map(|key| block.attributes.get(*key))
}

fn acl_string(value: &a3s_acl::Value) -> Option<String> {
    match value {
        a3s_acl::Value::String(s) => Some(s.clone()),
        a3s_acl::Value::Call(name, args) if name == "env" => {
            let var_name = args.first().and_then(acl_string)?;
            match std::env::var(&var_name) {
                Ok(resolved) => Some(resolved),
                Err(_) => {
                    // Keep optional positions working (an unset env drops the
                    // attribute), but never silently: an unset required
                    // secret otherwise surfaces much later as an opaque
                    // missing-LlmConfig error far from its cause.
                    tracing::warn!(
                        var = %var_name,
                        "ACL env() reference is unset; the attribute is dropped"
                    );
                    None
                }
            }
        }
        _ => None,
    }
}

fn acl_string_attr(block: &a3s_acl::Block, keys: &[&str]) -> Option<String> {
    acl_attr(block, keys).and_then(acl_string)
}

fn acl_label_or_attr(block: &a3s_acl::Block, keys: &[&str]) -> Option<String> {
    block
        .labels
        .first()
        .cloned()
        .or_else(|| acl_string_attr(block, keys))
}

fn acl_bool_attr(block: &a3s_acl::Block, keys: &[&str]) -> Option<bool> {
    match acl_attr(block, keys) {
        Some(a3s_acl::Value::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn acl_usize_attr(block: &a3s_acl::Block, keys: &[&str]) -> Option<usize> {
    match acl_attr(block, keys) {
        Some(a3s_acl::Value::Number(value)) if *value >= 0.0 => Some(*value as usize),
        _ => None,
    }
}

fn acl_f32_attr(block: &a3s_acl::Block, keys: &[&str]) -> Option<f32> {
    match acl_attr(block, keys) {
        Some(a3s_acl::Value::Number(value)) => Some(*value as f32),
        _ => None,
    }
}

fn parse_auto_delegation_block(
    block: &a3s_acl::Block,
    base: &AutoDelegationConfig,
) -> AutoDelegationConfig {
    let mut config = base.clone();
    if let Some(enabled) = acl_bool_attr(block, &["enabled"]) {
        config.enabled = enabled;
    }
    if let Some(auto_parallel) =
        acl_bool_attr(block, &["auto_parallel", "autoParallel", "parallel"])
    {
        config.auto_parallel = auto_parallel;
    }
    if let Some(allow_manual_delegation) = acl_bool_attr(
        block,
        &[
            "allow_manual_delegation",
            "allowManualDelegation",
            "manual_delegation",
            "manualDelegation",
        ],
    ) {
        config.allow_manual_delegation = allow_manual_delegation;
    }
    if let Some(min_confidence) = acl_f32_attr(block, &["min_confidence", "minConfidence"]) {
        config.min_confidence = min_confidence.clamp(0.0, 1.0);
    }
    if let Some(max_tasks) = acl_usize_attr(block, &["max_tasks", "maxTasks"]) {
        config.max_tasks = max_tasks.max(1);
    }
    config
}

fn parse_memory_block(
    block: &a3s_acl::Block,
    base: Option<&MemoryConfig>,
) -> std::result::Result<MemoryConfig, CodeError> {
    let mut config = base.cloned().unwrap_or_default();

    if let Some(max_short_term) = acl_usize_attr(block, &["max_short_term", "maxShortTerm"]) {
        config.max_short_term = max_short_term;
    }
    if let Some(max_working) = acl_usize_attr(block, &["max_working", "maxWorking"]) {
        config.max_working = max_working;
    }
    if let Some(prune_interval_secs) =
        acl_usize_attr(block, &["prune_interval_secs", "pruneIntervalSecs"])
    {
        config.prune_interval_secs = prune_interval_secs as u64;
    }
    if let Some(llm_extraction) = acl_bool_attr(block, &["llm_extraction", "llmExtraction"]) {
        config.llm_extraction = llm_extraction;
    }
    if let Some(max_items) = acl_usize_attr(
        block,
        &["llm_extraction_max_items", "llmExtractionMaxItems"],
    ) {
        config.llm_extraction_max_items = max_items;
    }
    if let Some(max_input_chars) = acl_usize_attr(
        block,
        &[
            "llm_extraction_max_input_chars",
            "llmExtractionMaxInputChars",
        ],
    ) {
        config.llm_extraction_max_input_chars = max_input_chars;
    }

    if let Some(relevance) = acl_attr(block, &["relevance"]) {
        config.relevance = parse_relevance_value(relevance, &config.relevance);
    }

    if let Some(prune_policy) = acl_attr(block, &["prune", "prune_policy", "prunePolicy"]) {
        if let a3s_acl::Value::String(word) = prune_policy {
            config.prune_policy = match word.trim().to_ascii_lowercase().as_str() {
                "off" | "none" | "disabled" => None,
                other => {
                    return Err(CodeError::Config(format!(
                        "memory block: prune string must be one of the closed set \
                         [off, none, disabled]; got {other:?} (an object tunes the policy)"
                    )))
                }
            };
        } else {
            config.prune_policy = Some(parse_prune_policy_value(
                prune_policy,
                config.prune_policy.as_ref(),
            ));
        }
    }

    for child in &block.blocks {
        let value = a3s_acl::Value::Object(
            child
                .attributes
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        match child.name.as_str() {
            "relevance" => {
                config.relevance = parse_relevance_value(&value, &config.relevance);
            }
            "prune" | "prune_policy" | "prunePolicy" => {
                config.prune_policy = Some(parse_prune_policy_value(
                    &value,
                    config.prune_policy.as_ref(),
                ));
            }
            _ => {}
        }
    }

    Ok(config)
}

fn parse_relevance_value(value: &a3s_acl::Value, base: &RelevanceConfig) -> RelevanceConfig {
    let mut config = base.clone();
    if let Some(decay_days) = acl_object_f32_attr(value, &["decay_days", "decayDays"]) {
        config.decay_days = decay_days.max(0.1);
    }
    if let Some(importance_weight) =
        acl_object_f32_attr(value, &["importance_weight", "importanceWeight"])
    {
        config.importance_weight = importance_weight.max(0.0);
    }
    if let Some(recency_weight) = acl_object_f32_attr(value, &["recency_weight", "recencyWeight"]) {
        config.recency_weight = recency_weight.max(0.0);
    }
    config
}

fn parse_prune_policy_value(value: &a3s_acl::Value, base: Option<&PrunePolicy>) -> PrunePolicy {
    let mut policy = base.cloned().unwrap_or_default();
    if let Some(max_age_days) = acl_object_u32_attr(value, &["max_age_days", "maxAgeDays"]) {
        policy.max_age_days = max_age_days;
    }
    if let Some(min_importance) =
        acl_object_f32_attr(value, &["min_importance_to_keep", "minImportanceToKeep"])
    {
        policy.min_importance_to_keep = min_importance.clamp(0.0, 1.0);
    }
    if let Some(max_items) = acl_object_usize_attr(value, &["max_items", "maxItems"]) {
        policy.max_items = max_items;
    }
    policy
}

fn acl_object_attr<'a>(value: &'a a3s_acl::Value, keys: &[&str]) -> Option<&'a a3s_acl::Value> {
    match value {
        a3s_acl::Value::Object(pairs) => keys.iter().find_map(|key| {
            pairs
                .iter()
                .find_map(|(candidate, value)| (candidate == key).then_some(value))
        }),
        _ => None,
    }
}

fn acl_f32(value: &a3s_acl::Value) -> Option<f32> {
    match value {
        a3s_acl::Value::Number(value) => Some(*value as f32),
        _ => None,
    }
}

fn acl_usize(value: &a3s_acl::Value) -> Option<usize> {
    match value {
        a3s_acl::Value::Number(value) if *value >= 0.0 => Some(*value as usize),
        _ => None,
    }
}

fn acl_u32(value: &a3s_acl::Value) -> Option<u32> {
    match value {
        a3s_acl::Value::Number(value) if *value >= 0.0 => {
            Some((*value as usize).min(u32::MAX as usize) as u32)
        }
        _ => None,
    }
}

fn acl_object_f32_attr(value: &a3s_acl::Value, keys: &[&str]) -> Option<f32> {
    acl_object_attr(value, keys).and_then(acl_f32)
}

fn acl_object_usize_attr(value: &a3s_acl::Value, keys: &[&str]) -> Option<usize> {
    acl_object_attr(value, keys).and_then(acl_usize)
}

fn acl_object_u32_attr(value: &a3s_acl::Value, keys: &[&str]) -> Option<u32> {
    acl_object_attr(value, keys).and_then(acl_u32)
}

fn acl_path_list_attr(block: &a3s_acl::Block, keys: &[&str]) -> Option<Vec<PathBuf>> {
    let value = acl_attr(block, keys)?;
    match value {
        a3s_acl::Value::List(items) => Some(
            items
                .iter()
                .filter_map(acl_string)
                .map(PathBuf::from)
                .collect(),
        ),
        _ => acl_string(value).map(|s| vec![PathBuf::from(s)]),
    }
}

fn acl_string_list_attr(block: &a3s_acl::Block, keys: &[&str]) -> Option<Vec<String>> {
    let value = acl_attr(block, keys)?;
    match value {
        a3s_acl::Value::List(items) => Some(items.iter().filter_map(acl_string).collect()),
        _ => acl_string(value).map(|value| vec![value]),
    }
}

fn snake_to_camel(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut uppercase_next = false;
    for ch in value.chars() {
        if ch == '_' || ch == '-' {
            uppercase_next = true;
        } else if uppercase_next {
            output.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            output.push(ch);
        }
    }
    output
}

fn acl_value_to_json(value: &a3s_acl::Value) -> Option<JsonValue> {
    match value {
        a3s_acl::Value::String(value) => Some(JsonValue::String(value.clone())),
        a3s_acl::Value::Number(value) if value.fract() == 0.0 && *value >= 0.0 => {
            Some(JsonValue::Number(serde_json::Number::from(*value as u64)))
        }
        a3s_acl::Value::Number(value) if value.fract() == 0.0 => {
            Some(JsonValue::Number(serde_json::Number::from(*value as i64)))
        }
        a3s_acl::Value::Number(value) => {
            serde_json::Number::from_f64(*value).map(JsonValue::Number)
        }
        a3s_acl::Value::Bool(value) => Some(JsonValue::Bool(*value)),
        a3s_acl::Value::List(items) => Some(JsonValue::Array(
            items.iter().filter_map(acl_value_to_json).collect(),
        )),
        a3s_acl::Value::Object(pairs) => {
            let mut object = JsonMap::new();
            for (key, value) in pairs {
                if let Some(value) = acl_value_to_json(value) {
                    object.insert(key.clone(), value);
                }
            }
            Some(JsonValue::Object(object))
        }
        a3s_acl::Value::Null => Some(JsonValue::Null),
        a3s_acl::Value::Call(name, _) if name == "env" => acl_string(value).map(JsonValue::String),
        a3s_acl::Value::Call(_, _) => None,
    }
}

fn insert_nested_json(object: &mut JsonMap<String, JsonValue>, key: String, value: JsonValue) {
    match object.remove(&key) {
        None => {
            object.insert(key, value);
        }
        Some(JsonValue::Array(mut values)) => {
            values.push(value);
            object.insert(key, JsonValue::Array(values));
        }
        Some(previous) => {
            object.insert(key, JsonValue::Array(vec![previous, value]));
        }
    }
}

fn acl_block_to_json(block: &a3s_acl::Block) -> JsonValue {
    let mut object = JsonMap::new();
    for (key, value) in &block.attributes {
        if let Some(value) = acl_value_to_json(value) {
            object.insert(snake_to_camel(key), value);
        }
    }

    for child in &block.blocks {
        let key = snake_to_camel(&child.name);
        let value = acl_block_to_json(child);
        if let Some(label) = child.labels.first() {
            let entry = object
                .entry(key)
                .or_insert_with(|| JsonValue::Object(JsonMap::new()));
            if let JsonValue::Object(entries) = entry {
                entries.insert(label.clone(), value);
            }
        } else {
            insert_nested_json(&mut object, key, value);
        }
    }

    JsonValue::Object(object)
}

fn normalize_lane_name(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "control" => Some("Control"),
        "query" => Some("Query"),
        "execute" => Some("Execute"),
        "generate" => Some("Generate"),
        _ => None,
    }
}

fn normalize_lane_map(value: &mut JsonValue, normalize_handler_mode: bool) -> Result<()> {
    let JsonValue::Object(entries) = value else {
        return Ok(());
    };
    let previous = std::mem::take(entries);
    for (raw_name, mut value) in previous {
        let Some(name) = normalize_lane_name(&raw_name) else {
            return Err(CodeError::Config(format!(
                "queue lane `{raw_name}` is not in the closed set control, query, execute, generate"
            )));
        };
        if normalize_handler_mode {
            if let JsonValue::Object(handler) = &mut value {
                rename_json_key(handler, "timeoutMs", "timeout_ms");
                if let Some(JsonValue::String(mode)) = handler.get_mut("mode") {
                    let normalized = mode.trim().to_ascii_lowercase();
                    *mode = match normalized.as_str() {
                        "external" => "External".to_string(),
                        "hybrid" => "Hybrid".to_string(),
                        "internal" => "Internal".to_string(),
                        other => {
                            return Err(CodeError::Config(format!(
                                "queue handler mode `{other}` is not in the closed set internal, external, hybrid"
                            )));
                        }
                    };
                }
            }
        }
        entries.insert(name.to_string(), value);
    }
    Ok(())
}

fn parse_queue_block(block: &a3s_acl::Block) -> Result<SessionQueueConfig> {
    let mut value = acl_block_to_json(block);
    if let Some(lane_handlers) = value.get_mut("laneHandlers") {
        normalize_lane_map(lane_handlers, true)?;
    }
    if let Some(lane_timeouts) = value.get_mut("laneTimeouts") {
        normalize_lane_map(lane_timeouts, false)?;
    }
    serde_json::from_value(value)
        .map_err(|error| CodeError::Config(format!("Invalid queue configuration: {error}")))
}

fn parse_task_scheduler_block(block: &a3s_acl::Block) -> Result<TaskSchedulerConfig> {
    let config: TaskSchedulerConfig =
        serde_json::from_value(acl_block_to_json(block)).map_err(|error| {
            CodeError::Config(format!("Invalid task scheduler configuration: {error}"))
        })?;
    config
        .validate()
        .map_err(|error| CodeError::Config(error.to_string()))?;
    Ok(config)
}

fn parse_search_block(block: &a3s_acl::Block) -> Result<super::SearchConfig> {
    serde_json::from_value(acl_block_to_json(block))
        .map_err(|error| CodeError::Config(format!("Invalid search configuration: {error}")))
}

fn parse_document_parser_block(block: &a3s_acl::Block) -> Result<super::DocumentParserConfig> {
    serde_json::from_value(acl_block_to_json(block)).map_err(|error| {
        CodeError::Config(format!("Invalid document parser configuration: {error}"))
    })
}

fn rename_json_key(object: &mut JsonMap<String, JsonValue>, from: &str, to: &str) {
    if let Some(value) = object.remove(from) {
        object.insert(to.to_string(), value);
    }
}

pub(crate) fn parse_mcp_server_block(block: &a3s_acl::Block) -> Result<McpServerConfig> {
    let mut value = acl_block_to_json(block);
    let object = value.as_object_mut().ok_or_else(|| {
        CodeError::Config("Invalid MCP server configuration: expected an object".to_string())
    })?;
    if let Some(label) = block.labels.first() {
        object.insert("name".to_string(), JsonValue::String(label.clone()));
    }
    if let Some(JsonValue::Object(oauth)) = object.get_mut("oauth") {
        rename_json_key(oauth, "authUrl", "auth_url");
        rename_json_key(oauth, "tokenUrl", "token_url");
        rename_json_key(oauth, "clientId", "client_id");
        rename_json_key(oauth, "clientSecret", "client_secret");
        rename_json_key(oauth, "redirectUri", "redirect_uri");
        rename_json_key(oauth, "accessToken", "access_token");
    }
    serde_json::from_value(value)
        .map_err(|error| CodeError::Config(format!("Invalid MCP server configuration: {error}")))
}

fn acl_string_map(value: &a3s_acl::Value) -> HashMap<String, String> {
    match value {
        a3s_acl::Value::Object(pairs) => pairs
            .iter()
            .filter_map(|(key, value)| acl_string(value).map(|value| (key.clone(), value)))
            .collect(),
        _ => HashMap::new(),
    }
}

fn acl_string_list(value: &a3s_acl::Value) -> Vec<String> {
    match value {
        a3s_acl::Value::List(items) => items.iter().filter_map(acl_string).collect(),
        _ => Vec::new(),
    }
}

fn acl_object_f64_attr(value: &a3s_acl::Value, keys: &[&str]) -> Option<f64> {
    match acl_object_attr(value, keys) {
        Some(a3s_acl::Value::Number(value)) => Some(*value),
        _ => None,
    }
}

// ============================================================================
// Host-Authoritative Blocks
// ============================================================================

/// Every top-level ACL block the loader dispatches (canonical names and
/// aliases). The unknown-block refusal formats from this list, so the
/// message cannot drift from the dispatch again (it once named a stale
/// set — the triage rule's classic). Keep in sync when adding an arm.
const KNOWN_CONFIG_BLOCKS: &[&str] = &[
    "agent_dirs",
    "auto_delegation",
    "auto_parallel",
    "default_model",
    "goal_control",
    "document_parser",
    "llm_api_timeout_ms",
    "max_parallel_tasks",
    "max_tool_rounds",
    "memory",
    "memory_dir",
    "mcp_server",
    "mcp_servers",
    "mcpServers",
    "os",
    "project_doc_fallback_filenames",
    "project_doc_max_bytes",
    "providers",
    "queue",
    "sandbox",
    "search",
    "sessions_dir",
    "skill_dirs",
    "skills",
    "storage_backend",
    "storage_url",
    "task_scheduler",
    "thinking_budget",
    "user_instructions_dir",
    "workspace_retrieval",
    "notifications",
    "verification_contract",
];

/// Top-level ACL blocks that carry host-authoritative posture: only a
/// configuration layer the host consented to (`--config`/`A3S_CONFIG` or
/// `~/.a3s/config.acl`) may set them. An automatically discovered workspace
/// layer is untrusted repository content; it may configure the agent but
/// must not arm or reroute host surfaces: the `sandbox` block arms or
/// narrows egress, and the `providers` block routes the host's model
/// traffic and credentials to endpoints of its choosing. A workspace-scoped
/// `verification_contract` grants suite evidence, so it is also host-owned
/// (decision: manual/HOST_VERIFICATION_CONTRACT.md, 2026-10-12). Finite
/// `goal_control` bounds are host-owned (manual/HOST_GOAL_CONTROL.md). Extending this
/// set is a platform decision, not a per-feature one (the `providers`
/// extension was ratified 2026-10-09).
pub const HOST_AUTHORITATIVE_BLOCKS: &[&str] = &[
    "sandbox",
    "providers",
    "verification_contract",
    "goal_control",
];

/// The first host-authoritative top-level block an ACL document declares,
/// if any. Uses the real ACL parser, so prose or an attribute that merely
/// spells the block name is not a declaration, and only top-level blocks
/// count — nested blocks never reach the loader's dispatch arms.
pub fn host_authoritative_block(acl: &str) -> Option<&'static str> {
    let document = a3s_acl::parse_acl(acl).ok()?;
    HOST_AUTHORITATIVE_BLOCKS.iter().copied().find(|name| {
        document
            .blocks
            .iter()
            .any(|block| block.name.as_str() == *name)
    })
}

/// One configuration layer's ACL text with its trust classification.
#[derive(Clone, Debug)]
pub struct LayerAcl {
    /// Whether the host consented to this layer (explicit `--config` /
    /// `A3S_CONFIG`, or `~/.a3s/config.acl`) — only trusted layers may
    /// set [`HOST_AUTHORITATIVE_BLOCKS`].
    pub trusted: bool,
    /// The layer's ACL document text.
    pub acl: String,
}

/// Overwrite the host-authoritative fields of `config` (the sandbox
/// posture, providers catalog, verification declarations and continuation policy) from a trusted-only parse of
/// `layers`, so no later consumer can read a value an untrusted layer
/// contributed — whatever attribute spelling carried the attempt.
/// Returns the host-authoritative block names found in untrusted layers,
/// for the caller's log (empty = nothing was withheld). Fail-closed: an
/// empty or unparseable trusted side leaves the deny posture and an
/// empty catalog rather than guessing.
pub fn withhold_untrusted_host_authoritative_blocks(
    config: &mut CodeConfig,
    layers: &[LayerAcl],
) -> Vec<&'static str> {
    let mut withheld: Vec<&'static str> = layers
        .iter()
        .filter(|layer| !layer.trusted)
        .filter_map(|layer| host_authoritative_block(&layer.acl))
        .collect();
    if withheld.is_empty() {
        return withheld;
    }
    let trusted_layers = layers
        .iter()
        .filter(|layer| layer.trusted)
        .map(|layer| layer.acl.as_str())
        .collect::<Vec<_>>();
    // A malformed trusted side cannot acquire authority from another layer.
    let trusted = CodeConfig::from_acl_layers(&trusted_layers).unwrap_or_default();
    config.sandbox_network = trusted.sandbox_network;
    config.sandbox_network_allow = trusted.sandbox_network_allow;
    config.providers = trusted.providers;
    config.verification_contracts = trusted.verification_contracts;
    config.goal_control = trusted.goal_control;
    withheld.sort_unstable();
    withheld.dedup();
    withheld
}

// ============================================================================
// CodeConfig Implementation
// ============================================================================

impl CodeConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Load configuration from an ACL-compatible config file.
    ///
    /// `.acl` is the only supported config file extension. JSON and legacy
    /// `.hcl` config files are not supported.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = crate::bounded_io::read_utf8_file_bounded(
            path,
            crate::bounded_io::MAX_CONFIG_FILE_BYTES,
        )
        .map_err(|e| {
            CodeError::Config(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        Self::from_acl(&content).map_err(|e| {
            CodeError::Config(format!(
                "Failed to parse ACL config {}: {}",
                path.display(),
                e
            ))
        })
    }

    /// Parse configuration from an ACL string.
    ///
    /// ACL (Agent Configuration Language) uses labeled blocks like
    /// `providers "openai" { }`.
    pub fn from_acl(content: &str) -> Result<Self> {
        Self::from_acl_layers(&[content])
    }

    /// Parse ordered ACL layers without losing per-file validation boundaries.
    /// This merges values only; launchers must still withhold untrusted host fields.
    pub fn from_acl_layers(layers: &[&str]) -> Result<Self> {
        let mut config = Self::default();
        let mut doc = a3s_acl::Document::default();
        for content in layers {
            let parsed = a3s_acl::parse_acl(content)
                .map_err(|error| CodeError::Config(format!("Failed to parse ACL: {error}")))?;
            if let Some(policy) = super::goal_control::parse_layer(content, &parsed)? {
                config.goal_control = Some(policy);
            }
            doc.blocks.extend(
                parsed
                    .blocks
                    .into_iter()
                    .filter(|block| block.name != "goal_control"),
            );
        }

        for block in doc.blocks {
            match block.name.as_str() {
                "verification_contract" => {
                    let contract = super::verification_contract::parse(&block)?;
                    if config.verification_contracts.iter().any(|current| {
                        current.workspace == contract.workspace
                            && current.contract.id == contract.contract.id
                    }) {
                        return Err(CodeError::Config(
                            "duplicate verification_contract id in the same workspace".into(),
                        ));
                    }
                    config.verification_contracts.push(contract);
                }
                "default_model" => {
                    // ACL: default_model = "openai/gpt-4" or just "openai/gpt-4" as label
                    if let Some(default_model) = acl_label_or_attr(&block, &["default_model"]) {
                        config.default_model = Some(default_model);
                    }
                }
                "storage_backend" => {
                    if let Some(backend) = acl_string_attr(&block, &["storage_backend"]) {
                        config.storage_backend = match backend.to_ascii_lowercase().as_str() {
                            "file" => StorageBackend::File,
                            "memory" => StorageBackend::Memory,
                            "custom" => StorageBackend::Custom,
                            other => {
                                return Err(CodeError::Config(format!(
                                    "storage_backend `{other}` is not in the closed set file, memory, custom"
                                )));
                            }
                        };
                    }
                }
                "sandbox" => {
                    if let Some(mode) = acl_string_attr(&block, &["network", "sandbox_network"]) {
                        config.sandbox_network = crate::sandbox::SandboxNetworkMode::parse(&mode)
                            .map_err(|error| {
                            CodeError::Config(format!("sandbox block: {error}"))
                        })?;
                    }
                    if let Some(origins) =
                        acl_string_list_attr(&block, &["network_allow", "sandbox_network_allow"])
                    {
                        // Fail at parse time: a typo must not wait for the
                        // first sandboxed command to be discovered.
                        crate::sandbox::parse_network_allow_rules(&origins).map_err(|error| {
                            CodeError::Config(format!("sandbox block: {error}"))
                        })?;
                        config.sandbox_network_allow = origins;
                    }
                    if config.sandbox_network == crate::sandbox::SandboxNetworkMode::Mediated
                        && config.sandbox_network_allow.is_empty()
                    {
                        return Err(CodeError::Config(
                            "sandbox block: mediated network requires a non-empty network_allow \
                             list (mediating nothing is not a posture)"
                                .to_string(),
                        ));
                    }
                }
                "sessions_dir" => {
                    if let Some(path) = acl_string_attr(&block, &["sessions_dir"]) {
                        config.sessions_dir = Some(PathBuf::from(path));
                    }
                }
                "memory_dir" | "memoryDir" => {
                    if let Some(path) = acl_string_attr(&block, &["memory_dir", "memoryDir"]) {
                        config.memory_dir = Some(PathBuf::from(path));
                    }
                }
                "memory" => {
                    config.memory = Some(parse_memory_block(&block, config.memory.as_ref())?);
                }
                "queue" => {
                    config.queue = Some(parse_queue_block(&block)?);
                }
                "task_scheduler" | "taskScheduler" => {
                    config.task_scheduler = parse_task_scheduler_block(&block)?;
                }
                "search" => {
                    config.search = Some(parse_search_block(&block)?);
                }
                "document_parser" | "documentParser" => {
                    config.document_parser = Some(parse_document_parser_block(&block)?);
                }
                "mcp_servers" | "mcpServers" | "mcp_server" => {
                    config.mcp_servers.push(parse_mcp_server_block(&block)?);
                }
                "storage_url" => {
                    if let Some(storage_url) = acl_string_attr(&block, &["storage_url"]) {
                        config.storage_url = Some(storage_url);
                    }
                }
                "notifications" => {
                    let mut run_completion: Option<super::RunCompletionSignal> = None;
                    let mut min_seconds: Option<u64> = None;
                    for (name, value) in &block.attributes {
                        match name.as_str() {
                            "run_completion" => match value {
                                a3s_acl::Value::String(text) => {
                                    run_completion =
                                        Some(super::RunCompletionSignal::parse(text).map_err(
                                            |error| {
                                                CodeError::Config(format!(
                                                    "notifications block: {error}"
                                                ))
                                            },
                                        )?);
                                }
                                _ => {
                                    return Err(CodeError::Config(
                                        "notifications run_completion must be a string".to_string(),
                                    ))
                                }
                            },
                            "min_seconds" => match value {
                                a3s_acl::Value::Number(number)
                                    if number.fract() == 0.0
                                        && *number >= 1.0
                                        && *number <= 86_400.0 =>
                                {
                                    min_seconds = Some(*number as u64);
                                }
                                _ => {
                                    return Err(CodeError::Config(
                                        "notifications min_seconds must be an integer \
                                         between 1 and 86400"
                                            .to_string(),
                                    ))
                                }
                            },
                            other => {
                                return Err(CodeError::Config(format!(
                                    "notifications attribute `{other}` is not in the \
                                     closed set run_completion, min_seconds"
                                )))
                            }
                        }
                    }
                    let run_completion = run_completion.ok_or_else(|| {
                        CodeError::Config(
                            "notifications requires run_completion = \"bell\" | \"off\""
                                .to_string(),
                        )
                    })?;
                    config.notifications = Some(super::NotificationsConfig {
                        run_completion,
                        // The long-run floor; absent means one minute.
                        min_seconds: min_seconds.unwrap_or(60),
                    });
                }
                "workspace_retrieval" => {
                    // Accepted shape: `enabled` (required bool) plus an
                    // optional empty `local_cpu {}` marker. The feature is
                    // host-injected today (`WorkspaceRetrievalOptions` needs
                    // an embedding provider no ACL can construct), so the
                    // block parses and is exposed for hosts — sessions do
                    // NOT silently enable semantic retrieval, and an
                    // enabled block says so loudly.
                    let mut enabled: Option<bool> = None;
                    let mut local_cpu = false;
                    for (name, value) in &block.attributes {
                        match name.as_str() {
                            "enabled" => match value {
                                a3s_acl::Value::Bool(on) => enabled = Some(*on),
                                _ => {
                                    return Err(CodeError::Config(
                                        "workspace_retrieval enabled must be a bool".to_string(),
                                    ))
                                }
                            },
                            other => {
                                return Err(CodeError::Config(format!(
                                    "workspace_retrieval attribute `{other}` is not in the \
                                     closed set enabled"
                                )))
                            }
                        }
                    }
                    if !block.blocks.is_empty() {
                        for child in &block.blocks {
                            match child.name.as_str() {
                                "local_cpu" if child.attributes.is_empty() => local_cpu = true,
                                other => {
                                    return Err(CodeError::Config(format!(
                                        "workspace_retrieval block `{other}` is not in the \
                                         closed set local_cpu"
                                    )))
                                }
                            }
                        }
                    }
                    let enabled = enabled.ok_or_else(|| {
                        CodeError::Config(
                            "workspace_retrieval requires enabled = <bool>".to_string(),
                        )
                    })?;
                    if enabled {
                        tracing::warn!(
                            "workspace_retrieval parsed; semantic retrieval needs a \
                             host-injected embedding provider and stays off for sessions"
                        );
                    }
                    config.workspace_retrieval =
                        Some(super::WorkspaceRetrievalConfig { enabled, local_cpu });
                }
                "skill_dirs" | "skills" => {
                    if let Some(paths) = acl_path_list_attr(&block, &["skill_dirs", "skills"]) {
                        // Layered documents append: a workspace layer's
                        // skill_dirs extends the user layer's set instead of
                        // silently discarding it (dedup preserves first
                        // occurrence order).
                        for path in paths {
                            if !config.skill_dirs.contains(&path) {
                                config.skill_dirs.push(path);
                            }
                        }
                    }
                }
                "agent_dirs" => {
                    if let Some(paths) = acl_path_list_attr(&block, &["agent_dirs"]) {
                        // Same append semantics as skill_dirs for layers.
                        for path in paths {
                            if !config.agent_dirs.contains(&path) {
                                config.agent_dirs.push(path);
                            }
                        }
                    }
                }
                "user_instructions_dir"
                | "userInstructionsDir"
                | "global_instructions_dir"
                | "globalInstructionsDir" => {
                    if let Some(path) = acl_string_attr(
                        &block,
                        &[
                            "user_instructions_dir",
                            "userInstructionsDir",
                            "global_instructions_dir",
                            "globalInstructionsDir",
                        ],
                    ) {
                        config.user_instructions_dir = Some(PathBuf::from(path));
                    }
                }
                "project_doc_max_bytes" | "projectDocMaxBytes" => {
                    if let Some(max_bytes) =
                        acl_usize_attr(&block, &["project_doc_max_bytes", "projectDocMaxBytes"])
                    {
                        config.project_doc_max_bytes = Some(max_bytes);
                    }
                }
                "project_doc_fallback_filenames" | "projectDocFallbackFilenames" => {
                    if let Some(filenames) = acl_string_list_attr(
                        &block,
                        &[
                            "project_doc_fallback_filenames",
                            "projectDocFallbackFilenames",
                        ],
                    ) {
                        config.project_doc_fallback_filenames = filenames;
                    }
                }
                "max_tool_rounds" => {
                    if let Some(max_tool_rounds) = acl_usize_attr(&block, &["max_tool_rounds"]) {
                        config.max_tool_rounds = Some(max_tool_rounds);
                    }
                }
                "max_parallel_tasks" => {
                    if let Some(max_parallel_tasks) =
                        acl_usize_attr(&block, &["max_parallel_tasks"])
                    {
                        config.max_parallel_tasks = Some(max_parallel_tasks);
                    }
                }
                "auto_parallel" | "auto_parallel_enabled" => {
                    if let Some(auto_parallel) =
                        acl_bool_attr(&block, &["auto_parallel", "auto_parallel_enabled"])
                    {
                        config.auto_parallel = Some(auto_parallel);
                    }
                }
                "auto_delegation" => {
                    config.auto_delegation =
                        parse_auto_delegation_block(&block, &config.auto_delegation);
                }
                "thinking_budget" => {
                    if let Some(thinking_budget) = acl_usize_attr(&block, &["thinking_budget"]) {
                        config.thinking_budget = Some(thinking_budget);
                    }
                }
                "llm_api_timeout_ms" | "api_timeout_ms" | "model_api_timeout_ms" => {
                    if let Some(timeout_ms) = acl_usize_attr(
                        &block,
                        &[
                            "llm_api_timeout_ms",
                            "api_timeout_ms",
                            "model_api_timeout_ms",
                        ],
                    ) {
                        config.llm_api_timeout_ms = Some(timeout_ms as u64);
                    }
                }
                "os" => {
                    if let Some(address) =
                        acl_label_or_attr(&block, &["os", "address", "url", "baseUrl", "base_url"])
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty())
                    {
                        config.os = Some(OsConfig { address });
                    }
                }
                "providers" => {
                    let provider_name = block.labels.first().cloned().ok_or_else(|| {
                        CodeError::Config(
                            "providers block requires a label (e.g., providers \"openai\" { ... })"
                                .into(),
                        )
                    })?;

                    let mut provider = ProviderConfig {
                        name: provider_name.clone(),
                        api_key: None,
                        base_url: None,
                        headers: HashMap::new(),
                        session_id_header: None,
                        models: Vec::new(),
                    };

                    for (key, value) in &block.attributes {
                        match key.as_str() {
                            "apiKey" | "api_key" => {
                                if let Some(api_key) = acl_string(value) {
                                    provider.api_key = Some(api_key);
                                }
                            }
                            "baseUrl" | "base_url" => {
                                if let Some(base_url) = acl_string(value) {
                                    provider.base_url = Some(base_url);
                                }
                            }
                            "sessionIdHeader" | "session_id_header" => {
                                if let Some(header) = acl_string(value) {
                                    provider.session_id_header = Some(header);
                                }
                            }
                            "headers" => {
                                provider.headers = acl_string_map(value);
                            }
                            other => {
                                return Err(CodeError::Config(format!(
                                    "providers attribute `{other}` is not in the closed set api_key, base_url, session_id_header, headers"
                                )));
                            }
                        }
                    }

                    // Process nested models blocks
                    for model_block in &block.blocks {
                        if model_block.name != "models" {
                            return Err(CodeError::Config(format!(
                                "providers block `{}` is not in the closed set models",
                                model_block.name
                            )));
                        }
                        {
                            let model_name =
                                model_block.labels.first().cloned().ok_or_else(|| {
                                    CodeError::Config(
                                        "models block requires a label (e.g., models \"gpt-4\" { ... })"
                                            .into(),
                                    )
                                })?;

                            let mut model = ModelConfig {
                                id: model_name.clone(),
                                name: model_name.clone(),
                                family: String::new(),
                                api_key: None,
                                base_url: None,
                                headers: HashMap::new(),
                                session_id_header: None,
                                attachment: false,
                                reasoning: false,
                                tool_call: true,
                                temperature: true,
                                release_date: None,
                                modalities: ModelModalities::default(),
                                cost: ModelCost::default(),
                                limit: ModelLimit::default(),
                            };

                            for (key, value) in &model_block.attributes {
                                match key.as_str() {
                                    "name" => {
                                        if let Some(s) = acl_string(value) {
                                            model.name = s;
                                        }
                                    }
                                    "family" => {
                                        if let Some(s) = acl_string(value) {
                                            model.family = s;
                                        }
                                    }
                                    "apiKey" | "api_key" => {
                                        if let Some(api_key) = acl_string(value) {
                                            model.api_key = Some(api_key);
                                        }
                                    }
                                    "baseUrl" | "base_url" => {
                                        if let Some(base_url) = acl_string(value) {
                                            model.base_url = Some(base_url);
                                        }
                                    }
                                    "sessionIdHeader" | "session_id_header" => {
                                        if let Some(header) = acl_string(value) {
                                            model.session_id_header = Some(header);
                                        }
                                    }
                                    "headers" => {
                                        model.headers = acl_string_map(value);
                                    }
                                    "attachment" => {
                                        model.attachment =
                                            acl_bool_attr(model_block, &["attachment"])
                                                .unwrap_or(model.attachment);
                                    }
                                    "reasoning" => {
                                        model.reasoning =
                                            acl_bool_attr(model_block, &["reasoning"])
                                                .unwrap_or(model.reasoning);
                                    }
                                    "toolCall" | "tool_call" => {
                                        model.tool_call =
                                            acl_bool_attr(model_block, &["toolCall", "tool_call"])
                                                .unwrap_or(model.tool_call);
                                    }
                                    "temperature" => {
                                        model.temperature =
                                            acl_bool_attr(model_block, &["temperature"])
                                                .unwrap_or(model.temperature);
                                    }
                                    "releaseDate" | "release_date" => {
                                        if let Some(release_date) = acl_string(value) {
                                            model.release_date = Some(release_date);
                                        }
                                    }
                                    "maxTokens" => {
                                        tracing::warn!(
                                            provider = %provider.name,
                                            model = %model.id,
                                            field = "maxTokens",
                                            "Flat ACL model token limit fields are deprecated; use limit = {{ output = ..., context = ... }}"
                                        );
                                        if let Some(output) = acl_u32(value) {
                                            model.limit.output = output;
                                        }
                                    }
                                    "contextTokens" => {
                                        tracing::warn!(
                                            provider = %provider.name,
                                            model = %model.id,
                                            field = "contextTokens",
                                            "Flat ACL model token limit fields are deprecated; use limit = {{ output = ..., context = ... }}"
                                        );
                                        if let Some(context) = acl_u32(value) {
                                            model.limit.context = context;
                                        }
                                    }
                                    "limit" => {
                                        if let Some(output) =
                                            acl_object_u32_attr(value, &["output"])
                                        {
                                            model.limit.output = output;
                                        }
                                        if let Some(context) =
                                            acl_object_u32_attr(value, &["context"])
                                        {
                                            model.limit.context = context;
                                        }
                                    }
                                    "modalities" => {
                                        if let Some(input) = acl_object_attr(value, &["input"]) {
                                            model.modalities.input = acl_string_list(input);
                                        }
                                        if let Some(output) = acl_object_attr(value, &["output"]) {
                                            model.modalities.output = acl_string_list(output);
                                        }
                                    }
                                    "cost" => {
                                        if let Some(input) = acl_object_f64_attr(value, &["input"])
                                        {
                                            model.cost.input = input;
                                        }
                                        if let Some(output) =
                                            acl_object_f64_attr(value, &["output"])
                                        {
                                            model.cost.output = output;
                                        }
                                        if let Some(cache_read) =
                                            acl_object_f64_attr(value, &["cache_read", "cacheRead"])
                                        {
                                            model.cost.cache_read = cache_read;
                                        }
                                        if let Some(cache_write) = acl_object_f64_attr(
                                            value,
                                            &["cache_write", "cacheWrite"],
                                        ) {
                                            model.cost.cache_write = cache_write;
                                        }
                                    }
                                    other => {
                                        return Err(CodeError::Config(format!(
                                            "models attribute `{other}` is not in the closed set name, family, api_key, base_url, session_id_header, headers, attachment, reasoning, tool_call, temperature, release_date, maxTokens, contextTokens, limit, modalities, cost"
                                        )));
                                    }
                                }
                            }

                            provider.models.push(model);
                        }
                    }

                    config.providers.push(provider);
                }
                other => {
                    return Err(CodeError::Config(format!(
                        "ACL block `{other}` is not in the closed set {}",
                        KNOWN_CONFIG_BLOCKS.join(", ")
                    )));
                }
            }
        }

        if let Some(auto_parallel) = config.auto_parallel {
            config.auto_delegation.auto_parallel = auto_parallel;
        }

        Ok(config)
    }

    /// Find a provider by name
    pub fn find_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Get the default provider configuration (parsed from `default_model` "provider/model" format)
    pub fn default_provider_config(&self) -> Option<&ProviderConfig> {
        let default = self.default_model.as_ref()?;
        let (provider_name, _) = default.split_once('/')?;
        self.find_provider(provider_name)
    }

    /// Get the default model configuration (parsed from `default_model` "provider/model" format)
    pub fn default_model_config(&self) -> Option<(&ProviderConfig, &ModelConfig)> {
        let default = self.default_model.as_ref()?;
        let (provider_name, model_id) = default.split_once('/')?;
        let provider = self.find_provider(provider_name)?;
        let model = provider.find_model(model_id)?;
        Some((provider, model))
    }

    /// Get LlmConfig for the default provider and model
    ///
    /// Returns None if default provider/model is not configured or API key is missing.
    pub fn default_llm_config(&self) -> Option<LlmConfig> {
        let (provider, model) = self.default_model_config()?;
        let api_key = provider.get_api_key(model)?;
        let base_url = provider.get_base_url(model);
        let headers = provider.get_headers(model);
        let session_id_header = provider.get_session_id_header(model);

        let mut config = LlmConfig::new(&provider.name, &model.id, api_key);
        if let Some(url) = base_url {
            config = config.with_base_url(url);
        }
        if !headers.is_empty() {
            config = config.with_headers(headers);
        }
        if let Some(header_name) = session_id_header {
            config = config.with_session_id_header(header_name);
        }
        if let Some(timeout_ms) = self.llm_api_timeout_ms {
            config = config.with_api_timeout(timeout_ms);
        }
        config = apply_model_caps(config, model, self.thinking_budget);
        Some(config)
    }

    /// Get LlmConfig for a specific provider and model
    ///
    /// Returns None if provider/model is not found or API key is missing.
    pub fn llm_config(&self, provider_name: &str, model_id: &str) -> Option<LlmConfig> {
        let provider = self.find_provider(provider_name)?;
        let model = provider.find_model(model_id)?;
        let api_key = provider.get_api_key(model)?;
        let base_url = provider.get_base_url(model);
        let headers = provider.get_headers(model);
        let session_id_header = provider.get_session_id_header(model);

        let mut config = LlmConfig::new(&provider.name, &model.id, api_key);
        if let Some(url) = base_url {
            config = config.with_base_url(url);
        }
        if !headers.is_empty() {
            config = config.with_headers(headers);
        }
        if let Some(header_name) = session_id_header {
            config = config.with_session_id_header(header_name);
        }
        if let Some(timeout_ms) = self.llm_api_timeout_ms {
            config = config.with_api_timeout(timeout_ms);
        }
        config = apply_model_caps(config, model, self.thinking_budget);
        Some(config)
    }

    /// Overlay duplicate same-named providers: a later declaration wins
    /// for scalars it sets, extends headers, and replaces same-id models
    /// while keeping the earlier layer's untouched entries.
    ///
    /// Layered discovery joins multiple ACL documents into one parse; without
    /// this normalization `find_provider` resolves to the FIRST declaration
    /// while `default_model` is last-wins — a workspace layer could pick the
    /// model but silently run on the user layer's credentials.
    pub fn overlay_duplicate_providers(&mut self) {
        let mut merged: Vec<ProviderConfig> = Vec::new();
        for mut provider in std::mem::take(&mut self.providers) {
            if let Some(existing) = merged
                .iter_mut()
                .find(|candidate| candidate.name == provider.name)
            {
                if provider.api_key.is_some() {
                    existing.api_key = provider.api_key;
                }
                if provider.base_url.is_some() {
                    existing.base_url = provider.base_url;
                }
                if provider.session_id_header.is_some() {
                    existing.session_id_header = provider.session_id_header;
                }
                let headers = std::mem::take(&mut provider.headers);
                existing.headers.extend(headers);
                for model in provider.models {
                    if let Some(index) = existing
                        .models
                        .iter()
                        .position(|candidate| candidate.id == model.id)
                    {
                        existing.models[index] = model;
                    } else {
                        existing.models.push(model);
                    }
                }
            } else {
                merged.push(provider);
            }
        }
        self.providers = merged;
    }

    /// List all available models across all providers
    pub fn list_models(&self) -> Vec<(&ProviderConfig, &ModelConfig)> {
        self.providers
            .iter()
            .flat_map(|p| p.models.iter().map(move |m| (p, m)))
            .collect()
    }

    /// Add a skill directory
    pub fn add_skill_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.skill_dirs.push(dir.into());
        self
    }

    /// Add an agent directory
    pub fn add_agent_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.agent_dirs.push(dir.into());
        self
    }

    /// Check if any directories are configured
    pub fn has_directories(&self) -> bool {
        !self.skill_dirs.is_empty() || !self.agent_dirs.is_empty()
    }

    /// Check if provider configuration is available
    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }
}
