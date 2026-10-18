# Shipped TUI case inventory

The interactive session is `a3s-code-tui` speaking ACP to `a3s-code-acp`.
Both binaries must report the current ACP package version. Hermetic rows use a local OpenAI-compatible
server and do not call a live provider. The real-LLM row is a separate
test binary so a missing provider cannot hide a launch failure.

| Case | Test | Binary |
| --- | --- | --- |
| launch | `composer_opens_on_the_acl_default_model` | `tui_913_hermetic` |
| model selection | `model_command_selects_the_catalog_model` | `tui_913_hermetic` |
| one conversational turn | `one_turn_shows_assistant_text` | `tui_913_hermetic` |
| missing-provider refusal | `missing_provider_refuses_without_an_invented_reply` | `tui_913_hermetic` |
| 轮次 | `three_turns_fix_the_bug_they_diagnosed` | `tui_913_hermetic` |
| 长程任务 | `long_task_repairs_the_check_after_a_failure` | `tui_913_hermetic` |
| one real-LLM turn | `real_llm_tui_turn_shows_assistant_text` | `tui_913_real_llm` |
| real-LLM 轮次 | `real_llm_tui_turns_fix_the_diagnosed_bug` | `tui_913_real_llm` |
| real-LLM 长程任务 | `real_llm_tui_long_task_repairs_and_quotes_the_check` | `tui_913_real_llm` |
| Cursor account text and picker | `picker_shows_the_cursor_account_and_a_turn_returns_stub_text` | `cursor_account_tui_e2e` |
| Cursor real model read/write and host byte verification | `live_cursor_tools_read_write_and_verify_actual_workspace_bytes` (explicit `--ignored`; fixture-only disk content attestor) | `cursor_account_tui_e2e` |
| Cursor real TUI reads, writes and refuses unverified completion | `tests/e2e/cursor_tools.acl` (prepared isolated Cursor config/workspace; assert output bytes after run) | `a3s-test` |

`shipped_binaries_report_current_version` in the hermetic binary checks the two
version lines. The real-LLM tests run only when `A3S_CONFIG_FILE` is
set; they are not `#[ignore]`. Each points `A3S_CONFIG` at that same file
and does not rewrite it.

The one-turn real-LLM test (`real_llm_tui_turn_shows_assistant_text`) is
non-empty assistant text. It is not the 轮次 gate. The one-defect
subtraction task (`calc.py` `return a - b`, token `SUM-OK-9`) is retired
as the gate.

轮次 keeps one pager session. `rate.py` and `fee.py` start with two
independent defects. The diagnosis names both and edits nothing. The next
prompt does not restate either defect and leaves exactly one; `gate.py`
still prints `DUAL-HOLD`. A later prompt does not restate the remaining
defect and clears both. The last prompt quotes the checker line. That
prompt does not contain `DUAL-GATE-7K`. A screen line that contains the
token is not enough, and a new `runs.txt` line is not enough by itself.
The quote wait does not read the screen. It ends only when `runs.txt`
gains a line equal to `DUAL-GATE-7K` and the session tool trace shows a
`python3 gate.py` command whose stdout has a line equal to that token.
A screen sentence, a `runs.txt` sentence that only contains the token,
a hand-written append, and a run that sets `GATE_SKIP_LOG` do not count. `gate.py` does not mention
`GATE_SKIP_LOG`. The test's own checker runs in a private directory and
does not write the session `runs.txt`. The cargo log prints `runs.txt`
before the success token, then prints the bash call id and that traced
command. The call id is not in the user prompt. The log also prints the
tool stdout line. It does not print the model's surrounding sentence.
The screen shows that token
only after a model request in the session already carried `DUAL-HOLD`.
`gate.py` bytes stay unchanged. Fixing only one defect, or swapping one
operator, still fails the checker.

长程任务 is one prompt. It contains neither `DUAL-GATE-7K` nor the defect
text. The session must see the checker fail, correct one file, see the
checker still fail, correct the other file, and quote `DUAL-GATE-7K`.
The hermetic run makes at least 6 model requests. A request made while
exactly one defect remains still contains `DUAL-HOLD`, and that output
appears before any request contains `DUAL-GATE-7K`. Each real `python3
gate.py` execution appends its printed line to `runs.txt`. The long-task
order oracle is that file: at least two `DUAL-HOLD` lines before the
first `DUAL-GATE-7K`, and one of those `DUAL-HOLD` lines must be appended
while exactly one defect remains. Two failing runs before any edit do
not satisfy that. The test's own checker runs in a private directory and
does not write that file. The checker source does not mention
`GATE_SKIP_LOG`. The 轮次 and 长程任务 tests panic when
`A3S_CONFIG_FILE` is unset. The one-turn test still returns early.

The CLI crate pins Core 9.1.4 (`apps/cli`). Its release pager and ACP use the
same publicly fetchable Code revision; local development builds can be newer.
