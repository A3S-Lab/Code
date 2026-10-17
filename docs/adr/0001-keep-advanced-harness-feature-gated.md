# ADR 0001: Keep the advanced harness feature-gated inside `a3s-code-core`

- Status: accepted (CORE-SPLIT1, 2026-10-17)
- Decision owner: core maintainers

## Context

`a3s-code-core` carries five advanced modules — `evaluation`, `research`,
`state_graph`, `flow_graph`, `dynamic_workflow` — about 25.9k of the crate's
413.9k lines (6.3%), behind the `advanced-harness` feature. The question was
whether to move them into a separate `a3s-code-advanced` crate so that the
default `local-code` embedder pays less in compile time and dependencies.

The split is only worth its cost (a new published crate, a cross-crate API
for the 73 `cfg(feature = ...)` sites in 22 files, one more crate in the
joint release train) if the advanced modules are a large part of the build.

## Measurement

Same snapshot, toolchain 1.97.1, macOS arm64 debug profile, one fresh target
directory per profile. Each profile is built with `--no-default-features`.

| Profile | Unique normal deps | Clean lib build | Crate `check` | rlib size |
|---|---:|---:|---:|---:|
| `local-code` (default) | 394 | 93 s | 33 s | 667 MB |
| `local-code,advanced-harness` | 396 | 91 s | 36 s | 717 MB |
| `full` (`scientific` + `server`) | 521 | 113 s | 56 s | 730 MB |

The advanced harness adds 2 dependencies (`a3s-flow` and its
`serde_yaml_ng`), no measurable clean-build time (the 2 s difference is below run
noise), about 3 s of `check`, and 7.5% of rlib size. The dependency and
build-time weight of `full` comes from `scientific`'s headless search and
SQLite memory and `server`'s S3 and OpenTelemetry stacks (+125 dependencies,
+20 s), which are already optional features.

## Decision

Do not split. Keep the advanced modules in `a3s-code-core` behind
`advanced-harness`, and keep module-path isolation:

- the default `local-code` profile does not compile them;
- core code outside the advanced modules may not depend on them except
  through `cfg(feature = ...)` sites;
- new advanced surfaces go behind an existing or new feature, never into
  the default profile.

## Consequences

- No new crate in the release train; the published surface is unchanged.
- Revisit this ADR if a profile adds more than about 10% clean-build time
  or more than 20 unique dependencies over `local-code`, or if the advanced
  modules need a release cadence of their own.
- The measurement script is retained with the evidence so the numbers can
  be reproduced.
