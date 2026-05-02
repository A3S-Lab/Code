# A3S Code 2.0 Refactor Completion Report

## Status

The 2.0 refactor is functionally complete.

The codebase now follows the intended harness-driven shape:

```text
Agent / AgentSession
  -> context assembly
  -> tool selection
  -> permission and confirmation policy
  -> execution
  -> trace, artifacts, verification evidence
  -> compaction
```

Routine multi-agent work has one default path: `task` and `parallel_task`.
Advanced `Orchestrator` APIs remain available only for explicit SubAgent
lifecycle control and event monitoring.

## Removed 1.0 Mechanisms

- Deleted explicit Lead/Worker/Reviewer `AgentTeam` and `TeamRunner` APIs.
- Removed model-visible `run_team` and SDK `runTeam` shortcuts.
- Removed the public task/progress/idle lifecycle bindings.
- Deleted the legacy cron scheduler and `/loop` commands.
- Removed placeholder SubAgent execution.
- Removed SubAgent-level permissive permission shortcuts.
- Removed public `PermissionPolicy::permissive()`.
- Removed SubAgent/Orchestrator lane-queue coupling.
- Removed generic queue task submission helpers from public session APIs.
- Removed `.hcl` config support; `.acl` is the supported config format.
- Removed unused prompt files, undercover mode, tool search side path, and stale
  historical SDK investigation docs.

## Retained Mechanisms

- `Agent` and `AgentSession` as the public facade.
- `task` and `parallel_task` as the default delegation tools.
- `Orchestrator` as an advanced SubAgent lifecycle control plane.
- Session-level external/hybrid queue dispatch for explicit integrations.
- AHP 2.3 as a harness extension.
- Permission policies with explicit default decisions and rule lists.
- Trace events, artifact references, and verification reports as completion
  evidence.
- Programmatic Tool Calling for bounded, repeatable tool chains.

## Public Surface

The Rust crate root now exposes the intended 2.0 facade and keeps low-level
runtime modules internal:

- `Agent`
- `AgentSession`
- `AgentEvent`
- `AgentResult`
- `SessionOptions`
- config, LLM, error, and prompt slot types

Node and Python bindings now consume these root exports instead of reaching into
low-level `agent` / `agent_api` modules.

## Verification

Latest verification pass:

- `cargo test -p a3s-code-core --lib`
  - 1459 passed
  - 0 failed
  - 2 ignored
- `cargo check -p a3s-code-core --all-targets --features ahp`
- `cargo check --manifest-path sdk/node/Cargo.toml --all-targets`
- `cargo check --manifest-path sdk/python/Cargo.toml --all-targets`
- `cargo test --manifest-path sdk/node/Cargo.toml --lib`
- `cargo test --manifest-path sdk/python/Cargo.toml --lib`
- `npx tsc --noEmit` in `sdk/node/examples`
- `npm --cache /private/tmp/a3s-npm-cache pack --dry-run` in `sdk/node`
- `npm run build` in `sdk/node`
- `npm test` in `sdk/node`
- `npm run test:helpers` in `sdk/node`
- `npm run basic:minimax` in `sdk/node/examples`
- `npm run orchestrator:kimi` in `sdk/node/examples`
- `npm run smoke` in `sdk/node/examples`
- `npx tsx basic/test_runtime_nesting.ts` in `sdk/node/examples`
- `python3 -m maturin build --release --out /private/tmp/a3s-code-py-dist` in `sdk/python`
- wheel import smoke:
  `PYTHONPATH=/private/tmp/a3s-code-py-import python3 -c 'import a3s_code'`
- `cargo fmt --all --check`
- `git diff --check`
- `scripts/check_release_versions.sh`
- `cargo test -p a3s-code-core --test test_real_config_env_integration`
  - verifies repo `.a3s/config.acl` resolves MiniMax defaults through injected
    `A3S_OPENAI_*` env vars and `MINIMAX_*` aliases without making network calls
- `scripts/real_config_env_integration.sh --dry-run`
  - runs the no-network ACL env-injection checks directly
- `scripts/real_config_env_integration.sh`
  - can extract literal OpenAI/MiniMax credentials from `.a3s/config.acl`,
    inject them into `A3S_OPENAI_*` for the test process, and run against a
    temporary env-style config without modifying the source config
- `scripts/release_preflight.sh`
  - passes all non-network gates
  - skips real-provider smoke unless `A3S_OPENAI_*` / `MINIMAX_*` is injected
    or literal OpenAI/MiniMax credentials are present in the config

## Residual Risks

- Real LLM examples and network-backed browser/search flows still require
  provider credentials in the local environment.
- Large deleted-example churn should be reviewed once before release so no
  intentionally supported sample was removed accidentally.
- Node native bindings were rebuilt during final verification so `index.js` and
  `index.d.ts` match the current Rust N-API surface.
- Real-provider Node examples now skip cleanly when required credentials/config
  are missing, and TypeScript ESM examples no longer rely on CommonJS
  `__dirname`.
- The examples package has a no-credential smoke script, and its Quick Start no
  longer points users at a live-provider test by default.

## Release Readiness

Before tagging a 2.0 release:

1. Run `REQUIRE_REAL_PROVIDER=1 scripts/release_preflight.sh` with `A3S_OPENAI_API_KEY` / `A3S_OPENAI_BASE_URL` injected, or use the MiniMax aliases `MINIMAX_API_KEY` / `MINIMAX_BASE_URL`.
2. Review generated TypeScript declarations and Python docs.
3. Commit the refactor as one architecture-level change or split by phase.
4. Tag and publish from a clean worktree.
