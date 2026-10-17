# apps/code AGENTS.md

The root `AGENTS.md` and `docs/development-standards.md` govern
everything here; this file adds the rules specific to the A3S Code
tree. When they disagree, the root wins.

## What this tree is

The native coding-agent harness (core) and its ACP process surface (acp).
The product terminal is the pager under `vendor/a3s-code-ui`. The in-process
TUI scaffold and the separate kernel package were removed.
Not a bounded-contexts service: the layering rules that shape
`apps/api` do not map one-to-one here; the equivalents are the facade
discipline per module and the never-cross lines called out below.

## Toolchain and workspace

- Rust `1.97.1` via the tree's own `rust-toolchain.toml` — plain
  `cargo` inside `apps/code/` resolves it; never `+stable`.
- This is its own cargo workspace: build from `apps/code/`, never from
  the monorepo root. `vendor/`, `website/`, and `target/` are not part
  of review surface.
- Native SDK crates (`sdk/node`, `sdk/python`) are excluded from the
  default workspace (special toolchains); build them individually.
- `--features deep-live` compiles the declaration-gated live/evidence
  files (`deep_e2e_live.rs`, `deep38_live.rs`): default builds never
  compile them, so a signature change touching them is only proven by
  a feature check.

## Configuration authority (SEC-EGRESS1)

- A3S ACL is the only configuration language. Layer order in the ACP
  launch path: explicit `--config`/`A3S_CONFIG`, then `~/.a3s/config.acl`,
  then the workspace `.a3s/config.acl` found by ancestor search.
- The workspace layer is UNTRUSTED repository content. It may configure
  the agent (models, skills, default_model) but never host-authoritative
  surfaces: `HOST_AUTHORITATIVE_BLOCKS` (`sandbox`, `providers` and
  workspace-scoped `verification_contract`, and `goal_control`; the extensions
  are recorded in `manual/HOST_VERIFICATION_CONTRACT.md` and
  `manual/HOST_GOAL_CONTROL.md`) is enforced at the
  `withhold_workspace_host_authoritative_blocks` choke point in
  `acp/src/config.rs`. Extending that set is a platform decision.
  SEC-WSTRUST1 extended it with host paths and executables (`search`,
  `sessions_dir`, `memory_dir`, `user_instructions_dir`,
  `global_instructions_dir`, `storage_backend`, `storage_url`, `os`).
- A workspace `mcp_servers` declaration stays inert until the user trusts
  its exact digest (`a3s-code-acp --trust-workspace-mcp <id|all>`); the
  digest binds the repository root and the whole declaration, and the
  store lives in the user state root, never in the repository. Workspace
  `skill_dirs`/`agent_dirs` entries that escape the repository root are
  dropped. Mechanism: `config::admit_untrusted_layers` in core.
- The process-host sandbox fallback (`A3S_CODE_ALLOW_PROCESS_HOST_SANDBOX`)
  only serves a deny request — never a mediated one.

## Error handling

- No `unwrap()`/`expect()` on production paths without the
  construction-level argument comment. `core/tests/production_unwrap_ratchet.rs`
  counts the legacy per file from the core and ACP module graphs (90
  sites on 2026-10-17); a row may only go down and a new file adds zero.
- Closed sets carry `parse()`/`as_str()` and the parse error names the
  set (`SandboxNetworkMode`, `InteractiveApprovalMode` are the
  reference shapes). Never coerce an unknown value to a default.
- Meaning is not matched (constitution XV): what a user MEANS —
  remember-intent, preference, importance — is decided by the model
  (prompt-layer rule) or a typed contract, never a regex/verb list.
  The memory extraction runtime's explicit-remember verb list is the
  one legacy hint: its correctness is the prompt's language-neutral
  rule (pinned cross-language); the list does not grow. Pattern
  matching only narrows fail-closed (secret redaction) or ranks
  candidates.
- Untrusted reads are bounded: text reads cap at 16 MiB with
  `take()` accounting; config layers at 1 MiB with symlink refusal;
  the regular-file gate runs BEFORE the open (a writer-less FIFO
  blocks the open itself).

## Testing

- Hermetic pins at the boundary a slice changes; real-model legs ride
  their own suites (`*_real_llm`) and are never silently skipped.
- Sibling test files (`<stem>/tests.rs` via a bare `mod tests;` — an
  explicit `#[path]` from a non-mod.rs file hijacks a same-named
  sibling). No inline test module over ~1000 lines; the current
  near-threshold set is tracked in the standards reviews.
- Mass-failure triage: re-run each distinct failure SOLO before
  believing it; attribute to contention, a sibling session's in-flight
  edits, or a real regression — in that order.
- PG/dual-adapter rules do not apply here (no repository adapters);
  the equivalent rigor bar is the live suites plus the family re-run.

## Delivery (the ratchet is mechanical)

- `just check-landing` (runs `scripts/check_landing.py origin/main`):
  every commit touching first-party code must commit its
  `docs/evidence/**/OUTCOME.md` (with the exact "Explicitly not
  claimed" section) and a ROADMAP gate-row touch in the SAME commit.
  The 43-entry renamed-section allowlist only shrinks.
- `just ci` = fmt + clippy (`-D warnings`) + tests + the ratchet. Run
  the clippy leg before committing — the pre-commit hook checks fmt
  only.
- Shared-tree discipline: stage by content-filtered hunks
  (`git apply --cached`), verify `git diff --cached --name-only`, and
  never commit a sibling session's in-flight edits. When your lines
  land in the same hunk as theirs, rebuild the patch against HEAD with
  only your lines.
