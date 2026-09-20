# Workspace retrieval cases

Capability: `workspace_retrieval` (WR).

Operations: `session.workspace_retrieval_status`, `session.semantic_search`,
`session.hybrid_search`. Lexical, symbol, and exact search are part of the
same capability even when the tool name is `search` / `grep`.

Closed-world rule: retrieval reads the workspace generation it was built from.
It does not search the network, and it does not keep vectors after close.

## Unit

### U-WR-01

- **Invariant:** exact and lexical search hit a token in a text file and miss it after the file is deleted and the index is refreshed.
- **Pre:** workspace with `NEEDLE` in `a.txt` only.
- **Stimulus:** search, delete, refresh, search.
- **Oracle:** first hit path is `a.txt`; second hit set is empty.
- **Fail:** a stale hit after refresh.
- **Home:** `core/src/workspace/retrieval/lexical.rs`, `core/src/tools/builtin/grep.rs`, `bm25`.

### U-WR-02

- **Invariant:** binary and non-text inputs are not embedded.
- **Pre:** a `.png` and a `.txt` with the same bytes interpreted as text only in the txt.
- **Stimulus:** index the tree.
- **Oracle:** status shows the png skipped; vector count equals the text chunks only.
- **Fail:** a vector whose source path is the png.
- **Home:** retrieval tests under `core/src/workspace/retrieval/tests/`.

### U-WR-03

- **Invariant:** semantic query returns chunks from the current generation only.
- **Pre:** generation 1 with token A; generation 2 replaces the file with token B.
- **Stimulus:** semantic search for A, then for B, after publication of generation 2.
- **Oracle:** A misses or is marked stale; B hits; status generation is 2.
- **Fail:** A still returned as current.
- **Home:** `core/src/workspace/retrieval/tests/semantic.rs`, `lifecycle.rs`.

### U-WR-04

- **Invariant:** hybrid search does not return a path outside the workspace, including via symlink.
- **Pre:** symlink to an outside file containing `NEEDLE`.
- **Stimulus:** hybrid and lexical search.
- **Oracle:** no hit whose canonical path is outside the root.
- **Fail:** outside path in results.
- **Home:** `read_text_does_not_return_bytes_through_a_symlink` and `grep_and_glob_do_not_surface_a_symlink_target` in `core/src/workspace/local.rs` (`#[cfg(unix)]`). Retrieval reconcile reads through that filesystem, so a symlink is not indexed as current text. The Windows cargo-test process cannot create the symlink fixture; these tests were not executed on this machine.

### U-WR-05

- **Invariant:** status while the index is building is not "ready", and search during build returns a typed not-ready or a documented partial flag. It does not block session creation.
- **Pre:** large fixture or a blocked publisher.
- **Stimulus:** create session, read status, search.
- **Oracle:** session exists; status is loading or not-ready; search does not panic.
- **Fail:** `register_builtins` opens the native index and stalls session build. The source comment in `builtin/mod.rs` says registration must not open native indexes.
- **Home:** `core/src/tools/builtin/mod.rs` `register_builtins_does_not_open_durable_a3s_vec`. `SearchTool::new` is reached through `register_builtins`; the durable index stays unopened and `.a3s-code/index` is not created.

### U-WR-06

- **Invariant:** chunker bounds are stable: a file over the chunk cap produces multiple chunks, each ≤ cap, and the concatenation covers the file without overlap beyond the documented overlap.
- **Pre:** file of known length.
- **Stimulus:** chunk.
- **Oracle:** each chunk length ≤ cap; coverage complete; overlap equals the configured window.
- **Fail:** a chunk over cap, or dropped tail.
- **Home:** `core/src/workspace/retrieval/chunk.rs`.

### U-WR-07

- **Invariant:** closing the session drops the vector store. A new session does not see the previous process's vectors unless it reindexed.
- **Pre:** indexed session.
- **Stimulus:** close; open a new session on a copy of the workspace that has not been indexed.
- **Oracle:** status not-ready or empty; no hit until index. On the same store, close releases the lock so the next open can acquire it.
- **Fail:** lock held after close, or vectors readable with no index.
- **Home:** `core/src/workspace/retrieval/tests/lifecycle.rs`; a3s-vec adapter in `a3s_vec.rs`.

## Integration

### I-WR-01

- **Invariant:** `search` tool with semantic enabled calls the retrieval runtime and records the generation id on the tool result.
- **Pre:** hermetic in-memory vector adapter.
- **Stimulus:** governed semantic search.
- **Oracle:** hits cite path + generation; generation equals status.
- **Fail:** hits with no generation, so the model cannot tell stale from current.
- **Home:** `core/src/workspace/retrieval/memory_vector_adapter.rs`, `semantic_query.rs`.

### I-WR-02

- **Invariant:** an edit through the governed `write` tool advances the retrieval generation. Search after the edit does not return the old line as current.
- **Pre:** indexed file; write replaces the line.
- **Stimulus:** write, wait for publication or call the documented refresh, search.
- **Oracle:** new line hits; old line does not hit as current.
- **Fail:** old line still current after the session's own write.
- **Home:** `write_then_search_drops_the_replaced_line` in `core/src/workspace/retrieval/tests/lifecycle.rs`. `ManifestWorkspaceBackend::write_text` replaces `OLD-LINE-91`. Grep match count for the old line is 0 and the new line is at least 1. The catalog runtime, fed by the manifest watcher rather than a direct `replace_file` call, then publishes `NEW-LINE-91`, drops the old chunk, and advances `revision`.

### I-WR-03

- **Invariant:** Node, Python, and Go retrieval fixtures agree on hit paths for the same corpus and the same adapter.
- **Pre:** shared fixture corpus.
- **Stimulus:** each SDK's workspace retrieval test.
- **Oracle:** same path set; no SDK enables headless by default.
- **Fail:** one language returns an extra path.
- **Home:** `sdk/node/test_workspace_retrieval.mjs`, `sdk/python/tests/test_workspace_retrieval.py`, `sdk/go/workspace_retrieval_test.go`.

### I-WR-L1

- **Invariant:** live retrieval against the Flash pin cites a file that exists in the fixture workspace, and the outer budget is 420s without weakening the citation assert.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** `test_workspace_retrieval_real_llm` and `test_workspace_search_real_llm`.
- **Oracle:** cited path exists; cited span contains the expected token; run terminal.
- **Fail:** pass on a plausible filename that is not in the tree.
- **Home:** those two tests. Fixture answer files carry semantic claims matching the query; query-echo distractors remain. `real_deepseek_chunking_strategy_matrix_qualifies_builtins_and_audits_custom_control` passed 2026-09-20 under remapped `boyue/bailian/deepseek-v4.1-flash` (~350s): line / fixed_window_512_64 / recursive all `qualityGatePassed=true` with `taskAccuracy=1.0` (oracle unchanged). Live twin `real_model_selects_bm25_and_uses_native_workspace_index` passed 2026-09-20 under remapped Flash (~14s).

### I-WR-04

- **Invariant:** remote-git workspace materializes a revision and searches that revision, not the host's dirty tree.
- **Pre:** fixture remote with two commits.
- **Stimulus:** bind commit 1, search a token that exists only in commit 2.
- **Oracle:** miss; status revision is commit 1.
- **Fail:** commit 2 token returned.
- **Home:** `core/src/workspace/remote_git/tests.rs`.

## What this capability does not prove

- Production Recall@k on a real embedding model. That is the external
  qualification in `manual/WORKSPACE_RETRIEVAL_QA.md`, not a unit oracle.
- Headless page rendering. That is `moli_runtime`.
- S3-backed search. That is `s3_workspace`.
