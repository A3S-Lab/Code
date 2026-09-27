# Fork status

The thin `a3s-code-tui` scaffold under `crates/code/tui/src` is **not** the
product TUI. The product surface is the Apache-2.0, A3S-branded UI fork at:

```text
crates/code/vendor/a3s-code-ui/
```

Binary: `a3s-code-pager-bin` → `a3s-code-tui`

`just code` builds that fork and launches it via `A3S_CODE_TUI_BIN`.

## Agent

When `A3S_ACP_AGENT_BIN` is set (as `just code` does), the pager spawns
`a3s-code-acp` (`crates/code/acp`) over stdio. That binary implements ACP and
drives `a3s-code-core` 9.1.0 sessions — not an upstream in-process agent.
`a3s-code-acp` is built with `headless-search` and uses the Moli sidecar
shipped beside the `a3s` CLI (`A3S_CODE_MOLI_EXECUTABLE`, or `bin/moli/moli`
next to the `a3s` executable on `PATH`).

Without `A3S_ACP_AGENT_BIN`, the fork still falls back to the legacy in-process
shell agent for emergency debugging only.

## CC Switch models

On launch, `a3s-code-acp` imports the **current Claude** provider from a local
CC Switch database (`~/.cc-switch/cc-switch.db`) as provider `cc-switch`. Those
models appear in the ACP model catalog (and therefore the TUI model picker).

| Behavior | Detail |
| -------- | ------ |
| Default DB | `~/.cc-switch/cc-switch.db` |
| Override DB | `A3S_CC_SWITCH_DB=/path/to/cc-switch.db` |
| Disable | `A3S_DISABLE_CC_SWITCH=1` |
| Prefer as default | `A3S_PREFER_CC_SWITCH=1` (overrides ACL `default_model`) |
| Proxy | When CC Switch Claude proxy is enabled, base URL becomes `http://<listen>:<port>` |
| Otherwise | Uses the provider's `ANTHROPIC_BASE_URL` directly |

Model ids look like `cc-switch/glm-5.3-flash[1M]`. No ACL rewrite is required;
keys stay in CC Switch.

## Grok account models

When Grok CLI is logged in (`~/.grok/auth.json` + `models_cache.json`), those
models are imported as provider `grok` and call
`https://cli-chat-proxy.grok.com/v1` with the stored bearer token.

| Behavior | Detail |
| -------- | ------ |
| Default home | `~/.grok` (fallback `~/.a3s` if it has `auth.json`) |
| Override home | `A3S_GROK_HOME` or `GROK_HOME` |
| Disable | `A3S_DISABLE_GROK_ACCOUNT=1` |
| Prefer as default | `A3S_PREFER_GROK=1` |

Model ids look like `grok/grok-4.7`.

## Model catalog merge order

1. All providers/models from merged ACL (home `~/.a3s/config.acl` + workspace
   `.a3s/config.acl`, or `--config`)
2. CC Switch current Claude provider (`cc-switch/…`)
3. Logged-in Grok account (`grok/…`)
4. `A3S_DEFAULT_MODEL` env override (highest priority for the default only)

ACL `default_model` is preserved unless a prefer flag fills a missing default.

## Branding

User-facing copy, documentation, and comments use A3S Code / A3S Lab.
Internal crate names use the `a3s-code-*` / `a3s_code_*` prefix. ACP wire method
keys that are protocol identifiers (`x.ai/…`) stay unchanged.

Welcome braille art is the A3S circular mark (stylized A + wave) in
`assets/logo/logo07.txt` / `logo05.txt`. The version badge reads
`a3s-code-version` (aligned to a3s-code **9.1.0**).

## Model picker Provider Tabs

When the ACP catalog has models from two or more providers (ACL, `cc-switch`,
`grok`, …), `/model` shows Provider Tabs and filters the list to the active
provider. Tab labels are the provider segment of each model id.

## `/effort` → BudgetProfile

Every catalog model advertises `supportsReasoningEffort` and a
`reasoningEfforts` menu whose entries include `value` (`low`, `medium`, `high`,
`xhigh`, `max`). `high` is the default, matching the a3s-code TUI. The pager
sends `_meta.reasoningEffort`. `a3s-code-acp` applies the matching a3s
`BudgetProfile` from `crates/cli/src/budget.rs` on the core session: thinking
budget, max tool rounds, max parallel tasks, max continuation turns, and the
profile's effort guideline. `ultracode`, if the token arrives, also turns on
automatic delegation.

| id | thinking_budget | max_tool_rounds |
| --- | --- | --- |
| low | 2048 | 240 |
| medium | 8192 | 800 |
| high | 16384 | 1200 |
| xhigh | 32768 | 1800 |
| max | 65536 | 2400 |

`ultracode` is accepted if the token arrives (65536 / 3200 rounds) but is not
on the menu, because the pager `ReasoningEffort` enum has no ultracode variant.

## a3sCode slash gate

When `InitializeResponse.meta.a3sCode` is true, the pager hides builtins that
have no a3s-code-acp backing (billing, Grok marketplace, plan store, media,
shell memory ops, etc.) via `CommandRegistry::set_backend_unavailable_commands`.
Those commands are omitted from autocomplete and the command palette — they are
not shown as tier upsells. Visible commands must map to working pager-local or
ACP-backed behavior.

## ACP tool chrome

`a3s-code-acp` maps a3s tool names onto ACP `ToolKind` and copies arguments into
`raw_input` (and bash stdout into `raw_output` as `ToolOutput::Bash`) so the
pager renders Execute, Read, Edit, Search, Fetch, and list-dir cards.
`ToolExecutionStart` supplies the arguments; `ToolEnd` marks a non-zero exit as
failed. `edit` / `write` also attach an ACP diff.

Task planning uses `SessionUpdate::Plan` from core `TaskUpdated` / `PlanningEnd`.
`update_plan` is tagged as the pager todo tool so it does not duplicate the
plan strip. Subagent runs use `x.ai/session_notification`
(`subagent_spawned` / `subagent_progress` / `subagent_finished`); the `task`
tool card is suppressed in favor of that block.

Core `ConfirmationRequired` maps to ACP `session/request_permission` with the
same kind and `raw_input`. The pager shows the permission modal, or
auto-selects AllowOnce under `/always-approve` YOLO. The Plan permission mode
and `/auto` stay hidden.
