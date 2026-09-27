# Capability inventory: A3S Code TUI ↔ a3s-build pager

Source of A3S business: `crates/cli/src/tui` (in-process TUI before spawn of
`a3s-code-tui`). Source of UI surfaces: `/tmp/grok-build` @
`f0e3be1100ef5252488e3be8bb0e91cf68d8c305`.

Status legend: **keep** (A3S), **vendor** (grok UI), **adapt** (shape from grok,
behavior from A3S), **drop** (Grok-only), **port** (not yet in a3s-code-tui).

## UI surfaces

| Capability | A3S (cli TUI) | grok-build | Plan |
| ---------- | ------------- | ---------- | ---- |
| Prompt editor / keys | custom + a3s-tui | `a3s-ratatui-textarea` | **vendor** → `vendor/prompt-textarea` ✅ |
| Inline ANSI / line split | limited | `a3s-ratatui-inline` | **vendor** → `vendor/ratatui-inline` ✅ |
| Scrollback column layout | custom | pager scrollback layout | **adapt** → `src/scrollback` ✅ |
| Slash fuzzy match | custom | pager `slash/matcher` + nucleo | **adapt** → `src/slash` ✅ |
| Markdown streaming | design_markdown / comrak | `a3s-code-markdown*` | **vendor** core ✅; plain flatten ✅; rich ratatui render later |
| Diff hunks | file_change_view | `a3s-code-pager-diff` | **adapt** later (needs tools shape) |
| Full pager agent UI | — | `a3s-code-pager` 547k LOC | **drop** fork; Hybrid only |
| Auth / login chrome | `/login` OS account | `a3s-code-login` | **keep** A3S; **drop** Grok auth |
| ACP wire channels | — | `a3s-acp-lib` | **adapt** → `src/agent` ACP-shaped |

## A3S slash catalog (port target)

From `crates/cli/src/tui/ui/chrome.rs` `SLASH_COMMANDS` (47 commands).

### Workflow

`/ask` `/plan` `/review` `/reviewer` `/tasks` `/queue` `/research` `/init`
`/compact` `/effort` `/unstick`

### Session & control

`/status` `/history` `/relay` `/fork` `/worktree` `/isolate` `/rewind`
`/clear` `/copy` `/export` `/exit`

### Context & memory

`/ctx` `/memory` `/evolution` `/kb` `/sleep`

### Assets & services

`/use` `/plugin` `/packages` `/reload` `/hooks` `/desktop`

### System & interface

`/model` `/permissions` `/sandbox` `/config` `/checkup` `/login` `/logout`
`/update` `/help` `/terminal` `/theme` `/display` `/statusline` `/ide`
`/goal` `/loop` `/auto` `/yolo`

Browse-hidden (typed only): see `SLASH_BROWSE_HIDDEN` in chrome.rs.

## Modes & safety (port)

| Surface | A3S | Notes |
| ------- | --- | ----- |
| Planning | `/ask` `/plan` · Shift+Tab | maps to `PlanningMode` / `CodeMode` |
| Autonomy | `/auto` `/yolo` · Shift+Tab | permission mode cycle |
| Approvals | `ui/approval.rs` · panels | grant UI before tool exec |
| Sandbox | `/sandbox` | host Bash readiness |
| Isolation | `/fork` `/worktree` `/isolate` | managed worktrees |

## Coding loop (on core 9.0.0)

| Surface | Status in a3s-code-tui |
| ------- | ---------------------- |
| ACL layer merge (home + workspace / `--config`) | ✅ `merge_launch_layers` |
| Fact-log turn `session.send` / `session.stream` | ✅ `CodeAgentAdapter` |
| Model from ACL `default_model` (ignore `XAI_API_KEY`) | ✅ |
| ACL multi-provider model catalog | ✅ via `a3s-code-acp` |
| CC Switch current Claude models (`cc-switch/…`) | ✅ via `a3s-code-acp` |
| Logged-in Grok account models (`grok/…`) | ✅ via `a3s-code-acp` |
| Streaming deltas / tool chrome | ✅ `prompt_streaming` + tool lines |
| Markdown flatten for scrollback | ✅ `markdown_to_plain` |
| Session resume | fact-log shared id `tui-session` ✅ |
| Permission prompts | ✅ y/n confirm; auto/yolo auto-approve |
| `/compact` | ✅ `x.ai/compact_conversation` writes `compaction.done` |
| Interactive question options | **port** (displayed; answer path TBD) |
| Full `/ctx` `/use` panel UIs | **port** (CLI panels; forward string for now) |

## Explicit non-goals from a3s-build

- Grok product name, themes, announcements
- xAI-only model list / sampler
- Embedded `a3s-code-shell` as the agent
- Foreign-session / dashboard store crates

## a3sCode visibility gate (pager + ACP)

When `a3s-code-acp` advertises `meta.a3sCode: true`, the pager applies
`A3S_CODE_UNAVAILABLE_COMMANDS` via `set_backend_unavailable_commands` so
unsupported builtins (`/usage`, `/view-plan`, `/plan`, `/auto`, `/tutorial`,
`/docs`, `/privacy`, marketplace, plugins, media, voice, …) are fully hidden —
not tier-upsold. Voice and `coding_data_sharing` settings rows follow
`apply_voice_mode_enabled(false)` / `A3S_CODE_ACTIVE`. Visible autonomy is
`/always-approve` (YOLO), which auto-selects AllowOnce on
`request_permission`. `/effort` maps to cli `BudgetProfile` budgets. `/goal`
sets one durable objective and calls Core `set_planning_mode`; status, pause,
and clear do not start a model turn. `/compact` asks the session model for a
summary and appends `compaction.done` so the next turn keeps that summary
instead of the older transcript. Tool
starts/ends emit ACP `ToolCall` updates; HITL uses
`ConfirmationRequired` → `session/request_permission`.
`A3S_CODE_EFFECTIVE_COMMANDS` is the allowlist of remaining surfaces that must
stay working.
