# macOS Guest owner binding in the Code adapter

Date: 2026-10-18. Producer: OCI Runtime source commit `3189636cbf903aaee01300f541d1af5ee7fc61a0` in the OS worktree.

The adapter passes its process identity to `guest-command --owner-pid`. The shim and VM worker independently watch their direct owners through the existing macOS kernel monitor. The producer's real-HVF gate observed late writes after host/shim death before the fix, then passed all five native cases after it. Its immutable image and exact artifacts are retained in `docs/evidence/code-harness/20261018-macos-guest-owner` in that producer worktree.

The command script now uses exclusive creation. Existing files and symbolic/hard links are refused without modifying their bytes. Its cleanup guard exists before writing, so a write failure retains the normal owned-file cleanup path. Aster carries these same adapter files; its real Guest lifecycle gate passed all seven checks on the fixed, signed producer artifact.

## Verification

- Original Code `cargo test --locked -p a3s-code-core --lib sandbox::guest:: -- --test-threads=1`: 8 passed, 0 failed, 0 ignored. [Raw output](guest-tests.log.gz).
- Original Code strict selected Core Clippy was attempted and failed on 48 existing diagnostics outside the two edited Guest files. [Raw output](strict-clippy.log.gz). No lint or ratchet was disabled.
- [Source identities](source-identities.json) bind the adapter files and paired producer revision.

## Explicitly not claimed

Native macOS command lifetime remains refused. This is not a packaged execution backend, a complete E1/E2 delivery or a release. Protected metadata, stale control-file recovery, mediated network, background streaming, Python/openpyxl and installed-product tasks remain open. The adapter and the new required-owner shim must be deployed together. No SDK binding changes, CI trigger or push were made here.
