// `McpOAuthConfig` / `McpOAuthConfigMap` are re-exported via `mcp` (see `mcp.rs`)

mod announcements;
mod campaigns;
mod consent;
mod hints;
mod load;
mod mcp;
mod mcp_reenable;
mod permissions;
mod persist;
mod resolve;
mod settings_writes;
mod tips;
mod worktree;

pub use announcements::*;
pub use campaigns::{
    CampaignModelsDefault, EffectiveConfigLayers, campaign_driven_models_default,
    load_effective_config, load_effective_config_disk_only, load_effective_config_with_layers,
    persist_models_default, remote_campaigns_from_settings, set_remote_campaigns_from_settings,
    sync_campaign_fields,
};
pub use consent::*;
pub use hints::*;
pub use load::*;
pub use mcp::*;
pub(crate) use mcp_reenable::{McpDefinitionIndex, needs_definition_scan};
pub use permissions::*;
pub use persist::*;
// These types live in `a3s-code-config-types`; the re-export keeps `crate::util::config::{RemoteSettings, GoalRoleModel}` working
pub use a3s_code_config_types::{
    CampaignOverride, ConsentGate, ContextualHintsRemote, DisplayRefreshSettings,
    DoomLoopRecoverySettings, GoalRoleModel, LongReasoningReminderSettings, RemoteSettings,
    WorktreeAutoGcSettings, WorktreeKindMaxAge, deserialize_tolerant,
};
pub use resolve::*;
pub use settings_writes::*;
pub use tips::*;
pub use worktree::*;
