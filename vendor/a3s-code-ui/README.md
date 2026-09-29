# A3S Code TUI UI (pager)

This tree is the **A3S Code** interactive TUI UI — rebranded and wired to
`a3s-code-core` 9.1.1.

| Item | Value |
| --- | --- |
| Product binary | `a3s-code-tui` (`a3s-code-pager-bin`) |
| Agent binary | `a3s-code-acp` (`crates/code/acp`) |
| Launch | `just code` (sets `A3S_CODE_TUI_BIN` + `A3S_ACP_AGENT_BIN`) |

## Branding

User-visible strings, docs, and comments in this tree use **A3S Code** / **A3S Lab**
product identity. Product crates use the `a3s-code-*` / `a3s_code_*` prefix.

ACP extension method keys that start with `x.ai/` are **protocol identifiers**
on the wire and are kept for compatibility with the pager’s ACP extensions.

Apache-2.0 `LICENSE` files keep upstream copyright notices as required.

## Attribution

Upstream provenance and NOTICE details live in `crates/code/tui/NOTICE` and
`crates/code/tui/docs/FORK.md` — not in product READMEs.
