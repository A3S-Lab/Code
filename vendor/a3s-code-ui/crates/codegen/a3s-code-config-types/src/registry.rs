//! The registry of boolean `[features]` keys.
//!
//! One row per feature spells it on every surface that can set it, and [`Feature::resolve`] is the only place precedence lives.
//! A key needing different precedence keeps its own resolver, as `remote_fetch` does.

use crate::{
    RemoteSettings,
    flags::{BoolFlag, ConfigSource, Resolved},
};
use a3s_code_config::{CampaignEntry, ConfigLayers, env_bool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, strum::EnumIter)]
pub enum Feature {
    /// The SQLite session-search index.
    SessionSearch,
    /// Language-server-backed navigation tools.
    LspTools,
    /// The `web_fetch` tool.
    WebFetch,
    /// `/recap` and the automatic return-from-away recap.
    SessionRecap,
    /// The `ask_user_question` tool.
    AskUserQuestion,
    /// Voice dictation (speech to text).
    VoiceMode,
    /// The `write_file` tool.
    WriteFile,
    /// Heuristic feedback popups and the `/feedback` command.
    Feedback,
    /// The `/feedback` trace-consent card (the trace-upload opt-in offer).
    FeedbackTraceCard,
    /// The per-turn summary on the agent dashboard.
    TurnSummary,
    /// Ctrl+C before a turn's first activity restores the prompt.
    CancelRewind,
    /// Summarize the verbatim conversation rather than a shortened copy of it.
    CompactionVerbatimInput,
    /// Summarize the earlier part of a long conversation in the background, before compaction.
    TwoPassCompaction,
    /// Server-side execution of `web_search` and `x_search`.
    BackendTools,
    /// Continue the conversation as soon as a background task or subagent finishes.
    AutoWake,
    /// Save a finished subagent's working copy into the repo as a git ref, restored on resume.
    SubagentWorktreeSnapshot,
    /// Hide the subagent `model` argument when every eligible catalog entry is an A3S model.
    SubagentModelInheritance,
    /// Send model-authored follow-ups to an owned active descendant.
    ActiveAgentMessages,
    /// Consolidated panel dock above the prompt (Subagents / Tasks / Watchers / Queued).
    Dock,
    /// The terminal-native `terminal` color theme (staged rollout).
    TerminalTheme,
}

/// How one feature is written on each surface it can be set from.
#[derive(Debug, Clone, Copy)]
pub struct FeatureSpec {
    pub id: Feature,
    pub key: &'static str,
    pub path: &'static str,
    pub env: &'static str,
    pub default_enabled: bool,
    /// `None` where the key has no remote tier, so adding one is a deliberate edit.
    pub remote: Option<fn(&RemoteSettings) -> Option<bool>>,
    // No managed tier: `config` is the loader's merge, where a user's config.toml already beats managed_config.toml
}

/// What each tier had to say about one feature.
#[derive(Debug, Clone, Copy, Default)]
pub struct FeatureSources {
    pub pin: Option<bool>,
    pub env: Option<bool>,
    pub config: Option<bool>,
    /// Already projected, by [`Feature::remote_value`].
    pub remote: Option<bool>,
}

impl FeatureSources {
    /// The environment tier read from the process; every other tier unset.
    pub fn from_process_env(feature: Feature) -> Self {
        Self {
            env: env_bool(feature.env()),
            ..Self::default()
        }
    }
}

/// A layer of the config tier other than the user `config.toml`, lowest first; the user file sits between `Managed`
/// and `Campaign` in the effective merge, and the overlay is re-applied over campaign patches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureConfigLayer {
    SystemManaged,
    Managed,
    Campaign,
    Overlay,
}

impl FeatureConfigLayer {
    /// Names the layer for the user.
    pub fn label(self) -> &'static str {
        match self {
            Self::SystemManaged => "the system managed_config.toml",
            Self::Managed => "managed_config.toml",
            Self::Campaign => "an active campaign",
            Self::Overlay => "the GROK_CONFIG overlay",
        }
    }
}

/// A `[features]` key as one layer of the effective merge sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureLayerValue {
    pub layer: FeatureConfigLayer,
    pub value: bool,
}

/// The config tier of one feature split around the user `config.toml`, the one layer a settings surface can write.
/// `merged` is what `Config` latches; the split tells a writer whether its key would decide anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureConfigLayers {
    /// The requirements pin (MDM, then system, then user), read from the same layers. It is the pin tier, not part of `merged`.
    pub pin: Option<bool>,
    /// The user `config.toml` key.
    pub user: Option<bool>,
    /// Beats a user write: the `GROK_CONFIG` overlay, or a campaign patch.
    pub above_user: Option<FeatureLayerValue>,
    /// Applies only while no user key exists: `managed_config.toml`, then the system managed config.
    pub below_user: Option<FeatureLayerValue>,
}

impl FeatureConfigLayers {
    /// The config tier as `Feature::resolve` sees it.
    pub fn merged(&self) -> Option<bool> {
        self.above_user
            .map(|layer| layer.value)
            .or(self.user)
            .or(self.below_user.map(|layer| layer.value))
    }
}

pub const FEATURES: &[FeatureSpec] = &[
    FeatureSpec {
        id: Feature::SessionSearch,
        key: "session_search",
        path: "features.session_search",
        env: "GROK_SESSION_SEARCH",
        default_enabled: true,
        remote: Some(|settings| settings.session_search),
    },
    FeatureSpec {
        id: Feature::LspTools,
        key: "lsp_tools",
        path: "features.lsp_tools",
        env: "GROK_LSP_TOOLS",
        default_enabled: false,
        remote: Some(|settings| settings.lsp_tools_enabled),
    },
    FeatureSpec {
        id: Feature::WebFetch,
        key: "web_fetch",
        path: "features.web_fetch",
        env: "GROK_WEB_FETCH",
        default_enabled: false,
        remote: Some(|settings| settings.web_fetch_enabled),
    },
    FeatureSpec {
        id: Feature::SessionRecap,
        key: "session_recap",
        path: "features.session_recap",
        env: "GROK_SESSION_RECAP",
        default_enabled: true,
        remote: Some(|settings| settings.session_recap),
    },
    FeatureSpec {
        id: Feature::AskUserQuestion,
        key: "ask_user_question",
        path: "features.ask_user_question",
        env: "GROK_ASK_USER_QUESTION",
        default_enabled: true,
        remote: Some(|settings| settings.ask_user_question_enabled),
    },
    FeatureSpec {
        id: Feature::VoiceMode,
        key: "voice_mode",
        path: "features.voice_mode",
        env: "GROK_VOICE_MODE",
        default_enabled: true,
        remote: Some(|settings| settings.voice_mode_enabled),
    },
    FeatureSpec {
        id: Feature::WriteFile,
        key: "write_file",
        path: "features.write_file",
        env: "GROK_WRITE_FILE",
        default_enabled: true,
        remote: Some(|settings| settings.write_file_enabled),
    },
    FeatureSpec {
        id: Feature::Feedback,
        key: "feedback",
        path: "features.feedback",
        env: "GROK_FEEDBACK_ENABLED",
        default_enabled: true,
        remote: Some(|settings| settings.feedback_enabled),
    },
    FeatureSpec {
        id: Feature::FeedbackTraceCard,
        key: "feedback_trace_card",
        path: "features.feedback_trace_card",
        env: "GROK_FEEDBACK_TRACE_CARD",
        default_enabled: false,
        remote: Some(|settings| settings.feedback_trace_card_enabled),
    },
    FeatureSpec {
        id: Feature::TurnSummary,
        key: "turn_summary",
        path: "features.turn_summary",
        env: "GROK_TURN_SUMMARY",
        default_enabled: true,
        remote: Some(|settings| settings.turn_summary),
    },
    FeatureSpec {
        id: Feature::CancelRewind,
        key: "cancel_rewind",
        path: "features.cancel_rewind",
        env: "GROK_CANCEL_REWIND",
        default_enabled: true,
        remote: Some(|settings| settings.cancel_rewind_enabled),
    },
    FeatureSpec {
        id: Feature::CompactionVerbatimInput,
        key: "compaction_verbatim_input",
        path: "features.compaction_verbatim_input",
        env: "GROK_COMPACTION_VERBATIM_INPUT",
        default_enabled: true,
        remote: Some(|settings| settings.compaction_verbatim_input),
    },
    FeatureSpec {
        id: Feature::TwoPassCompaction,
        key: "two_pass_compaction",
        path: "features.two_pass_compaction",
        env: "GROK_TWO_PASS_COMPACTION",
        default_enabled: true,
        remote: Some(|settings| settings.two_pass_compaction_enabled),
    },
    FeatureSpec {
        id: Feature::BackendTools,
        key: "backend_tools",
        path: "features.backend_tools",
        // The variable predates the key and is not spelled after it.
        env: "GROK_BACKEND_SEARCH",
        default_enabled: true,
        remote: None,
    },
    FeatureSpec {
        id: Feature::AutoWake,
        key: "auto_wake",
        path: "features.auto_wake",
        env: "GROK_AUTO_WAKE",
        default_enabled: true,
        remote: Some(|settings| settings.auto_wake_enabled),
    },
    FeatureSpec {
        id: Feature::SubagentWorktreeSnapshot,
        key: "subagent_worktree_snapshot",
        path: "features.subagent_worktree_snapshot",
        env: "GROK_SUBAGENT_WORKTREE_SNAPSHOT",
        default_enabled: false,
        remote: Some(|settings| settings.subagent_worktree_snapshot_enabled),
    },
    FeatureSpec {
        id: Feature::SubagentModelInheritance,
        key: "subagent_model_inheritance",
        path: "features.subagent_model_inheritance",
        env: "GROK_SUBAGENT_MODEL_INHERITANCE",
        default_enabled: false,
        remote: Some(|settings| settings.subagent_model_inheritance_enabled),
    },
    FeatureSpec {
        id: Feature::ActiveAgentMessages,
        key: "active_agent_messages",
        path: "features.active_agent_messages",
        env: "GROK_ACTIVE_AGENT_MESSAGES",
        default_enabled: false,
        remote: Some(|settings| settings.active_agent_messages_enabled),
    },
    FeatureSpec {
        id: Feature::Dock,
        key: "dock",
        path: "features.dock",
        env: "GROK_DOCK",
        default_enabled: false,
        remote: Some(|settings| settings.dock_enabled),
    },
    FeatureSpec {
        id: Feature::TerminalTheme,
        key: "terminal_theme",
        path: "features.terminal_theme",
        env: "GROK_TERMINAL_THEME",
        default_enabled: false,
        remote: Some(|settings| settings.terminal_theme_enabled),
    },
];

impl Feature {
    fn spec(self) -> &'static FeatureSpec {
        FEATURES
            .iter()
            .find(|spec| spec.id == self)
            .unwrap_or_else(|| unreachable!("missing FeatureSpec for {self:?}"))
    }

    pub fn key(self) -> &'static str {
        self.spec().key
    }

    pub fn env(self) -> &'static str {
        self.spec().env
    }

    pub fn path(self) -> &'static str {
        self.spec().path
    }

    /// The one place a `RemoteSettings` is read for a feature.
    pub fn remote_value(self, settings: Option<&RemoteSettings>) -> Option<bool> {
        let read = self.spec().remote?;
        read(settings?)
    }

    pub fn default_enabled(self) -> bool {
        self.spec().default_enabled
    }

    /// Reads like `off (a requirements.toml pin)`; `None` while the feature is on.
    pub fn off_reason(self, sources: FeatureSources) -> Option<String> {
        let resolved = self.resolve(sources);
        if resolved.value {
            return None;
        }
        Some(self.source_label(resolved.source))
    }

    /// Names a tier for the user: `a requirements.toml pin or an MDM policy`, `the GROK_X environment variable`, …
    /// `source` is where this feature's own resolution came from; the label spells this feature's key and variable.
    pub fn source_label(self, source: ConfigSource) -> String {
        let spec = self.spec();
        match source {
            ConfigSource::Requirement => "a requirements.toml pin or an MDM policy".to_owned(),
            ConfigSource::Env => format!("the {} environment variable", spec.env),
            // The tier is the merged document, but only a file can be opened.
            ConfigSource::Config
            // Not reachable from `resolve`, which reports the tier as `Config`.
            // They answer rather than panic for a caller that resolves otherwise.
            | ConfigSource::UserConfig
            | ConfigSource::ManagedConfig
            | ConfigSource::SystemManagedConfig
            | ConfigSource::EnvOverlay => {
                format!("the {} key in config.toml or managed_config.toml", spec.key)
            }
            ConfigSource::Remote => "a remote setting".to_owned(),
            ConfigSource::Default => "the default".to_owned(),
            // Not reachable either: no registered key has a flag to name.
            ConfigSource::Cli => "a command line override".to_owned(),
        }
    }

    /// Split this feature's config tier around the user `config.toml`. `active_campaigns` are the patches the effective
    /// merge applied, highest priority first; every layer is read from its own document, so a campaign that repeats a
    /// lower layer's value still shows above the user. A requirements pin is reported as `pin`, not as a config layer.
    pub fn config_layers(
        self,
        layers: &ConfigLayers,
        active_campaigns: &[CampaignEntry],
    ) -> FeatureConfigLayers {
        let key_in = |document: &toml::Value| -> Option<bool> {
            document.get("features")?.get(self.key())?.as_bool()
        };
        let layer_value = |layer: FeatureConfigLayer, document: &toml::Value| {
            key_in(document).map(|value| FeatureLayerValue { layer, value })
        };
        let pin = [
            &layers.mdm_requirements,
            &layers.system_requirements,
            &layers.user_requirements,
        ]
        .into_iter()
        .flatten()
        .find_map(key_in);
        let user = key_in(&layers.user);
        let below_user = layer_value(FeatureConfigLayer::Managed, &layers.managed)
            .or_else(|| layer_value(FeatureConfigLayer::SystemManaged, &layers.system_managed));
        let campaign = active_campaigns
            .iter()
            .find_map(|entry| entry.patch.get("features")?.get(self.key())?.as_bool())
            .map(|value| FeatureLayerValue {
                layer: FeatureConfigLayer::Campaign,
                value,
            });
        let above_user = layers
            .env_overlay
            .as_ref()
            .and_then(|overlay| layer_value(FeatureConfigLayer::Overlay, overlay))
            .or(campaign);
        FeatureConfigLayers {
            pin,
            user,
            above_user,
            below_user,
        }
    }

    /// Pin, then environment, then config, then remote, then the default.
    pub fn resolve(self, sources: FeatureSources) -> Resolved<bool> {
        let spec = self.spec();
        BoolFlag::env_value(sources.env)
            .requirement(sources.pin)
            .config(sources.config)
            .feature_flag(sources.remote)
            .default(spec.default_enabled)
            .resolve()
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
