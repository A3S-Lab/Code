# 9.1.4 release candidate

`a3s-code-core` 9.1.3 is on crates.io. It was published on 2026-10-15
from `2ba6f4fc`, the sha in the published crate's `.cargo_vcs_info.json`.
`a3s-code-acp` was never published. At that version ACP cannot be
published: it uses Core APIs that landed after the 9.1.3 upload. 9.1.4
releases the delta since `2ba6f4fc` and aligns every release member.

## Scope

| Member | 9.1.4 delivery |
| --- | --- |
| `a3s-code-core` | crates.io |
| `a3s-code-acp` | crates.io, published after Core |
| `a3s-code-tui` pager (`vendor/a3s-code-ui`) | release binary. Not a crates.io package: 18 path-only dependencies |
| Node, Python and Go SDKs | version-aligned at 9.1.4, published in the next release |

The delta is listed under `## [9.1.4]` in `CHANGELOG.md`.

## Gate

1. `scripts/check_release_versions.sh 9.1.4` passes.
2. `cargo publish -p a3s-code-core -p a3s-code-acp --dry-run` passes from
   the committed source. Cargo verifies ACP against the packaged Core.
3. `cargo install --path acp` from a clean checkout of the committed
   source, then one ACP turn against a real model.
4. Upload Core, then ACP. The upload is a manual step, outside this
   candidate's automation.

Status: candidate. Gates 1–3 are recorded in
[the 9.1.4 candidate evidence](../../../docs/evidence/code-harness/20261017-release-914-candidate/OUTCOME.md).
Gate 4 has not run.
