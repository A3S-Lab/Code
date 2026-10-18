# 9.1.4 runtime release qualification — 2026-10-18

## Result and scope

The user requested finishing the new release after the Core/ACP repair and
terminal default-activation work. This change completes the missing delivery
path for the documented 9.1.4 runtime scope: Core, ACP and the actual pager.
The SDKs remain aligned but are published only through an explicit workflow
option. The formal benefit evaluation is separate and has not been started.

The Release workflow now supports qualification-only dispatch, preserves all
existing qualification jobs, verifies Core and ACP registry packages together,
and builds version-matched terminal archives for five targets. Every archive
contains both ACP and the pager; each has a SHA-256 sidecar, and the final
release also has SHA256SUMS. All qualification and binary builds must pass
before any registry upload. Core uploads first, ACP second, and a failed or
unverifiable upload fails the delivery. Publishing uses the existing repository
CI secret; no local credential or trusted ACL was committed.

The existing pinned pager dependency preparation is shared by debug product
E2E and release builds. POSIX archives include the existing startup/revision
checker result. The final GitHub release is published only after both crates
and all terminal archives are ready. SDK qualification jobs remain mandatory;
SDK uploads are optional, matching the existing 9.1.4 scope.

## Local verification

- Version consistency: all release members match 9.1.4.
- Registry dependency preflight: Apofasi 0.1.1, Flow 1.1.0, Memory 0.1.5,
  Sandbox 0.2.3 and Search 3.4.0 are available.
- Final production source d6bac5d8: combined Core/ACP cargo publish dry-run
  passed, including compilation of ACP against the packaged Core. Actual
  uploads were aborted by Cargo's dry-run mode as intended.
- The ACP binary produced by that package verification completed one real
  DeepSeek Flash turn using the user's explicitly selected trusted ACL. It
  streamed READY and returned end_turn in 7.43 seconds. The binary SHA-256
  was e22c33ad27c6d20351f4baddcb9aa1c01508352de5939feabac01dd40d567c6c
  before and after the smoke. The harness source was unchanged.
- Seven new subprocess upload tests cover upload order, failed Core/ACP,
  resumable existing versions, absent credentials, invalid/yanked responses,
  registry errors and network failures. The runner supplies its own Python
  executable to subprocesses; the initial local system-Python 3.9 failure
  was corrected, and the family passed.
- All 52 validation-script tests passed without skips using the installed
  adapter test interpreter; six selected release/standalone contract tests
  also passed. The locked combined package dry-run passed. actionlint 1.7.12
  validated the changed workflow; its official archive checksum was verified.
  Shell syntax, workspace formatting and patch whitespace checks passed.
- This change edits delivery scripts, tests and documentation only. Core,
  ACP and pager production Rust files retain the already qualified d6bac5d8
  source and its strict workspace Clippy/product E2E evidence.

## Hosted qualification and upload

Pending. A main-branch dispatch with publish=false must pass before creating
v9.1.4. The stable tag reruns the same gates and enables the two registry
uploads. This record will be extended with the actual workflow and delivery
results; a pushed source commit is not an uploaded release.

## Explicitly not claimed

No registry upload, published v9.1.4 release, completed hosted binary matrix,
SDK publication, formal benchmark result or macOS native-sandbox qualification
is claimed at this preparation point. Existing sandbox and historical landing
limitations are not waived. The existing macOS unwind linker warning remains.
No issue comments were posted and no issue state was changed.
