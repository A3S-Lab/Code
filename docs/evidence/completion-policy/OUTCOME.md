# Explicit completion verification policy

## Change

Add a host-only per-session policy. Strict behavior remains the default. The opt-in may waive only absence of a report for the current effect, not a bound report with a failed, needs-review or incomplete result. Tool-effect uncertainty, unfinished children and observations remain blocking. The opt-in uses the existing assistant-final-response path rather than automatically stopping after a tool. No environment variable or workspace ACL can enable it.

## Validation

New regression coverage: strict default, missing evidence, failed/needs-review/passed reports, empty checks, incomplete observations, external work, and policy name parsing. Rust 1.97.1: `cargo test -p a3s-code-core --lib harness_loop::tests --no-default-features` passed 47 tests, including seven new policy regressions. `cargo fmt --all --check` passed. The companion CLI compiled against this source and its fake-provider integration test passed, including a Waived terminal and a final model response. Historical benchmark tests refer to a different prototype.

Strict no-default-feature Clippy reports unused `pinned_epoch` and `LEGACY_MANIFEST_SCHEMA_VERSION` in unchanged `workspace/retrieval/persistent.rs`. The same command on clean cfaa22e8 reproduced both errors at the same lines. This is not a claim of a clean Clippy run.

## Explicitly not claimed

No benchmark improvement, no current patch full-suite pass, no sandbox or approval change, no synthetic verification pass, and no platform release. Historical HTML runs both passed 2/2, with 6 vs 50 model usage events; they justify investigation, not a performance claim.
