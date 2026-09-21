# Typed System-1 Decision E2E Plan

**Status:** TD-A through TD-E implemented. Hermetic TD-B/TD-C run under
`--features apofasi`. TD-D and TD-E are `#[ignore]` and were executed against
the remapped pin `boyue/bailian/deepseek-v4.1-flash`.  
**Mechanism:** optional Code substrate `typed_decision` (A3S Apofasi System One)  
**Inventory id:** `typed_decisions` (Advanced, host-owned)  
**Cargo gate:** `apofasi` (lexical). `apofasi-infer` / `apofasi-metal` load the
published checkpoint for the ignored default-gate Auto test. That test records
generation count. It does not treat the chosen label as correctness.

This plan is the end-to-end test contract for the type-safe decision mechanism.
It composes [FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md) and
[FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md). It does not replace them.

## 0. Model pin (verified against `./.a3s/config.acl`)

The requested id `boyue/bailina/deepseek-v4-flash` is **not** declared.
`bailina` is not a provider or model segment in the ACL.

Declared boyue Flash routes in that file:

| ACL fact | Value |
| --- | --- |
| `default_model` | `boyue/deepseek-v4-flash` |
| Second Flash model id | `bailian/deepseek-v4.1-flash` |
| Full alternate route | `boyue/bailian/deepseek-v4.1-flash` |

**This plan’s live pin** matches the existing Layer C contract in
`core/tests/support/layer_c_model.rs`:

```bash
export A3S_CONFIG_FILE="$(git rev-parse --show-toplevel)/.a3s/config.acl"
export A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash
```

The loader remaps that unset id to the declared peer
`boyue/bailian/deepseek-v4.1-flash` and pins `CodeConfig.default_model`.
Suites must assert the **remapped** id, never the typo and never provider prose.

Do not copy API keys from the ACL into tests, logs, or this plan.

## 1. First principles

The decision mechanism and the generative model are different machines.

| Owner | Owns | Must not own |
| --- | --- | --- |
| Apofasi | Typed `choice` / `score` / `noul`, calibrated confidence | Free-form generation, host policy, tool side effects |
| Code Core | Admission of its own generations, `TypedDecisionEngine`, gate facade, digest receipt, `CodeError::TypedDecision` | Thresholds, question schemas, Desktop routing, forging an `Answer` from model text |
| Host | Extra call sites, `GatePolicy`, whether escalate invokes a model | Forging an `Answer` from model text |

Consequences for tests:

1. **Kernel effects, not labels.** Assert answer variant, `GateAction`, receipt
   schema, domain-separated digests, typed errors, and provider-call counts.
   Do not assert that lexical `choice` equals DeepSeek’s wording.
2. **Hermetic twin before any live call.** Every live branch has a recording
   fake `LlmClient` that proves the same call-count invariant with no network.
3. **The lexical engine makes zero provider calls.** A green live suite that
   only checks DeepSeek JSON is the wrong product.
4. **Auto means no generation.** `GateAction::Auto` must record zero model
   generations. `Escalate` may record exactly one generation, and only on the
   host side after Apofasi has already returned.
5. **Fail closed.** Empty `questions` never reaches the engine. Unparseable
   escalate output never becomes an `Answer`. Missing gate rows escalate.
6. **Thin default stays thin.** `local-code` must not link `a3s-apofasi`.
   Inventory presence is not linkage: `typed_decisions` is listed even when
   the feature is off.
7. **Live is not Required CI.** Pin identity and escalate composition are
   `#[ignore]` / manual, same class as Layer C.

## 2. What “end to end” means here

End to end is the host composition, not a Desktop screenshot:

```text
admitted SystemOneRequest
  → TypedDecisionEngine::decide
  → gate_system_one_response(GatePolicy::default)
  → TypedDecisionReceiptV1
  → host: Auto stops | Escalate calls the pinned model once
  → escalate prompt includes the task state and each question's type
  → host parser: valid structured text stays host-side; invalid text does not
    enter Answer
```

Desktop routing and Use `CapabilityKind` stay out of scope (`HOST-APOFASI`
is future work). A host may pass a stricter `GatePolicy`. That boundary is a
unit test (`min_confidence = 1`). It is not the product proof of Auto or
Escalate. Product Auto uses an engine whose confidence clears the default
`0.7` gate. Product Escalate uses the lexical engine on the refund request at
that same gate. The ignored neural test repeats Auto with the published
checkpoint and does not lower the gate.

## 3. Layers

### TD-A — Absence (Required CI, no network)

| Oracle | Evidence |
| --- | --- |
| `cargo tree -p a3s-code-core --features local-code -i a3s-apofasi` fails to resolve the package | Thin build does not link Apofasi |
| `cargo check -p a3s-code-core --features local-code` succeeds | Default product still builds |
| `sdk_capabilities()` still contains `typed_decisions` as Advanced | Discovery does not hide an opt-in surface |

### TD-B — Hermetic substrate (Required CI, `--features apofasi`, no network)

Already owned by `core/src/typed_decision.rs` tests. A change to the mechanism
is incomplete unless these stay green:

| Case | Oracle |
| --- | --- |
| Lexical choice | `Answer::Choice`, non-empty key |
| Score and noul | `Answer::Score` and `Answer::Noul` on one multi-kind request |
| Empty questions | `TypedDecisionError::EmptyRequest` before the engine |
| Default gate | Each row is `Auto` or `Escalate` |
| Impossible thresholds | `min_confidence = 1` and `min_noul_extremity = 1` ⇒ every row `Escalate` |
| Receipt | Schema `a3s-code/typed-decision-receipt/v1`, digests pass `validate_digest`, request digest matches `digest_json` |
| Route | `route` admits the same non-empty request |
| Trait | `TypedDecisionEngine` on `TypedDecisionService` |
| Neural off | `load_neural_engine` is `NeuralFeatureDisabled` without `apofasi-infer` |

Command:

```bash
cd crates/code
cargo test -p a3s-code-core --features apofasi --lib typed_decision
```

### TD-C — Host composition, hermetic (Required CI)

Owned by the `compose_host_decision` tests in `core/src/typed_decision.rs`.

Build a test host next to Core tests (feature `apofasi`) that:

1. Runs `decide_and_gate_with_receipt` on a fixed choice request.
2. Records generations on a fake client whose configured model id is
   `boyue/bailian/deepseek-v4.1-flash` (the remapped pin, no HTTP).
3. Branches on `GatedDecision.escalate` at `GatePolicy::default()` (`0.7`):
   - Auto: a fixture engine whose confidence is above `0.7`. Fake client
     invocation count is 0. Do not set `min_confidence` to `0`.
   - Escalate: the lexical engine on the refund request, which does not clear
     `0.7`. Invocation count is 1. The prompt contains the task state. The
     recorded model id equals the remapped pin. Apofasi still produced typed
     answers before that call.
4. Feeds the fake client three escalate payloads:
   - JSON object whose keys are exactly the question ids → host accepts it as
     **host evidence**, not as a replacement `Answer`.
   - Empty / non-JSON / extra-only keys → host error, original `Answer` and
     receipt unchanged.
5. Mutating `state` changes `request_digest` and does not change the schema id.

No test in TD-C may open a socket.

### TD-D — Pin identity, live config (ignored, ACL required)

Does not generate tokens. Proves the operator pin against the real ACL.

| Step | Oracle |
| --- | --- |
| `A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash` | Loader stderr says it is using `boyue/bailian/deepseek-v4.1-flash` |
| `load_pinned_layer_c_config()` | `default_model == boyue/bailian/deepseek-v4.1-flash` |
| Provider `boyue` | Model id `bailian/deepseek-v4.1-flash` exists |
| Negative | `boyue/bailina/deepseek-v4-flash` is not a declared route and must not be asserted as success |

Suggested suite: `core/tests/typed_decision_layer_c.rs`, `#[ignore]`,
one thread, same `A3S_CONFIG_FILE` rule as other Layer C tests.

### TD-E — Escalate generation, live (ignored, ACL + network)

Run only after TD-C is green. Reuse the same host branch with a real provider
client built from the pinned `CodeConfig`.

| Oracle | Pass | Fail |
| --- | --- | --- |
| Model id on the generation request | Exactly `boyue/bailian/deepseek-v4.1-flash` | Any other model, including the unmapped `v4-flash` typo |
| Call count when the default gate escalates | 1 | 0 or >1 |
| Call count when the default gate is Auto | 0 | Any generation |
| Escalate prompt | Contains the task state | Question ids only |
| Apofasi answer | Still a typed `Answer` produced before the provider call | Answer parsed out of model text |
| Receipt | Digests cover the System One request/response, not the model transcript | Receipt stores prompt text or API keys |
| Bad model output | Host typed error; receipt from Apofasi retained | Model text written into `Answer` |
| Bounds | One request. Complex cases are the schema three-kind map and the structured four-question billing triage | Open-ended chat, tool loop, Desktop session |

Do not assert department names, scores, or noul values from the live model.
The suite exists and stays out of `just layer-c-live-e2e`: that recipe is
required CI, and a live provider key is a non-goal. The tests stay serial and
`#[ignore]`.

Operator command:

```bash
cd crates/code
export A3S_CONFIG_FILE="$(git rev-parse --show-toplevel)/.a3s/config.acl"
export A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash
cargo test -p a3s-code-core --features apofasi --test typed_decision_layer_c -- --ignored --test-threads=1
```

The default-gate Auto twin needs the published checkpoint and the neural feature.
It stays ignored. It asserts generation count, not the chosen label. The complex
case is the same three-kind payout request as `schema.rs`: lexical must
escalate once at the default gate, and the neural run's generation count must
match that gate. The structured billing triage (object state, choice, score,
and two nouls) is the same request the Apofasi binary benches. Lexical must
escalate once there too. The neural run's generation count matches that gate.
Neither test asserts labels or lowers `0.7`.

```bash
cd crates/code
export APOFASI_CHECKPOINT=/path/to/published/english/checkpoint
cargo test -p a3s-code-core --features apofasi,apofasi-metal --test typed_decision_layer_c live_neural_auto -- --ignored --test-threads=1
```

## 4. Explicit non-goals

- Desktop, CLI, or Cloud product routing.
- Use-projected `CapabilityKind`.
- Agreement between lexical labels and DeepSeek labels.
- Treating `sdk_capabilities` id `typed_decisions` as proof the engine is linked.
- Required CI dependence on the boyue provider endpoint or any live key.

## 5. Implementation order

1. Keep TD-A and TD-B as they are; do not weaken them to make TD-E easier.
2. Add the TD-C host harness and its fake-client oracles.
3. Add ignored TD-D pin identity on `load_pinned_layer_c_config`.
4. Add ignored TD-E only as the live twin of the TD-C escalate branch.
5. Layer C docs point here. TD-D and TD-E live in
   `core/tests/typed_decision_layer_c.rs` (`#[ignore]`, feature `apofasi`).

## 6. Done when

| Layer | Done means |
| --- | --- |
| TD-A | `local-code` tree has no `a3s-apofasi`; check succeeds |
| TD-B | `cargo test -p a3s-code-core --features apofasi --lib typed_decision` passes |
| TD-C | Hermetic host proves 0 vs 1 fake generations and fail-closed parse |
| TD-D | Ignored test shows remapped `boyue/bailian/deepseek-v4.1-flash` |
| TD-E | Ignored live tests show one default-gate escalate generation on the single-question pin, the three-kind payout request, and the structured billing triage. Neural checkpoint tests show zero generations on Auto and a matching count on each complex request |
| TD-PERF | Ignored `typed_decision_perf` Flash percentiles on the billing triage at default 0.7; recorded in [ENTERPRISE_GA_E2E_PLAN.md](ENTERPRISE_GA_E2E_PLAN.md). Not an L6 digest |

TD-D, TD-E, and TD-PERF are release qualification, not merge blockers.
Enterprise GA still requires L6 digests and L7 receipts as named in
[ENTERPRISE_GA_E2E_PLAN.md](ENTERPRISE_GA_E2E_PLAN.md).
