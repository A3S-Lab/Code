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
4. Run the Release workflow's qualification mode on main. It builds and checks
   the five-platform ACP/TUI archive matrix without uploading packages.
5. Create the signed `v9.1.4` tag at the qualified main commit. The tag runs
   the same qualification, uploads Core followed by ACP, and publishes the
   complete binary archives and checksums as a GitHub Release.

The workflow also supports an explicit dispatch on the stable tag with
`publish=true` to resume a failed delivery. `publish_sdks` defaults to false;
the aligned SDKs remain outside this runtime release. Existing active registry
versions are verified before a retry skips them; a failed ACP upload keeps the
release red. Source code is on main, but that alone does not mean a version has
been uploaded.

Status: candidate. Gates 1–3 are recorded in
[the 9.1.4 candidate evidence](../../../docs/evidence/code-harness/20261017-release-914-candidate/OUTCOME.md).
Final-source packaging, real-provider smoke and workflow qualification are
tracked in [the runtime release record](../docs/evidence/release/20261018-runtime-914/OUTCOME.md).
The registry upload and hosted archive matrix remain pending until that record
identifies successful delivery.
