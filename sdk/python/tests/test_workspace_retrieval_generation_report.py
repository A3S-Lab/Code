"""Hermetic contracts for generation evaluation statistics."""

from __future__ import annotations

import math

from workspace_retrieval_generation_report import (
    summarize_generation_runs,
    wilson_lower_bound,
)


def generation_run(task: str, passed: bool) -> dict[str, object]:
    return {
        "task": task,
        "passed": passed,
        "toolProtocolOk": passed,
        "expectedEvidenceCount": 2,
        "returnedEvidenceCount": 2 if passed else 1,
        "cargoPassed": passed,
        "workspaceIntegrity": True,
        "releasedAfterClose": True,
        "batchLimitLowerBound": 1,
        "documentProviderRequests": 1,
        "nonTextProviderInputs": 0,
        "sessionConstructionMs": 3,
        "indexReadyMs": 5,
        "incrementalReadyMs": 2,
        "timeToFirstReadyMs": 4,
        "postEditTimeToFirstReadyMs": 1,
        "turnElapsedMs": 7,
        "cargoElapsedMs": 11,
        "closeMs": 13,
        "totalTokens": 17,
    }


def test_wilson_lower_bound_contract() -> None:
    assert math.isclose(wilson_lower_bound(0, 0), 0.0)
    assert 0.70 < wilson_lower_bound(9, 9) < 0.71
    assert wilson_lower_bound(8, 9) < wilson_lower_bound(9, 9)


def test_generation_summary_uses_compile_and_evidence_observables() -> None:
    summary = summarize_generation_runs(
        [generation_run("alpha", True), generation_run("beta", False)]
    )

    assert summary["runs"] == 2
    assert summary["passRate"] == 0.5
    assert summary["toolProtocolRate"] == 0.5
    assert summary["evidenceRecallAt5"] == 0.75
    assert summary["compilePassRate"] == 0.5
    assert summary["workspaceIntegrityRate"] == 1
    assert summary["documentRequestAmplification"] == 1
    assert summary["totalTokens"] == 34
