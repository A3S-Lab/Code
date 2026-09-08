# Cloud / Box Harness Conformance Checklist

Code-side readiness for Cloud `AgentExecutionProvider` and Box workload
certification (`CAR-01` … `CAR-05`). This document does **not** replace Cloud or
Box evidence. Closing a CAR gate requires a linked external run or conformance
report; “mechanism exists in Core” is insufficient.

See also `ROADMAP.md` §2, the cross-repository Agent Runtime platform roadmap,
and the wrap-up evidence templates in
[HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md).

## Ownership

| Owner | Responsibility |
| --- | --- |
| A3S Code | Session/run loop, tool evidence, transforms, checkpoint export payloads, fail-closed local recovery |
| A3S Cloud | Execution identity, sequencing, approvals, checkpoint/fork business lineage, audit, placement |
| A3S Box / Sandbox | Stronger workload isolation; Box-hosted restart/cancel/hostile-tool matrix (`CAR-05`) |

## Gate status matrix

| Gate | Code readiness (this repo) | Still required externally | Blocking party |
| --- | --- | --- | --- |
| `CAR-01` | Native harness command/receipt/event-page/cancel/recovery slices delivered; fail-closed workspace/session admission present | Cloud `A1.2`/`A1.3` cross-repo conformance run against current Code revision | Cloud + Code integration |
| `CAR-02` | Delivered — immutable-content adapter + digest-bound tool evidence | Cloud authorization/object lifecycle (Cloud-owned) | — |
| `CAR-03` | Deterministic transforms, policy digests, shared ToolRegistry observation pipeline delivered | Cloud-managed profile admission + cross-repository conformance | Cloud |
| `CAR-04` | `SessionCheckpointExportV1`, live sink, exact catalog/ceiling/Use cursor binding, split-write recovery removed | Common Harness adoption + real provider/Box certification (`A1.6` Cloud-owned identity) | Cloud + Box |
| `CAR-05` | Planned — no Code-to-node control path by design | One Cloud-managed Box workload proving restart, exact replay, cancel, hostile Tool output, bounded content, Secret redaction, checkpoint, cleanup | Cloud + Box |

## Evidence rules

1. Link a Cloud CI run, Box job ID, or signed conformance artifact in the ROADMAP
   exit cell when marking a CAR gate Delivered.
2. Reports must be secret-free (no provider keys, tokens, or raw prompts).
3. Do not mark `CAR-05` Delivered from local Docker-only diagnostics.

## Related Code live gates

These qualify the coding harness against a live provider; they are not CAR
substitutes:

- `test_deepseek_adversarial_e2e`
- `test_prompt_capability_real_llm`

Point `A3S_CONFIG_FILE` at a monorepo `.a3s/config.acl` and run with
`--ignored --test-threads=1`.
