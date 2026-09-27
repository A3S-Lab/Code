//! A managed-policy hook must not be disabled through `handle_hooks_action`.
//!
//! These tests drive the shipped `handle_hooks_action` with a real registry whose hook carries a managed-policy provenance
//! (the root-owned `/etc/a3s` tier and the signed synced `$GROK_HOME/requirements.toml`).
//! The per-hook `Disable` action must be refused and the bulk `ToggleSource` must skip it, both before writing any disable state.
//! The dispatcher-level exemption and the display predicate are covered with a sandboxed `GROK_HOME` in `a3s_code_hooks::dispatcher` tests.

use super::support::*;
use super::*;

use std::sync::Arc;
use tokio::sync::mpsc;

use a3s_code_hooks::config::HookProvenance;

/// The root-owned tier's hook; the pin tests below use it as their one managed hook.
const MANAGED_HOOK: &str = "requirements/system:pre_tool_use[0].hooks[0]";

/// One hook per managed-policy tier that `/hooks` must refuse to disable.
const MANAGED_HOOKS: [(&str, HookProvenance); 2] = [
    (MANAGED_HOOK, HookProvenance::Requirements),
    (
        "requirements/signed:pre_tool_use[0].hooks[0]",
        HookProvenance::SignedRequirements,
    ),
];

/// Snapshots and restores the disabled-hooks file wherever the process actually resolves it.
/// A temp `GROK_HOME` alone cannot redirect it: `grok_home()` is `OnceLock`-cached and another test in this binary may have resolved it first.
/// The guard lets the test assert nothing was written, and if the no-disable rule ever regresses it restores the developer's or CI's real file.
struct DisabledHooksGuard {
    path: Option<std::path::PathBuf>,
    before: Option<String>,
}

impl DisabledHooksGuard {
    fn capture() -> Self {
        let path = a3s_code_config::user_grok_home().map(|home| home.join("disabled-hooks"));
        let before = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        Self { path, before }
    }

    fn assert_unchanged(&self) {
        let after = self
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        assert_eq!(
            after, self.before,
            "no disable state may be written for a managed-policy hook"
        );
    }

    fn assert_disabled(&self, hook: &str) {
        let after = self
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        assert!(
            after.lines().any(|line| line.trim() == hook),
            "disable state must name {hook}: {after:?}"
        );
    }
}

impl Drop for DisabledHooksGuard {
    fn drop(&mut self) {
        let Some(path) = &self.path else { return };
        match &self.before {
            Some(content) => {
                let _ = std::fs::write(path, content);
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Builds a registry with one command hook under the given managed-policy provenance.
fn managed_registry(name: &str, layer: HookProvenance) -> a3s_code_hooks::discovery::HookRegistry {
    a3s_code_hooks::discovery::registry_from_specs_deduped(vec![a3s_code_hooks::config::HookSpec {
        name: name.to_string(),
        event: a3s_code_hooks::event::HookEventName::PreToolUse,
        handler_type: a3s_code_hooks::config::HandlerType::Command,
        configured_matcher: None,
        matcher: None,
        enabled: true,
        command: Some(std::path::PathBuf::from("exit 0")),
        command_raw: Some("exit 0".to_string()),
        url: None,
        url_raw: None,
        timeout_ms: 5000,
        source_dir: std::env::temp_dir(),
        extra_env: std::collections::HashMap::new(),
        layer,
    }])
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial(disabled_hooks_file)]
async fn managed_policy_hook_disable_actions_are_refused() {
    let guard = DisabledHooksGuard::capture();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let actor = Arc::new(actor);

            for (hook, layer) in MANAGED_HOOKS {
                *actor.hook_registry.borrow_mut() = Some(Arc::new(managed_registry(hook, layer)));

                let outcome = actor
                    .handle_hooks_action(a3s_hooks_plugins_types::HooksAction::Disable {
                        hook_name: hook.to_string(),
                    })
                    .await;
                assert_eq!(
                    outcome.status,
                    a3s_hooks_plugins_types::OutcomeStatus::ValidationError,
                    "disable of {hook} must be refused: {}",
                    outcome.message
                );
                assert!(
                    outcome.message.contains("managed policy"),
                    "refusal for {hook} must say why: {}",
                    outcome.message
                );

                let outcome = actor
                    .handle_hooks_action(a3s_hooks_plugins_types::HooksAction::ToggleSource {
                        hook_names: vec![hook.to_string()],
                        disable: true,
                    })
                    .await;
                assert!(
                    outcome.message.contains("enforced by managed policy"),
                    "bulk disable of {hook} must report the managed skip: {}",
                    outcome.message
                );
                assert!(
                    outcome.message.contains("Disabled 0/1"),
                    "{hook} may not actually be disabled: {}",
                    outcome.message
                );
            }
        })
        .await;
    // Both refusals happened before any disable state was written
    guard.assert_unchanged();
}

/// Under `allow_managed_hooks_only`, enabling a non-managed hook (per hook, or per source when any name in the group is non-managed)
/// is refused before any disable state changes; a managed-policy hook or an all-managed group passes, since the pin never blocks them.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial(disabled_hooks_file)]
async fn managed_hooks_only_refuses_enabling_non_managed_hooks() {
    const HOOK: &str = "global/qa:pre_tool_use[0].hooks[0]";
    let guard = DisabledHooksGuard::capture();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.hook_registry.borrow_mut() = Some(Arc::new(managed_registry(
                MANAGED_HOOK,
                HookProvenance::Requirements,
            )));
            let actor = Arc::new(actor);
            *actor.hook_disabled.borrow_mut() =
                Arc::new(a3s_code_hooks::trust::DisabledHooks::new([], true));

            for action in [
                a3s_hooks_plugins_types::HooksAction::Enable {
                    hook_name: HOOK.to_string(),
                },
                a3s_hooks_plugins_types::HooksAction::ToggleSource {
                    hook_names: vec![HOOK.to_string()],
                    disable: false,
                },
                a3s_hooks_plugins_types::HooksAction::ToggleSource {
                    hook_names: vec![MANAGED_HOOK.to_string(), HOOK.to_string()],
                    disable: false,
                },
            ] {
                let outcome = actor.handle_hooks_action(action).await;
                assert_eq!(
                    outcome.status,
                    a3s_hooks_plugins_types::OutcomeStatus::ValidationError,
                    "{}",
                    outcome.message
                );
                assert_eq!(
                    outcome.message,
                    super::hooks_plugins::MANAGED_HOOKS_ONLY_REFUSAL
                );
            }
            guard.assert_unchanged();

            // Past the guard the outcome depends on the host's disabled-hooks file (the guard's Drop restores it).
            for action in [
                a3s_hooks_plugins_types::HooksAction::Enable {
                    hook_name: MANAGED_HOOK.to_string(),
                },
                a3s_hooks_plugins_types::HooksAction::ToggleSource {
                    hook_names: vec![MANAGED_HOOK.to_string()],
                    disable: false,
                },
            ] {
                let outcome = actor.handle_hooks_action(action).await;
                assert_ne!(
                    outcome.status,
                    a3s_hooks_plugins_types::OutcomeStatus::ValidationError,
                    "managed-policy hooks must pass the allow_managed_hooks_only guard: {}",
                    outcome.message
                );
            }
        })
        .await;
}

/// The pin only refuses enabling: bulk disable of a non-managed hook still writes disable state under `allow_managed_hooks_only`,
/// since turning more hooks off never widens what runs.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial(disabled_hooks_file)]
async fn managed_hooks_only_still_allows_bulk_disable() {
    const HOOK: &str = "global/qa:pre_tool_use[0].hooks[0]";
    let guard = DisabledHooksGuard::capture();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.hook_registry.borrow_mut() = Some(Arc::new(managed_registry(
                MANAGED_HOOK,
                HookProvenance::Requirements,
            )));
            let actor = Arc::new(actor);
            *actor.hook_disabled.borrow_mut() =
                Arc::new(a3s_code_hooks::trust::DisabledHooks::new([], true));

            let outcome = actor
                .handle_hooks_action(a3s_hooks_plugins_types::HooksAction::ToggleSource {
                    hook_names: vec![HOOK.to_string()],
                    disable: true,
                })
                .await;
            assert_eq!(
                outcome.status,
                a3s_hooks_plugins_types::OutcomeStatus::Success,
                "{}",
                outcome.message
            );
            assert!(
                outcome.message.starts_with("Disabled 1/1"),
                "bulk disable must still write under the pin: {}",
                outcome.message
            );
            guard.assert_disabled(HOOK);
        })
        .await;
}

/// Enable/disable refresh the dispatch snapshot in place, so `hook_run_ctx` never reads the disabled-hooks file itself.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial(disabled_hooks_file)]
async fn toggling_a_hook_refreshes_the_dispatch_snapshot() {
    const HOOK: &str = "global/qa:pre_tool_use[0].hooks[0]";
    let _guard = DisabledHooksGuard::capture();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor =
                Arc::new(create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await);
            let _ = a3s_code_hooks::trust::enable_hook(HOOK);
            actor.refresh_hook_disabled();
            assert!(!actor.hook_run_ctx().disabled().contains(HOOK));

            let outcome = actor
                .handle_hooks_action(a3s_hooks_plugins_types::HooksAction::Disable {
                    hook_name: HOOK.to_string(),
                })
                .await;
            assert_eq!(
                outcome.status,
                a3s_hooks_plugins_types::OutcomeStatus::Success
            );
            assert!(
                actor.hook_run_ctx().disabled().contains(HOOK),
                "the next dispatch must see the disable without reading the file"
            );

            let outcome = actor
                .handle_hooks_action(a3s_hooks_plugins_types::HooksAction::Enable {
                    hook_name: HOOK.to_string(),
                })
                .await;
            assert_eq!(
                outcome.status,
                a3s_hooks_plugins_types::OutcomeStatus::Success
            );
            assert!(!actor.hook_run_ctx().disabled().contains(HOOK));
        })
        .await;
}
