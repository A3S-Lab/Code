use serde::{Deserialize, Serialize};

/// Access gate from `grok_build_access_gate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateInfo {
    pub message: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

/// Typed auth metadata passed from the shell to the pager via ACP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMeta {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// Team id from the token: the team principal's id, or a personal account's billing team.
    #[serde(default)]
    pub team_id: Option<String>,
    /// `GrokAuth::is_team_principal`: the credential carries team context, so `/user` can resolve `can_administer_team`.
    #[serde(default)]
    pub is_team_principal: bool,
    #[serde(default)]
    pub team_name: Option<String>,
    #[serde(default)]
    pub is_zdr: bool,
    #[serde(default)]
    pub team_role: Option<String>,
    /// Defaults to opted-out (safer) until auth meta is populated.
    #[serde(default = "crate::default_coding_data_retention_opt_out")]
    pub coding_data_retention_opt_out: bool,
    #[serde(default)]
    pub can_administer_team: Option<bool>,
    #[serde(default)]
    pub show_resolved_model: Option<bool>,
    /// `Some` means the user is blocked; `None` means the user has access.
    #[serde(default)]
    pub gate: Option<GateInfo>,
    /// Display name for the current subscription tier (e.g. "A3S Code Heavy", "X Premium", "Free"), from CCP `/settings`.
    #[serde(default)]
    pub subscription_tier: Option<String>,
    /// Whether `/feedback` may offer a one-shot trace upload; it lives on auth meta so it refreshes with auth changes.
    #[serde(default)]
    pub feedback_trace_offer: bool,
    /// Whether the agent's backend handles billing itself.
    #[serde(default)]
    pub backend_billed: bool,
}

impl Default for AuthMeta {
    fn default() -> Self {
        Self {
            email: None,
            auth_mode: None,
            team_id: None,
            is_team_principal: false,
            team_name: None,
            is_zdr: false,
            team_role: None,
            coding_data_retention_opt_out: crate::default_coding_data_retention_opt_out(),
            can_administer_team: None,
            show_resolved_model: None,
            gate: None,
            subscription_tier: None,
            feedback_trace_offer: false,
            backend_billed: false,
        }
    }
}
