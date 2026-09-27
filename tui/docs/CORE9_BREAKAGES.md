# CLI ↔ a3s-code-core 9.0.0 breakage catalog

Scratch pin of `crates/cli` to `a3s-code-core =9.0.0` (worktree `/tmp/cli-v9`,
log `/tmp/cli-v9-check.log`) while the in-tree CLI still lists
`a3s-code-core =8.7.0`.

**Resolution path for this goal:** the shipped coding TUI is `a3s-code-tui`
(already on core 9.0.0). The in-process CLI TUI (`crates/cli/src/tui`) is
smoke/legacy; fix or delete breakages only where CLI still compiles against
core for non-TUI paths. Do not block the Hybrid TUI on fixing every CLI panel.

## Compile summary

- `error[E0308]` mismatched types: 27
- `error[E0277]` trait bounds: 2 (`LazyFileMemoryStore: MemoryStore`)
- `error[E0027]` non-exhaustive pattern: 1 (`allow_free_text` on question)

## By area

| Area | Files | Root cause (9.0.0) | Fix owner |
| ---- | ----- | ------------------ | --------- |
| Memory item shape | `evolution/store.rs`, `evolution/mod.rs`, `tui/panels/context/*`, `tui/context/memutil.rs` | `a3s_memory::MemoryItem` / store APIs changed; duplicate or renamed fields | CLI memory/evolution adapters — or drop panels when TUI is binary-only |
| Lazy memory store | `lazy_memory_store.rs`, `commands/code/host_must_wires.rs` | `LazyFileMemoryStore` no longer implements `MemoryStore` | Re-implement against 9.0.0 trait or use core's store |
| User questions | `tui/app/events.rs`, `tui/ui/question.rs` | `UserQuestionV1` gained `allow_free_text` | Update match / struct literals |
| Flow binding | `use_registry/flow_runtime.rs` | `a3s-flow` / `flow_binding` arity change with core 9.0.0 | Align call sites to `a3s-flow` 1.1.0 + core binding |
| Launch / model panel | `tui/app/launch.rs`, `tui/panels/system/model.rs` | Memory / clone bounds tied to store types | Prefer spawn `a3s-code-tui` path (already landed) |

## Already mitigated in CLI

Commits on CLI `main` ahead of origin:

- `33aa736` Launch A3S Code 9.0.0 full-screen TUI from `a3s code` (spawns binary)
- `f0f67ca` Pass CLI home + explicit ACL into Code TUI

Smoke mode keeps the in-process UI; normal sessions do not need the broken
in-process coding loop to compile against 9.0.0 for the Hybrid goal.

## a3s-code-tui (this crate)

Pinned to path `../core` at **9.0.0**. Turn path uses `Agent::from_config` +
`session.send` (fact-log). No MemoryItem / UserQuestionV1 compile debt here yet.
