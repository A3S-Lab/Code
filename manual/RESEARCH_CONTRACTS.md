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
| `ResearchProvenanceReceiptV1` | `a3s.code.provenance-receipt.v1` | Binds an artifact to its inputs, workflow, code, environment, provider, optional model/seed, and validation output. |
| `ResearchReviewFindingV1` | `a3s.code.review-finding.v1` | Bounded host-produced observation linked to exact artifact and evidence digests; optional immutable provenance-receipt and evaluation-record bindings prevent artifact, evaluator, Run, or evidence drift; resolution and waiver are explicit lifecycle transitions. |
| `ResearchReviewBatchV1` | `a3s.code.review-batch.v1` | Bounded immutable projection of one evaluator result into findings; all findings share the same project, Run, evaluation record, and evidence snapshot. |
| `ResearchEventV1` | `a3s.code.science-event.v1` | Digest-only project/run event projection for Desktop and other hosts. |

All IDs and text are bounded. Digests use the existing Code SHA-256 format.
Maps and digest lists are canonicalized before identity calculation, and every
value uses `deny_unknown_fields` so an older host cannot silently accept a
newer shape.

## Lifecycle and integrity

Research runs progress through `planned -> admitted -> running`, may pass
through `checkpointed`, and then terminate as `completed`, `failed`, or
`cancelled`. Terminal runs cannot resume. Every transition validates the
previous digest before recalculating the new identity.

Review findings start `open`; only `resolve` or `waive` with a content digest
can close them. A finding or run whose fields were changed without updating its
digest is rejected before any transition can rebind the tampered value.

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
   `ResearchReviewFindingV1::bind_provenance_receipt`; Code checks the exact
   project, Run, artifact digest, and at least one retained input evidence
   digest without interpreting the scientific rubric.
4. Run a host-selected evaluator through the generic Code evaluation
   substrate, then project its bounded observations into
   `ResearchReviewFindingV1` values. Bind each finding to the exact
   `EvaluationRecordV1` with `bind_evaluation_record`; Code verifies that the
   evaluator, Run, and evidence snapshot match before accepting the binding.
   Group the bound findings in a `ResearchReviewBatchV1` before publication so
   a partial or mixed evaluator response cannot be presented as one review.
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
  cargo test --locked --no-default-features --test research_execution_qualification
```

Focused contract tests live beside the implementations in
`core/src/research/`. Run them with:

```bash
CARGO_TARGET_DIR=/tmp/a3s-code-target \
  cargo test --locked --no-default-features -p a3s-code-core research:: --lib
```
