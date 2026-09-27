# A3S Code TUI architecture (Hybrid)

Rebuild target: full-screen coding TUI for `a3s code`, on **a3s-code-core 9.1.0**,
using **grok-build UI building blocks** and **A3S business logic**.

## First principles

1. **UI chrome is not product identity.** Scrollback geometry, prompt editing,
   fuzzy slash menus, and markdown/ANSI rendering are commodity TUI surfaces.
   grok-build already ships strong implementations under Apache-2.0 — vendor those.
2. **Coding turns are owned by a3s-code-core.** Sessions, fact-log ingest,
   Meta Harness, planning modes, tools, and ACL config stay on core 9.1.0.
   Do not reimplement the agent loop inside the pager.
3. **Business catalog stays A3S.** Slash commands, permission modes, approvals,
   `/ctx` / `/use` hubs, worktree isolation, and CLI launch layering are A3S
   product surface — port from `crates/cli` TUI, do not inherit Grok commands.
4. **Seam is ACP-shaped, not a Grok fork.** The TUI talks to an in-process
   agent adapter with ACP-like operations (prompt, cancel, permission). The
   adapter calls `a3s-code-core`. External ACP agents remain a future option.
5. **Do not fork 547k LOC.** Full `a3s-code-pager` embeds Grok auth, shell, and
   agent runtime. Hybrid vendors separable crates only.

## Layers

```text
┌─────────────────────────────────────────────┐
│  a3s-code-tui (this crate)                  │
│  screen loop · slash catalog · chrome       │
├──────────────────┬──────────────────────────┤
│  Vendored UI     │  A3S business            │
│  prompt-textarea │  SLASH_COMMANDS (CLI)    │
│  ratatui-inline  │  ACL launch layers       │
│  markdown-core   │  approvals / modes       │
│  scrollback geo  │  history / export        │
├──────────────────┴──────────────────────────┤
│  agent::CodeAgentAdapter (ACP-shaped)       │
│  stream + y/n confirm + markdown flatten    │
├─────────────────────────────────────────────┤
│  a3s-code-core 9.1.0                        │
│  Agent · Session · fact-log send/stream     │
└─────────────────────────────────────────────┘
```

## Entry points

- Binary: `a3s-code-tui` (`src/main.rs`)
- Library: `run_fullscreen`, `merge_launch_layers`, `submit_configured_turn`
- CLI: `crates/cli` spawns `a3s-code-tui` for non-smoke `a3s code` sessions

## Out of scope for this rebuild

- Replacing `a3s-tui` TEA (used by `a3s top` and other CLI surfaces)
- Shipping Grok login / xAI-only model discovery
- Claiming Enterprise GA or Terminal-Bench qualification
