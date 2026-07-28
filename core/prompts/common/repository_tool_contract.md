## Repository Tool Contract

The registered tool schema is authoritative for availability, types, and limits.
Use its canonical argument names exactly; do not invent aliases, cursors, or
continuations.

- `read`: use `file_path` with optional 0-based `offset` and `limit` for one
  file. When several known text files are relevant, prefer one call with
  `files=[{path, offset?, limit?}]` and optional `max_output_bytes`; never send
  `file_path` and `files` together. If `metadata.batch.continuation` is non-empty,
  copy that exact array into the next call's `files` and stop when it is empty.
- `grep`: pass `pattern` and, when useful, `path`, `glob`, `context`, and `-i`.
  Choose the smallest useful `output_mode`: `content` for matching evidence,
  `files_with_matches` for paths, `count` for matching-line counts per file, or
  `summary` for totals only. Only `files_with_matches` and `count` accept
  pagination `limit`/`cursor`; copy `metadata.page.next_cursor` exactly and stop
  when it is absent.
- `glob`: pass `pattern` and optional `path`, `limit`, `cursor`, and `sort`.
  Keep the default `sort: "backend"` when backend relevance or recency matters;
  use `sort: "path"` for deterministic lexical pagination. Copy
  `metadata.page.next_cursor` exactly and stop when it is absent.
- `edit`: pass `file_path`, `old_string`, and `new_string`; set `replace_all`
  only when every occurrence should change. For `replace_all` or any mechanical
  change whose scope is uncertain, first use `dry_run`, inspect the diff and
  replacement count, then apply with that count as `expected_replacements` and
  an appropriate `max_replacements`. On a count mismatch or version conflict,
  re-read and re-preview instead of weakening the guards.

Use dedicated repository tools instead of shell commands for reading, searching,
and editing. Use `bash` for builds, tests, and commands that genuinely require a
shell.
