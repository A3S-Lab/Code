# A3S Code hybrid TUI scaffold

This crate is **not** the `a3s code` product path.

`a3s code` starts the pager binary `a3s-code-tui` from
`apps/code/vendor/a3s-code-ui` (`a3s-code-pager-bin`). That pager speaks ACP
to `apps/code/acp` (`a3s-code-acp`), which drives `apps/code/core` 9.1.2.
The CLI that spawns them is `apps/cli` (`apps/cli/src/code_pager.rs`). It
passes `--cwd`. There is no `crates/cli`.

This crate is an in-process scaffold. Its binary is `a3s-code-tui-hybrid`
so a workspace build does not occupy the product name `a3s-code-tui`.
The scaffold still accepts `--cwd` and `--workspace`. Its fact-log session
id is the fixed string `tui-turn`.

## First principles

1. **UI chrome is not product identity.** Scrollback geometry, prompt editing,
   fuzzy slash menus, and markdown/ANSI rendering are commodity TUI surfaces.
   grok-build already ships strong implementations under Apache-2.0 — vendor those.
2. **Coding turns are owned by a3s-code-core.** Sessions, fact-log ingest,
   Meta Harness, planning modes, tools, and ACL config stay on core 9.1.2.
   Do not reimplement the agent loop inside the pager.
3. **The product catalog is the pager plus ACP.** Slash commands, permission
   modes, and approvals on the running `a3s code` screen come from the pager
   and `a3s-code-acp`, not from this scaffold.
4. **This scaffold calls core in process.** It is not the ACP process the
   pager launches.
5. **The product pager is the vendored fork.** It lives at
   `apps/code/vendor/a3s-code-ui`. This scaffold vendors only the separable
   UI crates under `tui/vendor`.

## Layers

```text
Product path

  apps/cli  a3s code
       │  --cwd
       ▼
  a3s-code-tui   (pager bin, vendor/a3s-code-ui)
       │  ACP stdio
       ▼
  a3s-code-acp   (apps/code/acp)
       ▼
  a3s-code-core 9.1.2

This crate

  a3s-code-tui-hybrid
       ▼
  in-process CodeAgentAdapter
       ▼
  a3s-code-core 9.1.2
```

## Entry points

- Binary: `a3s-code-tui-hybrid` (`src/main.rs`)
- Library: `run_fullscreen`, `merge_launch_layers`, `submit_configured_turn`
- Product launch: `apps/cli/src/code_pager.rs` spawns the pager, then
  `a3s-code-acp`

## Out of scope for this rebuild

- Replacing `a3s-tui` TEA (used by `a3s top` and other CLI surfaces)
- Shipping Grok login / xAI-only model discovery
- Claiming Enterprise GA or Terminal-Bench qualification
