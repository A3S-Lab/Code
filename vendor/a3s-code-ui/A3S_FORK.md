# A3S Code UI fork notes

This tree is the A3S Code TUI surface used by `a3s-code-tui`.

Product direction:
- Keep the pager UI (scrollback, prompt, chrome) as the coding surface.
- Drive sessions with `a3s-code-core` 9.1.1 over ACP (`a3s-code-acp`).
- Branding and slash catalog are A3S Code only (`a3s-code-*` / `a3s_code_*`).

Binary entry: `a3s-code-tui` (from `crates/codegen/a3s-code-pager-bin`).

Upstream license attribution stays in this tree's Apache-2.0 `LICENSE` files.

Upstream snapshot: https://github.com/xai-org/grok-build commit
`f0e3be1100ef5252488e3be8bb0e91cf68d8c305` (2026-09-23).
