//! Non-bypassable Meta Harness kernel policy and Tardigrade-style assemble.
//!
//! Hosts mount ordered Moore components on one fact log (`components: [...]`).
//! Entries are stock parts (`system` / `tools` / `budget` / `compact` / `infer`)
//! or host registry mounts (`host:<id>`). Permission projection and the
//! completion gate stay Core-owned: a composed graph cannot disable them.
//!
//! Full Rust arbitrary trees use [`HostHarnessAssembler`] or
//! [`admit_component_tree`]. SDKs pass the declarative recipe; host Moore
//! factories stay Rust-side (no second imperative loop, no Effect-TS embed).

use std::sync::Arc;

use a3s_effect::{
    budget, coding_scheduler, compact, compose_coding_actor, component, system, tools,
    CodingServices, ErasedComponent, HarnessConfig, HarnessGraph, HarnessView, MetaHarnessSpec,
    ToolSpec,
};

pub use a3s_effect::HarnessPartId;

/// One entry in a Tardigrade-style `components: [...]` assemble list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessComponentRef {
    Stock(HarnessPartId),
    /// Registry id without the `host:` prefix.
    Host(String),
}

/// Host-facing compose recipe for SessionOptions / SDKs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessComposeOptions {
    /// Tool-call budget for the stock scheduler (`budget` part).
    pub tool_budget: Option<u32>,
    /// Compaction character threshold (`compact` part).
    pub compact_after_chars: Option<usize>,
    /// Extra system prompts merged into the `system` part.
    pub system: Vec<String>,
    /// Ordered stock parts. Used when [`Self::components`] is empty.
    pub parts: Vec<HarnessPartId>,
    /// Tardigrade-style ordered assemble list. When non-empty, takes precedence
    /// over [`Self::parts`]. Entries are stock names or `host:<id>`.
    pub components: Vec<String>,
}

impl HarnessComposeOptions {
    pub fn to_spec(&self, tools: Vec<ToolSpec>, defaults: &HarnessConfig) -> MetaHarnessSpec {
        MetaHarnessSpec {
            name: "a3s-code",
            budget: self.tool_budget.unwrap_or_else(|| defaults.budget()),
            compact_after_chars: self
                .compact_after_chars
                .unwrap_or_else(|| defaults.compact_after_chars()),
            step_limit: defaults.step_limit(),
            model_attempts: defaults.model_attempts(),
            system: if self.system.is_empty() {
                defaults.system().to_vec()
            } else {
                self.system.clone()
            },
            tools,
            tool_round_cap: defaults.tool_round_cap(),
            parts: self.parts.clone(),
        }
    }

    /// Resolve the ordered component list (stock + host mounts).
    pub fn resolved_components(&self) -> anyhow::Result<Vec<HarnessComponentRef>> {
        if !self.components.is_empty() {
            return parse_harness_components(&self.components);
        }
        if !self.parts.is_empty() {
            return Ok(self
                .parts
                .iter()
                .copied()
                .map(HarnessComponentRef::Stock)
                .collect());
        }
        Ok(default_stock_components())
    }

    /// Build from a Tardigrade-style `components: [...]` list.
    ///
    /// Stock names: `system`, `tools`, `budget`, `compact`, `infer`.
    /// Host mounts: `host:<id>` (resolved at admit time via
    /// [`HostHarnessRegistry`]).
    pub fn compose(
        components: Vec<String>,
        tool_budget: Option<u32>,
        compact_after_chars: Option<usize>,
        system: Vec<String>,
    ) -> anyhow::Result<Self> {
        let refs = parse_harness_components(&components)?;
        let (stock_parts, host_ids) = partition_components(&refs);
        // Host mounts force the components-list admit path; stock-only keeps
        // MetaHarnessSpec / cause-key stability via `parts`.
        let parts = if host_ids.is_empty() {
            stock_parts
        } else {
            Vec::new()
        };
        Ok(Self {
            tool_budget,
            compact_after_chars,
            system,
            parts,
            components,
        })
    }
}

/// Default Tardigrade stock order when neither `components` nor `parts` is set.
fn default_stock_components() -> Vec<HarnessComponentRef> {
    vec![
        HarnessComponentRef::Stock(HarnessPartId::System),
        HarnessComponentRef::Stock(HarnessPartId::Tools),
        HarnessComponentRef::Stock(HarnessPartId::Budget),
        HarnessComponentRef::Stock(HarnessPartId::Compact),
        HarnessComponentRef::Stock(HarnessPartId::Infer),
    ]
}

/// Split an assemble list into stock parts and host mount ids (order preserved
/// within each side). Used by compose admission so both arms are real.
fn partition_components(refs: &[HarnessComponentRef]) -> (Vec<HarnessPartId>, Vec<String>) {
    let mut stock = Vec::new();
    let mut hosts = Vec::new();
    for entry in refs {
        match entry {
            HarnessComponentRef::Stock(part) => stock.push(*part),
            HarnessComponentRef::Host(id) => hosts.push(id.clone()),
        }
    }
    (stock, hosts)
}

/// Parse a single stock Meta Harness part id.
pub fn parse_harness_part(name: &str) -> anyhow::Result<HarnessPartId> {
    match name.trim().to_ascii_lowercase().as_str() {
        "system" => Ok(HarnessPartId::System),
        "tools" => Ok(HarnessPartId::Tools),
        "budget" => Ok(HarnessPartId::Budget),
        "compact" => Ok(HarnessPartId::Compact),
        "infer" => Ok(HarnessPartId::Infer),
        other => anyhow::bail!(
            "unknown harness part '{other}'; expected system|tools|budget|compact|infer"
        ),
    }
}

/// Parse ordered stock part names into [`HarnessPartId`] values.
pub fn parse_harness_parts(parts: &[String]) -> anyhow::Result<Vec<HarnessPartId>> {
    parts.iter().map(|part| parse_harness_part(part)).collect()
}

/// Parse a stock name or `host:<id>` assemble entry.
pub fn parse_harness_component(name: &str) -> anyhow::Result<HarnessComponentRef> {
    let trimmed = name.trim();
    if let Some(id) = trimmed
        .strip_prefix("host:")
        .or_else(|| trimmed.strip_prefix("HOST:"))
    {
        let id = id.trim();
        if id.is_empty() {
            anyhow::bail!("host harness mount requires a non-empty id after 'host:'");
        }
        if id.contains(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')) {
            anyhow::bail!(
                "invalid host harness id '{id}': use ascii alphanumeric, '_' or '-'"
            );
        }
        return Ok(HarnessComponentRef::Host(id.to_ascii_lowercase()));
    }
    Ok(HarnessComponentRef::Stock(parse_harness_part(trimmed)?))
}

/// Parse an ordered `components: [...]` list.
pub fn parse_harness_components(components: &[String]) -> anyhow::Result<Vec<HarnessComponentRef>> {
    components
        .iter()
        .map(|entry| parse_harness_component(entry))
        .collect()
}

/// Format a host mount id as a `components: [...]` entry.
pub fn host_component_id(id: impl AsRef<str>) -> String {
    format!("host:{}", id.as_ref().trim().to_ascii_lowercase())
}

/// Kernel flags that host components cannot clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelPolicy {
    /// Permission overlay strips tool definitions before the model sees them.
    pub permission_overlay: bool,
    /// Mutating runs require verification / host waiver bound to an effect digest.
    pub completion_gate: bool,
}

impl Default for KernelPolicy {
    fn default() -> Self {
        Self {
            permission_overlay: true,
            completion_gate: true,
        }
    }
}

impl KernelPolicy {
    /// Hosts may not construct a policy that disables governance.
    pub fn admit(self) -> Self {
        Self {
            permission_overlay: true,
            completion_gate: true,
        }
    }
}

/// Host-authored Moore component factory (Rust-side; not model-grantable).
pub trait HostHarnessRegistry: Send + Sync {
    /// Build one host component for `id` (without the `host:` prefix).
    fn mount(
        &self,
        id: &str,
        config: &HarnessConfig,
    ) -> anyhow::Result<ErasedComponent<CodingServices, HarnessView>>;
}

/// Full custom graph builder for Rust embedders (arbitrary `compose_coding_actor`).
pub trait HostHarnessAssembler: Send + Sync {
    fn assemble(&self, config: HarnessConfig) -> anyhow::Result<HarnessGraph>;
}

/// Built-in registry with the `intent_stamp` host component.
///
/// Injects a stable system line so compose trees can prove host mounts without
/// embedding a second loop. Used by hermetic and Layer C Meta Harness suites.
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltinHostHarnessRegistry;

/// Stable marker injected by [`BuiltinHostHarnessRegistry`] `intent_stamp`.
pub const INTENT_STAMP_MARKER: &str = "a3s.meta_harness.intent_stamp.v1";

/// Moore output for the builtin `intent_stamp` host mount.
fn intent_stamp_view() -> a3s_effect::CodingView {
    a3s_effect::CodingView {
        system: vec![INTENT_STAMP_MARKER.into()],
        ..a3s_effect::CodingView::empty()
    }
}

impl HostHarnessRegistry for BuiltinHostHarnessRegistry {
    fn mount(
        &self,
        id: &str,
        _config: &HarnessConfig,
    ) -> anyhow::Result<ErasedComponent<CodingServices, HarnessView>> {
        match id {
            "intent_stamp" => Ok(component(
                || (),
                |state, _fact| state,
                |_state| (intent_stamp_view(), Vec::new()),
            )),
            other => anyhow::bail!(
                "unknown builtin host harness component '{other}'; known: intent_stamp"
            ),
        }
    }
}

/// Build the default graph under kernel policy.
pub fn admit_default_graph(config: HarnessConfig) -> (HarnessGraph, KernelPolicy) {
    (
        HarnessGraph::coding(config),
        KernelPolicy::default().admit(),
    )
}

/// Build a graph from a Meta Harness stock-only spec under kernel policy.
pub fn admit_spec_graph(spec: MetaHarnessSpec) -> anyhow::Result<(HarnessGraph, KernelPolicy)> {
    let config = spec
        .clone()
        .into_config()
        .map_err(|error| anyhow::anyhow!(error))?;
    let graph = HarnessGraph::from_spec(spec, || coding_scheduler(config.clone()));
    Ok((graph, KernelPolicy::default().admit()))
}

/// Admit an explicit component tree under kernel policy.
pub fn admit_component_tree(
    name: &'static str,
    components: Vec<ErasedComponent<CodingServices, HarnessView>>,
) -> (HarnessGraph, KernelPolicy) {
    (
        HarnessGraph::from_actor(compose_coding_actor(name, components)),
        KernelPolicy::default().admit(),
    )
}

/// Resolve SessionOptions harness compose into an admitted graph.
pub fn admit_from_compose(
    compose: Option<&HarnessComposeOptions>,
    config: HarnessConfig,
) -> anyhow::Result<(HarnessGraph, KernelPolicy)> {
    admit_from_compose_with_registry(compose, None, config)
}

/// Resolve compose + optional host registry into an admitted graph.
pub fn admit_from_compose_with_registry(
    compose: Option<&HarnessComposeOptions>,
    registry: Option<&dyn HostHarnessRegistry>,
    config: HarnessConfig,
) -> anyhow::Result<(HarnessGraph, KernelPolicy)> {
    let Some(options) = compose else {
        return Ok(admit_default_graph(config));
    };
    let refs = options.resolved_components()?;
    let needs_host = refs
        .iter()
        .any(|entry| matches!(entry, HarnessComponentRef::Host(_)));
    if !needs_host {
        // Stock-only path keeps MetaHarnessSpec / cause-key stability.
        let tools = config.tools().to_vec();
        let mut spec = options.to_spec(tools, &config);
        if !options.components.is_empty() {
            let (stock_parts, _) = partition_components(&refs);
            spec.parts = stock_parts;
        }
        return admit_spec_graph(spec);
    }
    let registry = registry.ok_or_else(|| {
        anyhow::anyhow!(
            "harness components include host:* mounts but no HostHarnessRegistry was installed"
        )
    })?;
    admit_mixed_tree(options, refs, registry, config)
}

fn admit_mixed_tree(
    options: &HarnessComposeOptions,
    refs: Vec<HarnessComponentRef>,
    registry: &dyn HostHarnessRegistry,
    config: HarnessConfig,
) -> anyhow::Result<(HarnessGraph, KernelPolicy)> {
    let system_prompts = if options.system.is_empty() {
        config.system().to_vec()
    } else {
        options.system.clone()
    };
    let tool_specs = config.tools().to_vec();
    let budget_limit = options.tool_budget.unwrap_or_else(|| config.budget());
    let compact_after = options
        .compact_after_chars
        .unwrap_or_else(|| config.compact_after_chars());

    let mut components = Vec::with_capacity(refs.len());
    for entry in refs {
        match entry {
            HarnessComponentRef::Stock(HarnessPartId::System) => {
                components.push(system(system_prompts.clone()));
            }
            HarnessComponentRef::Stock(HarnessPartId::Tools) => {
                components.push(tools(tool_specs.clone()));
            }
            HarnessComponentRef::Stock(HarnessPartId::Budget) => {
                components.push(budget(budget_limit));
            }
            HarnessComponentRef::Stock(HarnessPartId::Compact) => {
                components.push(compact(compact_after));
            }
            HarnessComponentRef::Stock(HarnessPartId::Infer) => {
                components.push(coding_scheduler(config.clone()));
            }
            HarnessComponentRef::Host(id) => {
                components.push(registry.mount(&id, &config)?);
            }
        }
    }
    Ok(admit_component_tree("a3s-code", components))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_policy_cannot_disable_governance() {
        let cleared = KernelPolicy {
            permission_overlay: false,
            completion_gate: false,
        };
        let admitted = cleared.admit();
        assert!(admitted.permission_overlay);
        assert!(admitted.completion_gate);
    }

    #[test]
    fn default_admission_builds_stock_graph() {
        let config = HarnessConfig::new(4, 1_000, 8, 1, vec!["s".into()], vec![]).expect("config");
        let (graph, policy) = admit_default_graph(config);
        assert_eq!(graph.actor().name, "a3s-code");
        assert!(policy.permission_overlay);
        assert!(policy.completion_gate);
    }

    #[test]
    fn compose_options_override_budget() {
        let defaults = HarnessConfig::new(8, 1_000, 8, 1, vec![], vec![]).expect("config");
        let options = HarnessComposeOptions {
            tool_budget: Some(3),
            compact_after_chars: Some(50),
            system: vec!["compose".into()],
            parts: vec![HarnessPartId::System, HarnessPartId::Infer],
            components: Vec::new(),
        };
        let spec = options.to_spec(Vec::new(), &defaults);
        assert_eq!(spec.budget, 3);
        assert_eq!(spec.compact_after_chars, 50);
        assert_eq!(spec.system, vec!["compose".to_string()]);
        assert_eq!(
            spec.parts,
            vec![HarnessPartId::System, HarnessPartId::Infer]
        );
    }

    #[test]
    fn compose_parses_stock_part_names() {
        let options = HarnessComposeOptions::compose(
            vec![
                "system".into(),
                "tools".into(),
                "budget".into(),
                "compact".into(),
                "infer".into(),
            ],
            Some(2),
            Some(10),
            vec!["s".into()],
        )
        .expect("compose");
        assert_eq!(
            options.parts,
            vec![
                HarnessPartId::System,
                HarnessPartId::Tools,
                HarnessPartId::Budget,
                HarnessPartId::Compact,
                HarnessPartId::Infer,
            ]
        );
        assert!(parse_harness_part("parallel_task").is_err());
    }

    #[test]
    fn components_list_accepts_host_mount_and_reorder() {
        let options = HarnessComposeOptions::compose(
            vec![
                "system".into(),
                host_component_id("intent_stamp"),
                "tools".into(),
                "budget".into(),
                "infer".into(),
            ],
            Some(2),
            None,
            vec!["base".into()],
        )
        .expect("compose");
        assert!(options.parts.is_empty(), "host mounts force components path");
        let refs = options.resolved_components().expect("refs");
        assert_eq!(
            refs,
            vec![
                HarnessComponentRef::Stock(HarnessPartId::System),
                HarnessComponentRef::Host("intent_stamp".into()),
                HarnessComponentRef::Stock(HarnessPartId::Tools),
                HarnessComponentRef::Stock(HarnessPartId::Budget),
                HarnessComponentRef::Stock(HarnessPartId::Infer),
            ]
        );
    }

    #[test]
    fn host_mount_without_registry_fails_closed() {
        let config = HarnessConfig::new(2, 100, 8, 1, vec![], vec![]).expect("config");
        let options = HarnessComposeOptions::compose(
            vec![
                "system".into(),
                host_component_id("intent_stamp"),
                "infer".into(),
            ],
            None,
            None,
            vec![],
        )
        .expect("compose");
        let err = match admit_from_compose_with_registry(Some(&options), None, config) {
            Ok(_) => panic!("missing registry must fail"),
            Err(error) => error,
        };
        assert!(err.to_string().contains("HostHarnessRegistry"));
    }

    #[test]
    fn builtin_registry_admits_mixed_tree() {
        let config = HarnessConfig::new(2, 100, 8, 1, vec!["sys".into()], vec![]).expect("config");
        let options = HarnessComposeOptions::compose(
            vec![
                "system".into(),
                "tools".into(),
                host_component_id("intent_stamp"),
                "budget".into(),
                "infer".into(),
            ],
            Some(2),
            None,
            vec!["sys".into()],
        )
        .expect("compose");
        let (graph, policy) = admit_from_compose_with_registry(
            Some(&options),
            Some(&BuiltinHostHarnessRegistry),
            config,
        )
        .expect("admit");
        assert_eq!(graph.actor().name, "a3s-code");
        assert!(policy.permission_overlay && policy.completion_gate);
    }

    #[test]
    fn unknown_host_id_fails_closed() {
        let config = HarnessConfig::new(2, 100, 8, 1, vec![], vec![]).expect("config");
        let options = HarnessComposeOptions::compose(
            vec!["system".into(), host_component_id("nope"), "infer".into()],
            None,
            None,
            vec![],
        )
        .expect("compose");
        let err = match admit_from_compose_with_registry(
            Some(&options),
            Some(&BuiltinHostHarnessRegistry),
            config,
        ) {
            Ok(_) => panic!("unknown host must fail"),
            Err(error) => error,
        };
        assert!(err.to_string().contains("unknown builtin"));
    }

    #[test]
    fn reject_empty_and_invalid_host_ids() {
        assert!(parse_harness_component("host:").is_err());
        assert!(parse_harness_component("host:bad.id").is_err());
        assert!(parse_harness_component("parallel_task").is_err());
    }

    #[test]
    fn admit_component_tree_preserves_kernel() {
        let (_graph, policy) = admit_component_tree(
            "custom",
            vec![system(vec!["x".into()]), budget(1), compact(8)],
        );
        assert!(policy.permission_overlay);
        assert!(policy.completion_gate);
    }

    #[test]
    fn stock_only_components_list_uses_spec_path() {
        let config = HarnessConfig::new(3, 50, 8, 1, vec![], vec![]).expect("config");
        let options = HarnessComposeOptions::compose(
            vec![
                "system".into(),
                "tools".into(),
                "budget".into(),
                "infer".into(),
            ],
            Some(3),
            None,
            vec![],
        )
        .expect("compose");
        // No compact part — intentional subset assemble.
        assert!(!options
            .resolved_components()
            .unwrap()
            .iter()
            .any(|entry| matches!(
                entry,
                HarnessComponentRef::Stock(HarnessPartId::Compact)
            )));
        let (graph, _) =
            admit_from_compose_with_registry(Some(&options), None, config).expect("admit");
        assert_eq!(graph.actor().name, "a3s-code");
    }

    #[test]
    fn resolved_components_falls_back_to_parts_then_default_stock() {
        let from_parts = HarnessComposeOptions {
            parts: vec![HarnessPartId::System, HarnessPartId::Infer],
            ..HarnessComposeOptions::default()
        };
        assert_eq!(
            from_parts.resolved_components().expect("parts"),
            vec![
                HarnessComponentRef::Stock(HarnessPartId::System),
                HarnessComponentRef::Stock(HarnessPartId::Infer),
            ]
        );

        let defaults = HarnessComposeOptions::default()
            .resolved_components()
            .expect("default stock");
        assert_eq!(defaults, default_stock_components());
    }

    #[test]
    fn parse_harness_parts_accepts_stock_names_and_rejects_unknown() {
        let parts = parse_harness_parts(&[
            "System".into(),
            "TOOLS".into(),
            "budget".into(),
            "compact".into(),
            "infer".into(),
        ])
        .expect("stock names");
        assert_eq!(
            parts,
            vec![
                HarnessPartId::System,
                HarnessPartId::Tools,
                HarnessPartId::Budget,
                HarnessPartId::Compact,
                HarnessPartId::Infer,
            ]
        );
        assert!(parse_harness_parts(&["nope".into()]).is_err());
    }

    #[test]
    fn admit_from_compose_wrapper_defaults_and_admits_stock() {
        let config = HarnessConfig::new(2, 100, 8, 1, vec![], vec![]).expect("config");
        let (default_graph, policy) = admit_from_compose(None, config.clone()).expect("default");
        assert_eq!(default_graph.actor().name, "a3s-code");
        assert!(policy.permission_overlay && policy.completion_gate);

        let options = HarnessComposeOptions {
            parts: vec![HarnessPartId::System, HarnessPartId::Budget, HarnessPartId::Infer],
            tool_budget: Some(2),
            ..HarnessComposeOptions::default()
        };
        let (graph, _) = admit_from_compose(Some(&options), config).expect("stock");
        assert_eq!(graph.actor().name, "a3s-code");
    }

    #[test]
    fn mixed_tree_includes_compact_and_empty_system_fallback() {
        let config =
            HarnessConfig::new(2, 40, 8, 1, vec!["cfg-system".into()], vec![]).expect("config");
        let options = HarnessComposeOptions::compose(
            vec![
                "system".into(),
                "tools".into(),
                "budget".into(),
                "compact".into(),
                host_component_id("intent_stamp"),
                "infer".into(),
            ],
            Some(2),
            Some(40),
            vec![], // fall back to config.system()
        )
        .expect("compose");
        let (graph, policy) = admit_from_compose_with_registry(
            Some(&options),
            Some(&BuiltinHostHarnessRegistry),
            config,
        )
        .expect("admit mixed with compact");
        assert_eq!(graph.actor().name, "a3s-code");
        assert!(policy.permission_overlay && policy.completion_gate);
    }

    #[test]
    fn partition_and_intent_stamp_view_are_behavior_oracles() {
        let refs = vec![
            HarnessComponentRef::Stock(HarnessPartId::System),
            HarnessComponentRef::Host("intent_stamp".into()),
            HarnessComponentRef::Stock(HarnessPartId::Infer),
        ];
        let (stock, hosts) = partition_components(&refs);
        assert_eq!(stock, vec![HarnessPartId::System, HarnessPartId::Infer]);
        assert_eq!(hosts, vec!["intent_stamp".to_string()]);

        let view = intent_stamp_view();
        assert_eq!(view.system, vec![INTENT_STAMP_MARKER.to_string()]);
    }

    #[test]
    fn host_prefix_is_case_insensitive_and_normalizes_id() {
        let upper = parse_harness_component("HOST:Intent_Stamp").expect("HOST:");
        assert_eq!(
            upper,
            HarnessComponentRef::Host("intent_stamp".into())
        );
        assert_eq!(
            host_component_id(" Intent_Stamp "),
            "host:intent_stamp".to_string()
        );
    }

    #[test]
    fn to_spec_keeps_default_system_when_compose_system_empty() {
        let defaults =
            HarnessConfig::new(4, 10, 8, 1, vec!["default-sys".into()], vec![]).expect("config");
        let options = HarnessComposeOptions {
            tool_budget: None,
            compact_after_chars: None,
            system: vec![],
            parts: vec![HarnessPartId::System],
            components: Vec::new(),
        };
        let spec = options.to_spec(Vec::new(), &defaults);
        assert_eq!(spec.budget, 4);
        assert_eq!(spec.compact_after_chars, 10);
        assert_eq!(spec.system, vec!["default-sys".to_string()]);
    }
}
