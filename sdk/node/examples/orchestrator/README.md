# Advanced SubAgent Control-Plane Examples

These examples cover explicit SubAgent lifecycle control: spawning,
pause/resume/cancel, and event monitoring.

Routine model-visible delegation should use `task` / `parallel_task`.
External/hybrid lane dispatch is covered by session queue examples, not by the
SubAgent control-plane path.

## Files

- `test_real_kimi.ts` - Real Kimi-backed SubAgent monitoring example
- `test_issue18_event_streaming.mjs` - Issue #18 event streaming
