# Code Intelligence Design

## Decision

A3S Code provides Code Intelligence as a native, workspace-scoped Core
capability shared by agent tools, the terminal editor, and A3S Web. The Rust
runtime owns language-process discovery, framed stdio JSON-RPC, lifecycle,
capability negotiation, saved-document synchronization, result normalization,
and bounded caching. It does not use MCP or a separate helper service.

The initial language profiles are Rust and TypeScript/JavaScript. The public
contracts use open-ended language identifiers so adding a profile does not
change callers.

## Capability Boundary

Code Intelligence owns only language-semantic operations:

- hierarchical symbols for one document;
- bounded symbol search across a workspace;
- definitions, declarations, references, and implementations;
- document and bounded workspace diagnostics;
- runtime status and negotiated capabilities.

Existing A3S Code capabilities remain the single source of truth for adjacent
work:

| Concern | Existing owner | Code Intelligence behavior |
| --- | --- | --- |
| File discovery and change events | Workspace manifest | Subscribes to the existing manifest and watcher |
| Path validation and file reads | Workspace services | Uses `WorkspacePath` and `WorkspaceFileSystem` |
| Text and filename search | `grep` and `glob` | Returns semantic symbols only |
| Source retrieval | `read` | Returns locations and metadata, not source text |
| Mutations | `write`, `edit`, and `patch` | Exposes no second editing or refactoring tool |
| Context, memory, and sessions | Existing Core runtimes | Stores no parallel project memory or session state |
| Permissions and cancellation | Tool runtime and workspace policy | Uses the same read-only query lane and cancellation tokens |

This boundary lets an agent locate a symbol semantically, inspect it with
`read`, and modify it with the existing compare-and-swap editing tools. It
avoids competing file indexes, mutation paths, or persistence formats.

## Shared Architecture

```text
agent tools       TUI /ide       Web + Monaco
     \                |                /
      +------ WorkspaceServices ------+
                     |
       WorkspaceCodeIntelligence
                     |
          LocalCodeIntelligence
             /               \
shared manifest + FS      runtime registry
                                  |
                    per-language process runtime
                                  |
                    framed stdio language protocol
```

`WorkspaceServices` is the host boundary. Local hosts attach one
`LocalCodeIntelligence` provider to the same `ManifestWorkspaceBackend` used by
file tools and workspace UI. Web caches that bundle by canonical workspace;
sessions for the same workspace reuse it. The TUI owns one bundle for its
active workspace.

The registry key includes the canonical workspace, project layout, and host
isolation scope; the workspace runtime owns its per-language processes.
Concurrent initialization is single-flight. Leases protect active runtimes.
The generic registry supports bounded TTL/LRU reclamation, while the local
provider retires an idle superseded layout immediately. Shutdown is explicit
and idempotent; process exit also changes status immediately and permits a
clean restart on the next query.

The first workspace diagnostics query starts every language profile relevant
to supported source files in the current manifest, then fairly queries a
bounded number of saved documents with bounded concurrency. A missing runtime
does not suppress diagnostics from available languages: the result is marked
`truncated`, the aggregate status becomes degraded, and the unavailable
language remains visible in per-language status.

## Public Contract

- Paths are normalized, workspace-relative `WorkspacePath` values.
- Lines and characters are zero-based.
- Characters count UTF-16 code units. This matches the language protocol and
  Monaco without lossy column conversion.
- Queries operate on saved files only. An editor with unsaved changes must say
  that results are based on the saved version.
- A document-scoped result includes a monotonic revision, an opaque saved
  content hash, and a `stale` flag if the file changed during the query.
- Every result is bounded and reports `truncated` plus the observed workspace
  revision.
- Unsupported server capabilities are reported through typed status and errors
  instead of being guessed from the language name.

Saved-only synchronization prevents two UI sessions or editor tabs from
publishing conflicting in-memory document state. Save events flow through the
shared manifest; a later query resynchronizes the saved contents before asking
the language runtime.

## Language Profiles and Project Layout

The manifest identifies project roots from stable marker topology:

| Profile | Source files | Project markers | Default executable |
| --- | --- | --- | --- |
| Rust | `.rs` | `Cargo.toml` | `rust-analyzer` |
| TypeScript/JavaScript | `.ts`, `.tsx`, `.js`, `.jsx` | `package.json`, `tsconfig*.json` | `typescript-language-server --stdio` |

Only marker topology changes restart a workspace runtime. Ordinary source or
metadata changes keep the existing process and use saved-document events. A
mixed monorepo can run both profiles with deduplicated project folders.

Language executables are discovered from the process environment when a query
first needs that profile. Missing or incompatible executables degrade only that
language profile and remain visible in status; they do not disable file tools
or another working language.

## Product Surfaces

Agent sessions expose three read-only tools when the workspace advertises the
capability:

- `code_symbols` for document outlines and workspace symbol search;
- `code_navigation` for definitions, declarations, references, and
  implementations;
- `code_diagnostics` for document or bounded workspace diagnostics.

The TUI maps the same service to `/ide` commands and a navigable result list.
A3S Web maps it to typed read-only endpoints and Monaco providers/actions. Both
surfaces reuse the existing file-selection flow when opening a returned
location.

## Security and Failure Model

- Canonical path checks reject absolute client paths, traversal, and symlink
  escapes before a file is read or a result is returned.
- Language-process output is treated as untrusted protocol data. Malformed
  frames, invalid URIs, and out-of-workspace locations become typed errors.
- The built-in Rust profile disables workspace build scripts, procedural
  macros, and automatic check commands so a read-only semantic query cannot
  execute workspace code or bypass the existing command-approval path.
- Request timeouts and cancellation remove pending protocol calls. Process
  shutdown is bounded, followed by forced cleanup when necessary.
- Stderr is bounded diagnostic evidence and never mixed with protocol stdout.
- Runtime failure changes only Code Intelligence status; existing workspace
  reads, searches, edits, and sessions remain available.

## Delivery and Verification

1. Core contracts and runtime: protocol codec, initialization, process
   lifecycle, saved-document state, path containment, project layout, and
   registry.
2. Agent integration: capability-gated semantic tools with bounded structured
   output.
3. Product integration: asynchronous TUI commands plus Web endpoints and
   Monaco diagnostics, symbols, and navigation.
4. Hardening: real child-process fixtures, crash/restart and shutdown tests,
   mixed-workspace isolation, dirty-editor messaging, and end-to-end product
   checks.

Required regression coverage includes UTF-16 positions, stale evidence,
unsupported capabilities, process crashes, delayed status subscribers,
workspace isolation, traversal and symlink escapes, bounded results, explicit
shutdown, and the absence of duplicate file/search/edit behavior.
