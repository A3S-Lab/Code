# Durable engineering: work policy and bound goals

Spec 018 adds two session capabilities to the Meta Harness
([META_HARNESS.md](META_HARNESS.md)) without a second runtime:

- a **work policy**: one host-selected instruction body, recorded once in the
  session's fact log and appended to the original `guidelines` prompt slot;
- a **bound goal**: a typed goal definition and plan recorded in the same fact
  log, advanced one original Run (a "slice") at a time.

Neither adds a scheduler, queue, ledger, worker, benchmark engine or second
progress authority. Permission projection, the completion gate and the original
Run owner/writer stay in charge. Evidence:
[M1 OUTCOME](../../../docs/evidence/code-harness/20261013-durable-engineering-m1/OUTCOME.md).

## Work policy

| Surface | Selection |
| --- | --- |
| Rust | `SessionOptions::with_work_policy(WorkPolicyV1::new(id, version, body)?)` |
| Node / Python / Go | the same field on the original session options |
| ACP | session `_meta.a3sEngineering = { "policy": "engineering-v1" }` (closed set; the body is `acp/prompts/engineering.md`) |

ACP and Code TUI new sessions select `engineering-v1` by default. A client can
create a plain session with `_meta.a3sEngineering = { "policy": "none" }`.
Unknown names or malformed selections refuse instead of falling back. Core
and SDK session defaults stay optional. Loading an old session does not add
a policy; an already-bound session keeps its original policy.

`--unattended` also selects engineering and goal continuation by default.
`--engineering-policy none` and `--no-goal-continuation` opt out separately.

- Building, resuming or replacing a session with a policy admits it as one
  fact before any Run; `work_policy_digest()` returns it at once. A session
  without a policy writes nothing. A reopened session keeps the recorded policy
  even when the host stops selecting one. A different policy, a legacy session
  gaining one, or a corrupt source refuses at session build
  (`work_policy.conflict`, `work_policy.legacy_rebind`, `work_policy.corrupt`).
- `engineering-v1` guides: smallest complete change, read callers first, reuse,
  root-cause fixes, protected checks unchanged, verify with the existing check,
  delegate review/audit to the original `review` worker (narrowed to read-only
  under a policy), read-only deferral scans.
- Review drafts are admitted with `admit_session_review_drafts` (ACP
  `a3s/review/admit`) only for the current policy, recomputed target bytes and,
  optionally, the current goal step and slice Run. Findings never accept a step.
- A goal-step Finding's subject digests the target and that scope.
  `a3s/review/settle` (address, accept, waive) recomputes the subject from the
  target digest and scope the host presents and re-checks a goal step is still
  current; another target or scope, or a replanned step, refuses.

## Bound goals

All commands are typed V1 DTOs carrying `session_id`, `workspace`,
`work_policy_digest` and the expected goal/plan revisions and goal sequence.
Stale references, another policy, an unsettled slice or a terminal goal refuse
with zero facts and zero model calls. A definition or plan change while a
slice's Run executes is refused with `SessionBusy` and writes nothing; resubmit
after the slice settles and the next slice uses the new revision.

| Facade (`AgentSession`) | Effect |
| --- | --- |
| `apply_goal_definition` | Create / Revise. Never starts a Run. |
| `apply_goal_plan` | ProposePlan, then AcceptPlan with `proposal_sequence`. |
| `apply_goal_wait` | Declare / observe a typed step wait (question, confirmation, source revision, UTC due). |
| `control_goal(cmd, grace)` | Pause / Resume / Cancel. Pause and Cancel first validate, then stop a running slice through the original RunControl, then record. A refused control cancels nothing. |
| `preview_goal_tick` | Point-in-time decision; no authority. |
| `tick_goal` / `stream_goal_tick` | Admit at most one original Run named by the operation id, or return a decision: Advance, Wait, NeedsInput, Paused, BudgetExhausted, Reconcile, Complete. |
| `reconcile_goal_slices` | Close slices whose owner is gone, without replay. Needs no host bounds. |
| `goal_definition`, `work_policy_digest`, `check_goal_step_run` | Read-only views. |

ACP binds `/goal status|pause|resume|clear` and `a3s/goal/{definition,plan,control,wait,state,import,import/preview}`
to this facade; `/goal resume` streams successive slices through the turn path
while the trusted host policy returns Advance. Each tick admits at most one Run.
The SDKs expose the same methods once.

`/goal <objective>` in a session without an existing goal now binds that exact
user objective through the same Core goal/plan admission used by unattended
tasks, then starts the bounded wake. Ordinary messages create no goal. Existing
legacy session goals keep their old path; workspace goals still require explicit
import. A cancelled or completed bound goal retains its history; use `/new` for
a different goal or ordinary work.

- A step is accepted only by the original step judgment over this Run's own
  evidence; the goal closes (`a3s.goal.completed`) only when every step is
  accepted, no wait is open and every slice is settled.
- Every slice closes once: by its owner (completed with governed usage, else
  interrupted with usage unknown) or by an explicit reconcile.

## Automatic continuation

ACP and Code TUI supply finite host defaults when trusted ACL declares no
`goal_control`: at most 8 slices, at most 2 successive slices without accepted
progress, and a deadline 24 hours after the Core host initializes. Counts come
from the original goal history, including after reopen; pause/resume does not
reset them. A trusted declaration, including `automatic_enabled = false`, takes
precedence. A long-lived host whose window expired must restart or receive a
new explicit trusted policy. Core and SDK hosts still require their own policy.

A trusted host can select `goal_control`
([HOST_GOAL_CONTROL.md](HOST_GOAL_CONTROL.md)): slice lifetime, no-progress
bound and an absolute deadline. A newly admitted policy allows at most 1000
slices and a deadline no more than 30 days past the host clock; replaying
recorded facts applies neither cap. Workspace ACL cannot enable or raise it. Ticks
are still explicit host calls; no driver runs in the background.

## Decisions

**Product default: authorized by the user on 2026-10-18.**
- ACP/TUI new sessions select the engineering policy. Explicit goals wake the
  original bounded execution chain; no background driver is installed.
- This product choice does not close native/independent acceptance gates or
  claim a measured quality/efficiency benefit. T039 remains separate work.
- The descriptive 2×2 experiment (T030) found no benefit, non-regression or
  regression signal that survives run-to-run variance.
- On this host every check run is refused (`native_process_lifetime_unqualified`),
  so strict completion is 0 in every arm. Positives stay pending native
  qualification (HC-ACCEPT1, HC-EVAL1).

**Automatic goal mode: host-bounded, inside the turn that woke it.**
- `/goal resume` keeps admitting the next original Run while the trusted
  host policy's next decision is Advance. It stops on Complete, NeedsInput,
  Wait, Paused, BudgetExhausted, or Reconcile. No background driver is added.

**Disable and rollback:**
- To disable engineering on a new ACP session, select policy `none`. To disable
  goal continuation, declare trusted `goal_control` with `automatic_enabled = false`
  and all required finite fields. In unattended launches, use the opt-out flags.
- An already-bound session keeps its recorded policy and goal. It can still be
  paused, cancelled (history kept) and reconciled. Its legacy goal state stays
  refused while the goal is bound.
- Rollback is a new session. An old reader refuses the added facts instead of
  downgrading them in place.

## Verification

| Layer | Command (from `apps/code`) |
| --- | --- |
| Hermetic | `cargo test -p a3s-code-core --lib goal` and `--lib durable_engineering` (four modes and combined counterexamples) |
| Real model (mandatory) | Layer C `test_engineering_discipline_real_llm`, `test_durable_goal_real_llm` |
| Real model (measurement) | Layer C `test_durable_engineering_paired_real_llm` (preregistered; never a release gate) |
| SDK | `node scripts/sdk_api_alignment_check.mjs`, Node/Python/Go suites |

## Non-goals

- A goal engine, workflow, StateGraph or background driver beside the fact log.
- Policy text deciding what the user means; it guides the model, never parses intent.
- Running model-edited code outside the qualified tool boundary.
- A benefit claim before native qualification and repeated paired runs.
