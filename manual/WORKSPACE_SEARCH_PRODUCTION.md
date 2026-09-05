# Workspace Search Production Qualification

`scripts/workspace_search_production.sh` is the repeatable local acceptance
gate for the framework-owned workspace search path. It does not require MCP or
a live model provider. The default profile runs focused native/portable
retrieval tests, strict Clippy, and a release qualification over 512 real
source files so normal development does not wait for unrelated CI suites.

Run it from `crates/code`:

```bash
scripts/workspace_search_production.sh
```

Run the complete Core unit suites only for release admission:

```bash
scripts/workspace_search_production.sh --full
```

The scale profile creates a temporary workspace, starts the real deferred
manifest scanner, activates the normal retrieval services, and waits for the
background zvec generation. It then verifies all of the invariants that matter
for production use:

- every source file is admitted and every chunk is indexed;
- concurrent readers continue to return verified hits while the index is live;
- rewriting identical bytes reuses the current generation;
- changed bytes publish a new generation and become searchable;
- obsolete generations are removed after the atomic `CURRENT` switch; and
- a fresh `WorkspacePersistentIndex` reopens the published generation and
  answers the same query.

The automatic persistent path uses native zvec for the workspace-wide durable
projection. During cold admission, the session catalog uses the portable
scorer as a verified fallback, so the runtime does not open one native
collection per source file. This keeps admission proportional to source bytes;
the model-facing `search` contract remains unchanged and switches to the
durable native postings as soon as that generation is ready.

The command prints one JSON measurement line. `discoveryMs` is the standalone
manifest scan, `admissionMs` is catalog admission through the real runtime,
`buildMs` is the first durable generation, and `queryP50Ms`/`queryP95Ms` are
warm native query samples under concurrent readers. These values are machine
dependent; the acceptance gate asserts correctness and lifecycle invariants,
not a brittle wall-clock threshold.

For a smaller or larger local profile:

```bash
A3S_WORKSPACE_ACCEPTANCE_FILES=1024 \
  A3S_WORKSPACE_ACCEPTANCE_QUERY_WORKERS=16 \
  scripts/workspace_search_production.sh
```

The native feature can be pointed at a verified packaged library with
`A3S_WORKSPACE_FEATURES=zvec-rust-fts` and the normal `ZVEC_LIB_DIR` packaging
configuration. The default bundled feature is intended for local and CI
qualification; release artifacts must still pass the repository's native
library attestation and packaging checks.
