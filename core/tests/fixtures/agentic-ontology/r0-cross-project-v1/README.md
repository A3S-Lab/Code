# R0 cross-project contract fixtures

This data-only package freezes the first Agentic Ontology -> A3S Use -> A3S
Code -> A3S Cloud integration boundary. It does not grant package lifecycle,
Knowledge, Agent, tenant, publication, or authorization authority.

## Ownership

- Agentic Ontology owns the candidate handoff and this integration projection.
- A3S Use owns the reviewed lock, Registry generation, installed receipt,
  Knowledge capability projection, and exact-generation search/read behavior.
- A3S Code owns the Agent release, session, run, events, result, and task proof.
- A3S Cloud owns organization, Workspace, immutable release, execution, and
  audit identities.

Every runtime must resolve and revalidate the referenced state from its own
authority. A digest in this package is evidence identity, not authority to
perform the referenced operation. No consumer may replace an exact identity
with local `latest` state.

## Canonical fixtures

- `a3s-use-handoff.valid.json` is a real, revision-bound compiler handoff.
- `plugin-package-lock.json`, `registry-snapshot.json`,
  `extension-receipt.json`, and `a3s-use-generation-receipt.json` freeze the
  exact reviewed A3S Use generation evidence.
- `knowledge-lease-binding.json`, `capability-snapshot.json`, and
  `knowledge-citation.json` freeze the minimum Agent-facing OKF search/read and
  citation projection. They expose no internal `GraphProjection` or source
  repository.
- `code-run-identity.json`, `code-session-binding.json`, and
  `code-task-result.json` freeze the exact Code session/run and task proof
  fields.
- `cloud-audit-link.json` connects the candidate, generation, execution, and
  task proof without merging their owners.
- `r0-cross-project.valid.json` is the complete accepted projection.

The `*.drift*.json` and `*.unknown-field.json` files are intentionally invalid.
Consumers must reject handoff digest, lifecycle generation, Code run, Cloud
task proof, and closed-schema drift.

## Digest rules

Agentic Ontology canonical digests use SHA-256 over:

1. `agentic-ontology-canonical-v1\0`;
2. the big-endian `u64` byte length of the ASCII domain and the domain bytes;
3. the big-endian `u64` byte length of compact deterministic JSON and those
   JSON bytes.

A3S Use lock, Registry snapshot, and extension receipt digests use the owning
Use contract's canonical JSON rules. `manifest.json` records the raw SHA-256
and byte length of every other package file. Its `packageDigest` uses the
Agentic Ontology canonical algorithm with domain
`agentic.ontology.r0-contract-fixture-package.v1` over the ordered `files`
array.

All four repositories vendor this directory byte-for-byte and pin the raw
`manifest.json` digest in their conformance test. Changes require a new
versioned package or an explicitly reviewed compatible fixture update across
all four consumers.
