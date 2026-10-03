# Deep end-to-end receipts

Observed on 2026-10-03 while `HEAD` was `9fb1cb7985226c7971ef59a20710b46391af2f2b`.
The served model was `deepseek-v4.1-flash`, taken from the completion's response model.
The pinned route was `boyue/bailian/deepseek-v4.1-flash`.
One completion ran before the kernel cases. Its text is not an oracle.
Each row is the kernel effect for that case id on the published tree.
The `run_observability` replay check is in this receipt's commit: an identical cursor is classified before a new chain digest is bound. That check was in the working tree during this run and is not present in `9fb1cb79` alone.
Enterprise GA stays withheld. Terminal-Bench was not run.
The case specification was not edited.

```text
DEEP_E2E_ROW id=agent_runtime commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=SessionClosed
DEEP_E2E_ROW id=conversation commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=RunIdentityConflict
DEEP_E2E_ROW id=run_control commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=StaleTurn
DEEP_E2E_ROW id=governed_tools commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=SessionClosed
DEEP_E2E_ROW id=workspace_tools commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=VersionConflict
DEEP_E2E_ROW id=workspace_retrieval commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=InvalidQuery
DEEP_E2E_ROW id=model_adapters commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=openai-402
DEEP_E2E_ROW id=structured_output commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=schema-rejected
DEEP_E2E_ROW id=mcp_and_skills commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=SessionClosed
DEEP_E2E_ROW id=planning_delegation commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=InvalidArgument
DEEP_E2E_ROW id=priority_scheduling commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=Closed
DEEP_E2E_ROW id=persistence commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=snapshot-digest
DEEP_E2E_ROW id=governance commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=Session
DEEP_E2E_ROW id=run_observability commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=CursorConflict
DEEP_E2E_ROW id=context_memory commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=candidate-related-absent
DEEP_E2E_ROW id=web_search commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=EngineFailure-tavily
DEEP_E2E_ROW id=web_fetch commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=SafeHttpError
DEEP_E2E_ROW id=code_intelligence commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=status-not-published
DEEP_E2E_ROW id=cognitive_packages commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=Conflict
DEEP_E2E_ROW id=use_runtime_tasks commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=ResponseDrift
DEEP_E2E_ROW id=program commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=script-timeout
DEEP_E2E_ROW id=programmable_workflows commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=cached-a
DEEP_E2E_ROW id=state_graph commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=PatchRejected
DEEP_E2E_ROW id=agent_release_contract commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=IncompatibleProtocol
DEEP_E2E_ROW id=agent_protocol commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=IdentityMismatch
DEEP_E2E_ROW id=evaluation_substrate commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=Conflict
DEEP_E2E_ROW id=typed_decisions commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=CannotReplace
DEEP_E2E_ROW id=moli_runtime commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=install-lock-denied
DEEP_E2E_ROW id=s3_workspace commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=VersionConflict
DEEP_E2E_ROW id=opentelemetry commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=shutdown-empty-slot
DEEP_E2E_ROW id=Effect isolation / orphan `.a3s-isolate-*` commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=PromoteOutcome-Conflict
DEEP_E2E_ROW id=Native sandbox and process-host opt-in commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=sandbox-denied
DEEP_E2E_ROW id=Safe HTTP / Fake-IP / SSRF commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=SafeHttpError
DEEP_E2E_ROW id=Mutation verify gate commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=Incomplete-then-Verified
DEEP_E2E_ROW id=`batch` schema pin (no application `$ref` in `examples`) commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=maxItems-no-examples
DEEP_E2E_ROW id=Image `read` attachments commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=image-png
DEEP_E2E_ROW id=Event envelope and oversized projection commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=InvalidField-event
DEEP_E2E_ROW id=research wire commit=9fb1cb7985226c7971ef59a20710b46391af2f2b model=deepseek-v4.1-flash oracle=payload-digest
```
