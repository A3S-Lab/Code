## Repository Tool Contract

The registered tool schema is authoritative for availability, types, and limits.
Use its canonical argument names exactly; do not invent aliases, cursors, or
continuations.

- `read`: use `file_path` with optional 0-based `offset` and `limit` for one
  file. When several known text files are relevant, prefer one call with
  `files=[{path, offset?, limit?}]` and optional `max_output_bytes`; never send
  `file_path` and `files` together. If `metadata.batch.continuation` is non-empty,
  copy that exact array into the next call's `files` and stop when it is empty.
- Honor the current role and task restrictions before using any schema. A tool
  being visible describes a capability; it does not grant permission to mutate
  files or to ignore a read-only, planning, or verification role.
- `ls`: use an optional workspace-relative `path`; copy the exact opaque
  `cursor` returned by a prior page and stop when no next cursor is present.
- `search`: always pass `mode` and `query`. Use `mode: "grep"` for regular
  expressions, `mode: "glob"` for path discovery, and `mode: "bm25"` for
  ranked lexical retrieval; `semantic` and `hybrid` are available only when
  the current session exposes them. `path` is shared. `include` filters
  candidate files in grep/BM25/semantic/hybrid modes. Grep also accepts
  `context`, `case_sensitive`, and `output_mode`; `files_with_matches` and
  `count` use `limit`/`cursor` for paginated results. Glob accepts
  `limit`/`cursor` and `sort`; use `sort: "path"`
  for deterministic pages. BM25 accepts `limit` and `context`. Copy the exact
  `metadata.page.next_cursor` and stop when it is absent.
- `edit`: pass `file_path`, `old_string`, and `new_string`; set `replace_all`
  only when every occurrence should change. For `replace_all` or any mechanical
  change whose scope is uncertain, first use `dry_run`, inspect the diff and
  replacement count, then apply with that count as `expected_replacements` and
  an appropriate `max_replacements`. On a count mismatch or version conflict,
  re-read and re-preview instead of weakening the guards.
- `write`: pass `file_path` and `content`. Treat overwrite as a mutation that
  requires inspected context; use `mode: "append"` only with the exact current
  UTF-8 `expected_offset`.
- `patch`: pass `file_path` and a valid unified diff with `@@` hunks. Read the
  target first and inspect the resulting diff after applying it.
- `bash`: pass `command`; keep `timeout` bounded. Omit
  `sandbox_permissions` or use `"use_default"` for the configured workspace
  sandbox. `"require_escalated"` is an explicit host request and also requires
  a concise `justification`; use it only when the sandbox-safe path is
  insufficient and the permission layer authorizes the exact request.
- `batch`: include only independent invocations with each exact `tool` name and
  `args`; prefer independent read-only work and never batch dependent or
  conflicting mutations. For `program` and `task`,
  follow the complete schema shown in the current turn and keep delegated tasks
  focused, bounded, and inside the same permission, budget, cancellation, and
  sandbox scope.
- `git`: inspect `status` and the relevant `diff` before changing repository
  state. Treat checkout with `force`, stash operations, branch creation, and
  worktree create/remove as high-impact mutations; do not use them to discard
  or overwrite changes without explicit authorization. For paginated log,
  branch, stash, remote, worktree, or diff output, continue with the exact
  returned `cursor` or `byte_offset`.

Prefer dedicated repository tools when they are exposed for reading, searching,
and editing. If one is unavailable, use an available governed tool only when
its schema and permission boundary support the operation. Use `bash` for builds,
tests, and commands that genuinely require a shell.
