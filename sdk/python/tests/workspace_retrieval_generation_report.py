"""Statistical summaries for retrieval-dependent generation evaluations."""

from __future__ import annotations

import math
from collections import defaultdict
from typing import Any

from workspace_retrieval_eval_fixture import percentile


def ratio(numerator: int, denominator: int) -> float:
    return numerator / denominator if denominator else 0.0


def wilson_lower_bound(successes: int, total: int, z_score: float = 1.96) -> float:
    """Return the two-sided 95% Wilson score interval's lower bound."""

    if total <= 0 or successes < 0 or successes > total:
        return 0.0
    proportion = successes / total
    denominator = 1.0 + z_score * z_score / total
    center = proportion + z_score * z_score / (2.0 * total)
    radius = z_score * math.sqrt(
        proportion * (1.0 - proportion) / total
        + z_score * z_score / (4.0 * total * total)
    )
    return (center - radius) / denominator


def summarize_generation_runs(runs: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate compile-gated runs without treating model prose as success."""

    total = len(runs)
    passed = sum(run["passed"] for run in runs)
    evidence_expected = sum(run["expectedEvidenceCount"] for run in runs)
    evidence_returned = sum(run["returnedEvidenceCount"] for run in runs)
    lower_bound_requests = sum(run["batchLimitLowerBound"] for run in runs)
    document_requests = sum(run["documentProviderRequests"] for run in runs)
    by_task: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        by_task[run["task"]].append(run)

    task_summaries = {}
    for task, task_runs in sorted(by_task.items()):
        task_passed = sum(run["passed"] for run in task_runs)
        task_summaries[task] = {
            "runs": len(task_runs),
            "passed": task_passed,
            "passRate": ratio(task_passed, len(task_runs)),
            "wilsonLowerBound95": wilson_lower_bound(task_passed, len(task_runs)),
        }

    return {
        "runs": total,
        "passed": passed,
        "passRate": ratio(passed, total),
        "wilsonLowerBound95": wilson_lower_bound(passed, total),
        "taskPassRates": task_summaries,
        "toolProtocolRate": ratio(
            sum(run["toolProtocolOk"] for run in runs), total
        ),
        "evidenceRecallAt5": ratio(evidence_returned, evidence_expected),
        "compilePassRate": ratio(sum(run["cargoPassed"] for run in runs), total),
        "workspaceIntegrityRate": ratio(
            sum(run["workspaceIntegrity"] for run in runs), total
        ),
        "releaseRate": ratio(sum(run["releasedAfterClose"] for run in runs), total),
        "documentRequestAmplification": ratio(
            document_requests, lower_bound_requests
        ),
        "nonTextProviderInputs": sum(run["nonTextProviderInputs"] for run in runs),
        "sessionConstructionP50Ms": percentile(
            [run["sessionConstructionMs"] for run in runs], 0.5
        ),
        "sessionConstructionP95Ms": percentile(
            [run["sessionConstructionMs"] for run in runs], 0.95
        ),
        "indexReadyP50Ms": percentile([run["indexReadyMs"] for run in runs], 0.5),
        "indexReadyP95Ms": percentile(
            [run["indexReadyMs"] for run in runs], 0.95
        ),
        "incrementalReadyP50Ms": percentile(
            [
                run["incrementalReadyMs"]
                for run in runs
                if run["incrementalReadyMs"] is not None
            ],
            0.5,
        ),
        "incrementalReadyP95Ms": percentile(
            [
                run["incrementalReadyMs"]
                for run in runs
                if run["incrementalReadyMs"] is not None
            ],
            0.95,
        ),
        "timeToFirstReadyP50Ms": percentile(
            [run["timeToFirstReadyMs"] for run in runs], 0.5
        ),
        "timeToFirstReadyP95Ms": percentile(
            [run["timeToFirstReadyMs"] for run in runs], 0.95
        ),
        "postEditTimeToFirstReadyP50Ms": percentile(
            [
                run["postEditTimeToFirstReadyMs"]
                for run in runs
                if run["postEditTimeToFirstReadyMs"] is not None
            ],
            0.5,
        ),
        "postEditTimeToFirstReadyP95Ms": percentile(
            [
                run["postEditTimeToFirstReadyMs"]
                for run in runs
                if run["postEditTimeToFirstReadyMs"] is not None
            ],
            0.95,
        ),
        "turnP50Ms": percentile([run["turnElapsedMs"] for run in runs], 0.5),
        "turnP95Ms": percentile([run["turnElapsedMs"] for run in runs], 0.95),
        "cargoP50Ms": percentile([run["cargoElapsedMs"] for run in runs], 0.5),
        "cargoP95Ms": percentile([run["cargoElapsedMs"] for run in runs], 0.95),
        "closeP50Ms": percentile([run["closeMs"] for run in runs], 0.5),
        "closeP95Ms": percentile([run["closeMs"] for run in runs], 0.95),
        "totalTokens": sum(run["totalTokens"] for run in runs),
        "documentProviderRequests": document_requests,
    }


def assess_generation_summary(
    summary: dict[str, Any],
    evaluation: dict[str, Any],
    *,
    revision_locked: bool,
    full_task_matrix: bool,
) -> dict[str, bool]:
    """Apply the versioned production gates to one aggregate report."""

    def at_most(metric: str, limit: str) -> bool:
        value = summary.get(metric)
        return (
            isinstance(value, (int, float))
            and not isinstance(value, bool)
            and math.isfinite(value)
            and value <= evaluation[limit]
        )

    return {
        "revisionLocked": revision_locked,
        "fullTaskMatrix": full_task_matrix,
        "passRate": summary["passRate"] >= evaluation["minimum_pass_rate"],
        "statisticalConfidence": summary["wilsonLowerBound95"]
        >= evaluation["minimum_wilson_lower_bound"],
        "toolProtocol": summary["toolProtocolRate"] == 1.0,
        "evidenceCoverage": summary["evidenceRecallAt5"] == 1.0,
        "compileOracle": summary["compilePassRate"] == 1.0,
        "workspaceIntegrity": summary["workspaceIntegrityRate"] == 1.0,
        "requestAmplification": summary["documentRequestAmplification"]
        <= evaluation["maximum_document_request_amplification"],
        "nonTextEgress": summary["nonTextProviderInputs"] == 0,
        "releasedAfterClose": summary["releaseRate"] == 1.0,
        "sessionConstructionP95": at_most(
            "sessionConstructionP95Ms", "maximum_session_construction_p95_ms"
        ),
        "indexReadyP95": at_most(
            "indexReadyP95Ms", "maximum_index_ready_p95_ms"
        ),
        "incrementalReadyP95": at_most(
            "incrementalReadyP95Ms", "maximum_incremental_ready_p95_ms"
        ),
        "timeToFirstReadyP95": at_most(
            "timeToFirstReadyP95Ms", "maximum_time_to_first_ready_p95_ms"
        ),
        "postEditTimeToFirstReadyP95": at_most(
            "postEditTimeToFirstReadyP95Ms",
            "maximum_post_edit_time_to_first_ready_p95_ms",
        ),
        "closeP95": at_most("closeP95Ms", "maximum_close_p95_ms"),
    }
