//! Non-bypassable Meta Harness kernel policy.
//!
//! Host composition may mount arbitrary Moore components on the fact log.
//! Permission projection and completion-gate enrollment stay Core-owned: a
//! composed graph cannot disable them. LiveCompletion already strips the
//! model-facing catalog; [`crate::harness_loop`] still decides mutating
//! success. This module records the admission contract for a
//! [`a3s_effect::HarnessGraph`].

use a3s_effect::{HarnessConfig, HarnessGraph, MetaHarnessSpec, ToolSpec};

pub use a3s_effect::HarnessPartId;

/// Host-facing compose recipe for SessionOptions / SDKs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessComposeOptions {
    /// Tool-call budget for the stock scheduler (`budget` part).
    pub tool_budget: Option<u32>,
    /// Compaction character threshold (`compact` part).
    pub compact_after_chars: Option<usize>,
    /// Extra system prompts merged into the `system` part.
    pub system: Vec<String>,
    /// Ordered stock parts. Empty selects the full stock tree.
    pub parts: Vec<HarnessPartId>,
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

    /// Build from SDK stock part names (`system`, `tools`, `budget`, `compact`, `infer`).
    pub fn compose(
        parts: Vec<String>,
        tool_budget: Option<u32>,
        compact_after_chars: Option<usize>,
        system: Vec<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            tool_budget,
            compact_after_chars,
            system,
            parts: parse_harness_parts(&parts)?,
        })
    }
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

/// Build the default graph under kernel policy.
pub fn admit_default_graph(config: HarnessConfig) -> (HarnessGraph, KernelPolicy) {
    (HarnessGraph::coding(config), KernelPolicy::default().admit())
}

/// Build a graph from a Meta Harness spec under kernel policy.
pub fn admit_spec_graph(spec: MetaHarnessSpec) -> anyhow::Result<(HarnessGraph, KernelPolicy)> {
    let config = spec
        .clone()
        .into_config()
        .map_err(|error| anyhow::anyhow!(error))?;
    let graph = HarnessGraph::from_spec(spec, || a3s_effect::coding_scheduler(config.clone()));
    Ok((graph, KernelPolicy::default().admit()))
}

/// Resolve SessionOptions harness compose into an admitted graph.
pub fn admit_from_compose(
    compose: Option<&HarnessComposeOptions>,
    config: HarnessConfig,
) -> anyhow::Result<(HarnessGraph, KernelPolicy)> {
    match compose {
        None => Ok(admit_default_graph(config)),
        Some(options) => {
            let tools = config.tools().to_vec();
            let spec = options.to_spec(tools, &config);
            admit_spec_graph(spec)
        }
    }
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
        let config =
            HarnessConfig::new(4, 1_000, 8, 1, vec!["s".into()], vec![]).expect("config");
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
}
