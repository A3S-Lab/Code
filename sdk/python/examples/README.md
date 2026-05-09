# A3S Code Python SDK Examples

## HITL Confirmation Loop

`hitl_confirmation_loop.py` demonstrates the event-driven Human-in-the-Loop
pattern used by a UI: listen for `confirmation_required`, read the pending
confirmation snapshot, then call `confirm_tool_use(tool_id, approved, reason)`
before the timeout expires.

```bash
A3S_CONFIG_FILE=./agent.acl python examples/hitl_confirmation_loop.py
A3S_CONFIG_FILE=./agent.acl A3S_CODE_HITL_AUTO_APPROVE=1 python examples/hitl_confirmation_loop.py
```
