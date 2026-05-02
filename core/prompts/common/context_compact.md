Summarise the conversation below into a compact context block. The summary will
replace the older messages so the agent can continue working with full awareness
of what has already happened.

Requirements:
- Keep the summary under 600 words.
- Preserve the **original user goal** verbatim in a "Goal" section.
- List every file that was **read, written, or modified** with a one-line note on
  what changed.
- List every **shell command** that was run and its outcome (success / error / output
  snippet if important).
- Note any **errors or failures** encountered and how they were resolved (or that
  they are still open).
- Note the **current state**: what has been completed and what still needs to be done.
- Do NOT include raw file contents or full command output — only summaries.

Output format:

## Goal
<original user request>

## Files Touched
- `path/to/file` — <what was done>

## Commands Run
- `command` → <outcome>

## Errors / Issues
- <error> → <resolution or status>

## Current State
<what is done> / <what remains>

---

Conversation:
{conversation}
