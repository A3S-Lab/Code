# Deep end-to-end receipts

Observed on 2026-10-03 on a clean checkout of `5f5528f42595d0747515c79607cd0fa1e7d1cb5b`.
`git status --porcelain` was empty before `cargo test` and empty after it.
Each case performed its own completion on the pinned route `boyue/bailian/deepseek-v4.1-flash` before that case's kernel oracle.
The served model on every row is `deepseek-v4.1-flash`, taken from that completion's response model.
The completion text is not an oracle.
The run printed 38 `DEEP_E2E_COMPLETION` lines and the 38 rows below, then `test result: ok` in 107.35s (`TEST_EXIT:0`).
`web_fetch` records `ToolOutput-FetchFailure`: the private-address failure is the tool output, and the blocked page body is absent.
Image read records `ToolOutput-ranged-image`: the ranged read returns the offset/limit tool error, and the text path has no image attachment.
Native sandbox records `UnavailableDefaultSandbox`: the command does not run on the process host.
`moli_runtime` records `install-lock-timeout`: at the deadline, `acquire_install_lock` returns the install-lock timeout and does not return the lock file.
`run_observability` replays an identical cursor and records `CursorConflict` for a different payload. That classification is in this commit.
GitHub's ci-all library job skips `deep_e2e_live_receipts` because runners have no provider ACL. A ci-all run without that ACL panics `acl unreadable` instead of printing rows. This clean-tree log is the receipt.
Re-running the test on a later commit names that later HEAD.
Enterprise GA stays withheld. Terminal-Bench was not run.
The case specification was not edited.

```text
DEEP_E2E_ROW id=agent_runtime commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=SessionClosed
DEEP_E2E_ROW id=conversation commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=RunIdentityConflict
DEEP_E2E_ROW id=run_control commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=StaleTurn
DEEP_E2E_ROW id=governed_tools commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=SessionClosed
DEEP_E2E_ROW id=workspace_tools commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=VersionConflict
DEEP_E2E_ROW id=workspace_retrieval commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=InvalidQuery
DEEP_E2E_ROW id=model_adapters commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=openai-402
DEEP_E2E_ROW id=structured_output commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=schema-rejected
DEEP_E2E_ROW id=mcp_and_skills commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=SessionClosed
DEEP_E2E_ROW id=planning_delegation commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=InvalidArgument
DEEP_E2E_ROW id=priority_scheduling commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=Closed
DEEP_E2E_ROW id=persistence commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=snapshot-digest
DEEP_E2E_ROW id=governance commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=Session
DEEP_E2E_ROW id=run_observability commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=CursorConflict
DEEP_E2E_ROW id=context_memory commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=candidate-related-absent
DEEP_E2E_ROW id=web_search commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=EngineFailure-tavily
DEEP_E2E_ROW id=web_fetch commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=ToolOutput-FetchFailure
DEEP_E2E_ROW id=code_intelligence commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=status-not-published
DEEP_E2E_ROW id=cognitive_packages commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=Conflict
DEEP_E2E_ROW id=use_runtime_tasks commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=ResponseDrift
DEEP_E2E_ROW id=program commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=script-timeout
DEEP_E2E_ROW id=programmable_workflows commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=cached-a
DEEP_E2E_ROW id=state_graph commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=PatchRejected
DEEP_E2E_ROW id=agent_release_contract commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=IncompatibleProtocol
DEEP_E2E_ROW id=agent_protocol commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=IdentityMismatch
DEEP_E2E_ROW id=evaluation_substrate commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=Conflict
DEEP_E2E_ROW id=typed_decisions commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=CannotReplace
DEEP_E2E_ROW id=moli_runtime commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=install-lock-timeout
DEEP_E2E_ROW id=s3_workspace commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=VersionConflict
DEEP_E2E_ROW id=opentelemetry commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=shutdown-empty-slot
DEEP_E2E_ROW id=Effect isolation / orphan `.a3s-isolate-*` commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=PromoteOutcome-Conflict
DEEP_E2E_ROW id=Native sandbox and process-host opt-in commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=UnavailableDefaultSandbox
DEEP_E2E_ROW id=Safe HTTP / Fake-IP / SSRF commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=SafeHttpError
DEEP_E2E_ROW id=Mutation verify gate commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=Incomplete-then-Verified
DEEP_E2E_ROW id=`batch` schema pin (no application `$ref` in `examples`) commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=maxItems-no-examples
DEEP_E2E_ROW id=Image `read` attachments commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=ToolOutput-ranged-image
DEEP_E2E_ROW id=Event envelope and oversized projection commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=InvalidField-event
DEEP_E2E_ROW id=research wire commit=5f5528f42595d0747515c79607cd0fa1e7d1cb5b model=deepseek-v4.1-flash oracle=payload-digest
```
