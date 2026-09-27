# A3S Code UI fork notes

This tree is the A3S Code TUI surface used by `a3s-code-tui`.

Product direction:
- Keep the pager UI (scrollback, prompt, chrome) as the coding surface.
- Drive sessions with `a3s-code-core` 9.1.0 over ACP (`a3s-code-acp`).
- Branding and slash catalog are A3S Code only (`a3s-code-*` / `a3s_code_*`).

Binary entry: `a3s-code-tui` (from `crates/codegen/a3s-code-pager-bin`).

Upstream license attribution: see `crates/code/tui/NOTICE` and
`crates/code/tui/docs/FORK.md`.
