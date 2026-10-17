use serde::{Deserialize, Serialize};

use super::{MatchingRules, PermissionChecker, PermissionDecision, PermissionRule};
use crate::queue::SessionLane;

/// Policy the next run admission freezes.
///
/// `default` asks. `plan` allows reads and denies mutations. `auto` allows
/// known reads and still asks before execute. `yolo` allows query and execute
/// lanes. Deny rules added by the caller still win, because
/// [`PermissionPolicy::check`] evaluates deny before allow.
pub fn policy_for_posture(posture: &str) -> PermissionPolicy {
    match posture.trim().to_ascii_lowercase().as_str() {
        "plan" => super::style_specialty::plan_permissions(),
        "auto" => PermissionPolicy::new().allow_all(&["read", "search", "ls", "update_plan"]),
        "yolo" => {
            PermissionPolicy::new().allow_yolo_lanes([SessionLane::Query, SessionLane::Execute])
        }
        _ => PermissionPolicy::new(),
    }
}

fn yolo_lane_rule(lane: SessionLane) -> String {
    match lane {
        SessionLane::Control => "lane:control".to_string(),
        SessionLane::Query => "lane:query".to_string(),
        SessionLane::Execute => "lane:execute".to_string(),
        SessionLane::Generate => "lane:generate".to_string(),
    }
}

/// Whether a policy's allow/ask rules are evaluated (closed set).
///
/// `unrestricted` allows every call that no deny rule matches; deny rules
/// always win. There is no setting that skips deny rules.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionEnforcement {
    #[default]
    Enforced,
    Unrestricted,
}

impl PermissionEnforcement {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "enforced" => Ok(Self::Enforced),
            "unrestricted" => Ok(Self::Unrestricted),
            other => Err(format!(
                "unknown permission enforcement `{other}`; expected one of: enforced, unrestricted"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforced => "enforced",
            Self::Unrestricted => "unrestricted",
        }
    }
    /// Resolve an explicit enforcement against the retired `enabled`
    /// switch. `enabled = false` without `unrestricted` is refused, never
    /// read as "allow everything".
    pub fn resolve(enforcement: Option<Self>, enabled: Option<bool>) -> Result<Self, String> {
        if enabled.is_some() {
            tracing::warn!(
                "permission policy `enabled` is deprecated; use `enforcement` (enforced, unrestricted)"
            );
        }
        match (enforcement, enabled) {
            (Some(Self::Enforced), Some(false)) => Err(
                "permission policy sets `enabled = false` and `enforcement = \"enforced\"`"
                    .to_string(),
            ),
            (Some(enforcement), _) => Ok(enforcement),
            (None, Some(false)) => {
                Err("permission policy `enabled = false` no longer disables \
                 checks; set `enforcement = \"unrestricted\"` (closed set: enforced, unrestricted)"
                    .to_string())
            }
            (None, _) => Ok(Self::Enforced),
        }
    }
}

/// Serialized form. `enabled` is the retired switch: `true` is accepted as
/// `enforced`; `false` without `enforcement = "unrestricted"` is refused
/// instead of silently allowing every call.
#[derive(Deserialize)]
struct PermissionPolicyWire {
    #[serde(default)]
    deny: Vec<PermissionRule>,
    #[serde(default)]
    allow: Vec<PermissionRule>,
    #[serde(default)]
    ask: Vec<PermissionRule>,
    #[serde(default = "default_decision")]
    default_decision: PermissionDecision,
    #[serde(default)]
    enforcement: Option<PermissionEnforcement>,
    #[serde(default)]
    enabled: Option<bool>,
}

impl TryFrom<PermissionPolicyWire> for PermissionPolicy {
    type Error = String;

    fn try_from(wire: PermissionPolicyWire) -> Result<Self, Self::Error> {
        let enforcement = PermissionEnforcement::resolve(wire.enforcement, wire.enabled)?;
        Ok(Self {
            deny: wire.deny,
            allow: wire.allow,
            ask: wire.ask,
            default_decision: wire.default_decision,
            enforcement,
        })
    }
}

/// Permission policy configuration
///
/// Evaluation order:
/// 1. Deny rules - any match results in denial
/// 2. Allow rules - any match results in auto-approval
/// 3. Ask rules - any match requires user confirmation
/// 4. Default - falls back to default_decision
///
/// Under [`PermissionEnforcement::Unrestricted`] steps 2-4 become Allow;
/// step 1 still applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "PermissionPolicyWire")]
pub struct PermissionPolicy {
    /// Rules that always deny (checked first)
    #[serde(default)]
    pub deny: Vec<PermissionRule>,

    /// Rules that auto-approve without confirmation
    #[serde(default)]
    pub allow: Vec<PermissionRule>,

    /// Rules that always require confirmation
    #[serde(default)]
    pub ask: Vec<PermissionRule>,

    /// Default decision when no rules match
    #[serde(default = "default_decision")]
    pub default_decision: PermissionDecision,

    /// Whether allow/ask rules are evaluated. Deny rules always are.
    pub enforcement: PermissionEnforcement,
}

fn default_decision() -> PermissionDecision {
    PermissionDecision::Ask
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            deny: Vec::new(),
            allow: Vec::new(),
            ask: Vec::new(),
            default_decision: PermissionDecision::Ask,
            enforcement: PermissionEnforcement::Enforced,
        }
    }
}

impl PermissionPolicy {
    /// Create a new permission policy
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a strict policy that asks for everything
    pub fn strict() -> Self {
        Self {
            deny: Vec::new(),
            allow: Vec::new(),
            ask: Vec::new(),
            default_decision: PermissionDecision::Ask,
            enforcement: PermissionEnforcement::Enforced,
        }
    }

    /// Add a deny rule
    pub fn deny(mut self, rule: &str) -> Self {
        self.deny.push(PermissionRule::new(rule));
        self
    }

    /// Add an allow rule
    pub fn allow(mut self, rule: &str) -> Self {
        self.allow.push(PermissionRule::new(rule));
        self
    }

    /// Record YOLO lanes as Allow. Deny rules still win. The lane of a tool
    /// comes from [`SessionLane::from_tool_name`].
    pub fn allow_yolo_lanes(mut self, lanes: impl IntoIterator<Item = SessionLane>) -> Self {
        for lane in lanes {
            self.allow.push(PermissionRule::new(&yolo_lane_rule(lane)));
        }
        self
    }

    /// Add an ask rule
    pub fn ask(mut self, rule: &str) -> Self {
        self.ask.push(PermissionRule::new(rule));
        self
    }

    /// Add multiple deny rules
    pub fn deny_all(mut self, rules: &[&str]) -> Self {
        for rule in rules {
            self.deny.push(PermissionRule::new(rule));
        }
        self
    }

    /// Add multiple allow rules
    pub fn allow_all(mut self, rules: &[&str]) -> Self {
        for rule in rules {
            self.allow.push(PermissionRule::new(rule));
        }
        self
    }

    /// Add multiple ask rules
    pub fn ask_all(mut self, rules: &[&str]) -> Self {
        for rule in rules {
            self.ask.push(PermissionRule::new(rule));
        }
        self
    }

    /// Check permission for a tool invocation
    ///
    /// Returns the permission decision based on rule evaluation order:
    /// 1. Deny rules (any match = Deny)
    /// 2. Allow rules (any match = Allow)
    /// 3. Ask rules (any match = Ask)
    /// 4. Default decision
    pub fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        // Reuse the scalar policy for every path. A matching Allow for one
        // file cannot grant access to another member of the same read batch.
        if let Some(decision) =
            super::read_batch::check(tool_name, args, |member| self.check(tool_name, member))
        {
            return decision;
        }

        // 1. Check deny rules first
        for rule in &self.deny {
            if rule.matches(tool_name, args) {
                return PermissionDecision::Deny;
            }
        }
        if self.enforcement == PermissionEnforcement::Unrestricted {
            return PermissionDecision::Allow;
        }

        // 2. Check allow rules. A YOLO lane rule is Allow for every tool in
        // that lane, including names `from_tool_name` maps by default.
        let lane_rule = yolo_lane_rule(SessionLane::from_tool_name(tool_name));
        for rule in &self.allow {
            if rule.rule == lane_rule || rule.matches(tool_name, args) {
                return PermissionDecision::Allow;
            }
        }

        // 3. Check ask rules
        for rule in &self.ask {
            if rule.matches(tool_name, args) {
                return PermissionDecision::Ask;
            }
        }

        // 4. Fall back to default
        self.default_decision
    }

    /// Check if a tool invocation is allowed (Allow or not Deny)
    pub fn is_allowed(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        matches!(self.check(tool_name, args), PermissionDecision::Allow)
    }

    /// Check if a tool invocation is denied
    pub fn is_denied(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        matches!(self.check(tool_name, args), PermissionDecision::Deny)
    }

    /// Check if a tool invocation requires confirmation
    pub fn requires_confirmation(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        matches!(self.check(tool_name, args), PermissionDecision::Ask)
    }

    /// Get matching rules for debugging/logging
    pub fn get_matching_rules(&self, tool_name: &str, args: &serde_json::Value) -> MatchingRules {
        let mut result = MatchingRules::default();

        for rule in &self.deny {
            if rule.matches(tool_name, args) {
                result.deny.push(rule.rule.clone());
            }
        }

        for rule in &self.allow {
            if rule.matches(tool_name, args) {
                result.allow.push(rule.rule.clone());
            }
        }

        for rule in &self.ask {
            if rule.matches(tool_name, args) {
                result.ask.push(rule.rule.clone());
            }
        }

        result
    }

    /// Whether this policy explicitly declares that a tool may be considered
    /// through an Allow or Ask rule.
    ///
    /// This ignores argument patterns and does not authorize execution. It is
    /// used when composing parent and delegated-worker visibility: an explicit
    /// worker capability can override a parent host's ordinary model hiding,
    /// while execution-time checks from both scopes remain authoritative.
    pub fn declares_tool_access(&self, tool_name: &str) -> bool {
        self.allow
            .iter()
            .chain(&self.ask)
            .any(|rule| rule.matches_tool(tool_name))
    }
}

impl PermissionChecker for PermissionPolicy {
    fn expose_to_model(&self, tool_name: &str) -> bool {
        // A deny that covers every argument makes the entire capability
        // unavailable. Argument-scoped denies keep the tool visible so an
        // allowed invocation can still be formed and checked at execution.
        if self
            .deny
            .iter()
            .any(|rule| rule.matches_tool(tool_name) && rule.matches_all_args())
        {
            return false;
        }

        if self.enforcement == PermissionEnforcement::Unrestricted
            || self.default_decision != PermissionDecision::Deny
        {
            return true;
        }

        // Under a deny-by-default worker policy, expose only tools for which
        // at least one Allow or Ask rule can match. Argument patterns are
        // intentionally ignored here; execution-time checks remain
        // authoritative for the actual arguments.
        self.declares_tool_access(tool_name)
    }

    fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        self.check(tool_name, args)
    }
}
