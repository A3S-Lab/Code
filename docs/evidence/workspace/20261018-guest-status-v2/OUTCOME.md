# Reject legacy guest worker settlement

The v1 development shim could report its VMM worker's exit code as if it were
the payload status. A successful worker exit therefore did not prove a
successful command. Core now accepts only the corrected v2 report contract;
even a v1 report claiming VM entry and a settled exit remains indeterminate.

The change and its regression test are byte-identical in the Aster checkout.
No permission, network, verification, or default-backend gate is relaxed.

## Validation

- Before: the legacy-report regression failed because exit 0 was accepted as
  `Exited(0)` instead of `MissingStatus`.
- After: `cargo test --offline --locked -p a3s-code-core --lib sandbox::guest`:
  9 passed, 1 explicitly ignored real-HVF test. The same family passes in
  Aster with 9 passed and 1 ignored.
- Scoped Rust formatting and `git diff --check` passed.
- Strict original-Core Clippy was run and fails with 48 errors in unchanged
  modules, including unused `AgentLoop` imports and an existing range-pattern
  lint. None references the changed guest files. This is not a passing global
  lint result.

## Explicitly not claimed

No usable default macOS Bash backend, successful Core HVF E2E, complete global
Clippy gate, release, push, merge, or CI run. The v2 command producer currently
refuses to claim a settled exit without a guest terminal receipt. The separate
full OCI protocol has passed a real nonzero-exit probe, but its workspace and
policy integration is unfinished. Old v1 artifacts must not be used as proof
of payload success.
