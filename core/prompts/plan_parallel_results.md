The following plan steps completed in parallel. The results are provided as structured JSON in a USER message (not plain text) with this envelope:

```json
{"type":"parallel_results","steps":[{"step_id":"...","step_number":1,"status":"completed","summary":"...","key_findings":[...],"error":null},...]}
```

- `summary`: Brief description of what this step accomplished (in the user's language).
- `key_findings`: Optional array of notable findings or outputs.
- `error`: Present only when `status` is `"failed"`.

Use the structured step data above to continue planning and execution.
