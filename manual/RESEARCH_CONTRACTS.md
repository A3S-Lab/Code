# Native Research Contracts

`a3s-code-core::research` is the Code-side identity and evidence boundary for
scientific workflows. It makes a research run replayable and reviewable
without turning Code into a package manager, a scientific policy engine, or a
second Cloud audit authority.

## Ownership

- Code owns bounded wire values, canonical serialization, domain-separated
  digests, run lifecycle transitions, and fail-closed validation.
- A3S Use owns signed package and environment selection. A host injects the
  exact `RunCapabilityBindingV1` produced by that selection.
- The host or Desktop owns project aggregates, source retrieval, reviewer
  prompts and rubrics, severity thresholds, human decisions, retention, and
  publication workflows.

The contracts are intentionally compatible with a host-owned reviewer. Code
does not depend on DeepSeek Harness or another foreign runtime and does not
interpret a review finding as approval.

## Versioned values

| Value | Schema | Purpose |
| --- | --- | --- |
| `ResearchRunV1` | `a3s.code.research-run.v1` | Binds project revision, source/evidence snapshots, Code/Use capability identity, provider/model, reproducibility promise, and lifecycle status. |
| `ResearchEvidenceFactV1` | `a3s.code.evidence-fact.v1` | Append-only, digest-only observation with a monotonic sequence and bounded metadata. |
| `ResearchClaimV1` | `a3s.code.research-claim.v1` | Digest-only claim statement with explicit `proposed`, `supported`, `conflicted`, or `unsupported` status; support, conflict, and gap digests are mutually exclusive by state. |
| `ResearchCitationV1` | `a3s.code.research-citation.v1` | Bounded citation linking one claim to a source digest and source-span digest without retaining source plaintext. |
| `ResearchEvidenceGraphV1` | `a3s.code.evidence-graph.v1` | Canonical claim/citation projection for one Run; measures publication completeness and rejects orphan citations or unlinked support digests. |
| `ResearchWorkflowStepV1` | `a3s.code.research-workflow-step.v1` | Research-visible workflow step with input/output digests, dependency edges, and optional binding to an `ExecutionResultReceiptV1`. |
| `ResearchWorkflowPlanV1` | `a3s.code.research-workflow-plan.v1` | Bounded DAG of workflow steps for one Run, fenced to the Run seed and workflow digest. |
| `ResearchRerunLineageV1` | `a3s.code.research-rerun-lineage.v1` | Finding-triggered affected-step set in dependency-first order for partial recompute without rewriting parent evidence. |
| `ResearchProvenanceReceiptV1` | `a3s.code.provenance-receipt.v1` | Binds an artifact to its inputs, workflow, code, environment, provider, optional model/seed, and validation output. |
| `ResearchReproducibilityManifestV1` | `a3s.code.reproducibility-manifest.v1` | Digests and non-secret parameters that make one Run reproducible: provider/model, optional model revision, environment lock, code/workflow/parameter digests, seed, tolerances, and output artifacts. |
| `ResearchReviewFindingV1` | `a3s.code.review-finding.v1` | Bounded host-produced observation linked to exact artifact and evidence digests; optional immutable provenance-receipt and evaluation-record bindings prevent artifact, evaluator, Run, or evidence drift; resolution and waiver are explicit lifecycle transitions. |
| `ResearchReviewBatchV1` | `a3s.code.review-batch.v1` | Bounded immutable projection of one evaluator result into findings; all findings share the same project, Run, evaluation record, and evidence snapshot. |
| `ResearchEventV1` | `a3s.code.science-event.v1` | Digest-only project/run event projection for Desktop and other hosts. |

Hosts that need a process-boundary envelope use `ResearchWireEnvelopeV1`
(`a3s.code.research-wire.v1`) with the generated Node/Python/Go projections
under `sdk/research/`. Regenerate with
`node scripts/generate_research_protocol_artifacts.mjs`.

All IDs and text are bounded and reject both embedded and trailing CR/LF line
endings at the Code boundary. Digests use the existing Code SHA-256 format.
Maps and digest lists are canonicalized before identity calculation, and every
value uses `deny_unknown_fields` so an older host cannot silently accept a
newer shape. Hosts crossing a process boundary should use each research value's
bounded `from_slice`/`to_vec` helpers; those helpers enforce the 32 MiB wire
limit and validate the complete digest tree before returning a value.

## Lifecycle and integrity

Research runs progress through `planned -> admitted -> running`, may pass
through `checkpointed`, and then terminate as `completed`, `failed`, or
`cancelled`. Terminal runs cannot resume. Every transition validates the
previous digest before recalculating the new identity.

Review findings start `open`; only `resolve` or `waive` with a content digest
can close them. Optional source/artifact locations use one-based coordinates:
line and column zero are rejected, and a column cannot appear without a line.
A finding or run whose fields were changed without updating its digest is
rejected before any transition can rebind the tampered value. Serialized
resolved and waived batches reopen with their terminal state and continue to
reject every subsequent resolution transition.

Claims start `proposed`. Hosts must call `mark_supported`, `mark_conflicted`,
or `mark_unsupported` with the matching digest set before publication.
`ResearchEvidenceGraphV1::validate_publication_completeness` rejects any
remaining proposed claim and requires every supported claim's support digests
to resolve to citations retained in the same graph. Unsupported claims must
carry an explicit gap digest so missing evidence stays visible rather than
becoming an implicit success.

A `ResearchReviewBatchV1` may contain zero findings. This is the canonical
representation of a clean reviewer result; the bound evaluator record remains
the source of the decision and evidence identity, so hosts do not need to
invent a synthetic finding merely to publish a successful review.

Evidence facts and events carry sequence numbers, but contiguous ordering and
durability remain host responsibilities. The payloads intentionally contain
digests and bounded metadata rather than prompts, source text, credentials, or
raw model/tool output.

## Host integration sequence

1. Admit a Code `ResearchRunV1` with the exact capability binding and snapshot
   digests selected by the host.
   Validate its `ExecutionTargetV1` with `validate_execution_target` before
   projecting any runtime event or evidence fact.
2. Emit `ResearchEventV1` and append `ResearchEvidenceFactV1` values as the
   run observes sources, claims, measurements, derivations, and artifacts.
   Use `ResearchEventV1::from_core_event_for_run` when adapting Core events so
   an opaque operation id cannot be mistaken for the bare Run id.
3. Materialize each output as a content-addressed artifact and publish a
   `ResearchProvenanceReceiptV1`. Bind a finding to that receipt with
   `ResearchReviewFindingV1::bind_provenance_receipt_for_run`, passing the
   admitted `ResearchRunV1`; Code checks the exact project, Run, project
   revision, provider, random seed, artifact digest, and at least one retained
   input evidence digest without interpreting the scientific rubric. The older
   `bind_provenance_receipt` form remains available for callers that do not
   retain the admitted Run, but cannot perform the project-revision check.
3b. Project claims and citations into a `ResearchEvidenceGraphV1` with
   `new_for_run`. Mark each claim supported, conflicted, or unsupported with
   explicit digests, then call `validate_publication_completeness` before a
   host treats the claim set as publishable. Code measures completeness; it
   does not invent claim text or decide scientific validity.
3c. Admit a `ResearchWorkflowPlanV1` for the same Run. Bind each completed
   step to its `ExecutionResultReceiptV1` when available. After review
   findings land, project a `ResearchRerunLineageV1` so only affected steps
   and their dependents recompute; Code does not rewrite the parent ledger.
3d. Publish a `ResearchReproducibilityManifestV1` with
   `new_for_run`, then call `validate_against_provenance` against the
   Run's artifact receipts so seed, environment, workflow, code, and
   declared output digests cannot drift before export. Code fences
   identity; hosts retain prompts and credentials behind digests.
4. Run a host-selected evaluator through the generic Code evaluation
   substrate, then project its bounded observations into
   `ResearchReviewFindingV1` values. Bind each finding to the exact
   `EvaluationRecordV1` with `bind_evaluation_record_for_run`, passing the
   admitted `ResearchRunV1`; Code verifies that the evaluator, project, Run,
   and the Run's exact evidence snapshot match before accepting the binding.
   The older
   `bind_evaluation_record` form remains available for callers that do not
   retain the admitted Run, but cannot perform the project-namespace check.
   Group the bound findings with `ResearchReviewBatchV1::new_for_run`, passing
   the admitted Run and evaluator record, before publication so a partial or
   mixed evaluator response cannot be presented as one review. The strict
   constructor and `validate_for_run` also require the batch evidence digest
   to equal both the evaluator record and admitted Run evidence digests, and
   each finding evaluator identity to equal the evaluator record. The older
   `new` constructor remains
   available for wire-level compatibility but cannot perform those checks.
5. Let the host apply its rubric, human approval, retention, and publication
   policy; Code only validates the supplied identities and lifecycle.

The end-to-end qualification fixture in
`core/tests/research_execution_qualification.rs` exercises this sequence in a
temporary workspace. It reopens the serialized Run, evaluator dispatch ledger,
and evaluator result store; rejects a missing or gapped evidence cursor;
verifies terminal cancellation cannot resume; checks create-only artifact
replay; and proves that the Code catalog and upstream A3S Use generation remain
bound to the same Run. Run it with:

```bash
CARGO_TARGET_DIR=/tmp/a3s-code-target \
  cargo test --locked --features advanced-harness --test research_execution_qualification
```

Focused contract tests live beside the implementations in
`core/src/research/`. Run them with:

```bash
CARGO_TARGET_DIR=/tmp/a3s-code-target \
  cargo test --locked --features advanced-harness -p a3s-code-core research:: --lib
```
