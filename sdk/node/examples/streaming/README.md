# Streaming Examples

Real-time event streaming and focused diagnostics.

Default sessions are queue-free. Lane queue configuration is covered only as
advanced external/hybrid dispatch infrastructure; it is not a public task
parallelism API.

## Files

- `news_radar.ts` - News monitoring demo
- `integration_tests.ts` - Streaming integration tests
- `test_quick.ts` - Minimal streaming smoke test
- `hitl_confirmation_loop.ts` - Streaming Human-in-the-Loop confirmation loop
- `reasoning_delta_test.ts` - Reasoning delta event check

## HITL Confirmation Loop

`hitl_confirmation_loop.ts` demonstrates the event-driven pattern used by a UI:
listen for `confirmation_required`, read the pending confirmation snapshot, then
call `confirmToolUse(toolId, approved, reason)` before the timeout expires.

```bash
A3S_CONFIG_FILE=./agent.acl npx tsx streaming/hitl_confirmation_loop.ts
A3S_CONFIG_FILE=./agent.acl A3S_CODE_HITL_AUTO_APPROVE=1 npx tsx streaming/hitl_confirmation_loop.ts
```
