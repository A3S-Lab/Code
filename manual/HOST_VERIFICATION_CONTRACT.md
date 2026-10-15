# TUI host verification contracts

Decision date: 2026-10-12. Status: implementation under qualification; this
document is not a passing live or release certificate.

The actual ACP session builder must receive the same host-declared command
and covered paths that Core uses to bind a passing check to workspace changes.
An SDK-only session option left TUI coding tasks unable to use that contract.
An ordinary command exit 0 does not establish what it verified.

## Host entry and authority

Place a declaration in the explicit host ACL selected with --config/A3S_CONFIG
or in the user's ~/.a3s/config.acl. Parse and generate it with a3s-acl:

```acl
verification_contract "project-tests" {
  workspace = "/absolute/path/to/project"
  command = "python3 gate.py"
  covers = ["src/**", "tests/**"]
}
```

workspace, command and covers are required literal values. The id is one
nonempty label. Unknown attributes, nested blocks, missing values, relative
workspace roots and traversal in covered paths refuse. A declaration can
cover exact relative files or dir/**. Every mutation must be covered; select
paths from the actual project test contract, including generated files whose
correctness the suite establishes. Do not use a broad pattern just to pass.

Platform authority decision: verification_contract joins sandbox and providers
in HOST_AUTHORITATIVE_BLOCKS. This grants the host a new explicit declaration;
it grants no authority to repository content. Automatically discovered
workspace ACL declarations are withheld by the existing trusted-layer choke
point, including when there is no trusted declaration. No SDK/private test
injection or legacy loop ACCEPTANCE file is needed for this entry.

ACP canonicalizes the actual and declared workspaces at session creation and
replacement. Different canonical workspace paths receive no contract. This
selection does not attest directory-inode lifetime; native capability/resource
owners retain their own binding and revocation checks. Missing or
nondirectory declared roots and alias-hidden duplicate ids refuse activation.
Trusted declarations are re-resolved when a session is rebuilt, so model/mode
changes do not drop the host contract. All configured roots must remain
accessible; remove stale declarations rather than silently ignoring them.

## Execution and completion

Core projects the admitted contract ids, exact commands and covered paths into
the initial model system context, including explicit-style requests. Commands
are data from the host manifest, not inferred from user or repository text.
This disclosure explains how to verify; it grants no execution authority.

Only a real host bash result for the exact stored command can acquire this
contract. Outer whitespace is ignored; arguments and shell composition are
part of the stored command. Model-added suffixes, pipelines, || true, later
successful commands, or a quoted mention never inherit verification rights.
To use a different command, the host must explicitly declare that exact
command. This tightens the former prefix/substring matching behavior.

The existing Core binder requires exit 0, complete execution evidence, covered
mutations and the current effect digest. Unknown operations, incomplete
receipts, failed checks and stale reports cannot become Verified. The contract
does not grant execute permission or bypass native sandbox qualification.
The model cannot add this host contract, waive unknown effects, or grant itself
success. /goal manages durable objectives; it is not this declaration.

ACP requires an actual assistant reply after verified tools. A typed host
session option controls this delivery policy; verification alone does not
satisfy a request to quote or explain the result. Evidence-only SDK hosts
retain their existing default. Both modes use the same completion gate.

## Qualification obligations

- Parse valid declarations; refuse malformed/foreign authority and traversal.
- Prove workspace isolation, aliases, duplicates and required-root failures.
- Prove actual ACP options retain the contract and existing permission gates.
- Reject compound status masking, quoted mentions, and test-selection suffixes.
- Through real TUI and LLM, retain unchanged checker, failed then passing
  original receipts, contract report, current effect digest and bound Verified
  completion. Repeat the full mandatory target and SOLO distinct failures.

The multi-turn fixture also intentionally leaves one defect after a failing
check. A contract does not make that checkpoint Verified; test the product's
explicit failure/cancellation/continuation behavior without waiving it or
quietly replacing the fixture with an easier coding task.
