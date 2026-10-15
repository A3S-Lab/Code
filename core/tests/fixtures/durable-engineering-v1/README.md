# Frozen engineering acceptance fixtures

T003 defines these fixtures before strategy implementation or model selection.
`fixtures.rs` owns exact project/check/request/reference bytes. The sibling
`core/tests/durable_engineering_fixtures.rs` calibrates correct and incorrect
references against Node and independently checks protected bytes.

| Partition | Task | Required behavior | Incorrect reference |
|---|---|---|---|
| development | reuse | Use the existing normalizer for each input; protect shared code/checks | Duplicate behavior without calling the shared implementation |
| development | callers | Update both product callers for the existing shared API | Fix only service and leave CLI broken |
| held-out | boundary | Add editor access while preserving tenant isolation and default denial | Add roles but omit tenant checking |
| held-out | scope | Correct numeric addition and preserve unrelated document/checks | Wrong arithmetic, altered document, or replaced check |

Do not use held-out model outcomes to tune the strategy body. If a fixture or
check changes, freeze a new version and rerun all reference calibration; do not
silently replace an initial failure. The durable multi-Run task reuses the
caller project and its unchanged check, with staged partial/full references.
Fixture calibration is not a real-model run, a product end-to-end test, or an
independent HC-ACCEPT certificate.

For later paired model evaluation, use the existing Layer C config helper,
same model/effort/permissions/resources and preregistered limits: one primary
attempt per task/arm, at most one diagnostic retry retained separately,
max20 tool rounds and600 seconds per task. Primary arms are ordinary,
engineering, goal and combined. Development-only instruction ablations are
separate diagnostics. Record actual model/tool/usage, failure/unknown and
original effect/verification/settlement evidence; elapsed is not autonomy.

Real code execution must use the existing qualified tool/host boundary.
The fixed-reference Node checks in the calibration test run only repository
fixture code, and do not justify a native sandbox fallback for model edits.
