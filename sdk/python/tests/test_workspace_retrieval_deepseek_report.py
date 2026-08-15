"""Hermetic contracts for real DeepSeek retrieval report calculations."""

from __future__ import annotations

import math

from workspace_retrieval_deepseek_report import summarize


def test_workspace_retrieval_deepseek_summary_contract() -> None:
    run = {
        "completionCorrect": True,
        "toolProtocolOk": True,
        "expectedPathRank": 2,
        "resultCount": 5,
        "sessionConstructionMs": 3,
        "indexReadyMs": 7,
        "turnElapsedMs": 11,
        "closeMs": 13,
        "totalTokens": 17,
        "batching": {
            "documentProviderRequests": 1,
            "batchLimitLowerBound": 1,
            "timeToFirstReadyMs": 5,
        },
        "provider": {"nonTextInputs": 0},
        "releasedAfterClose": True,
    }

    summary = summarize([run])

    assert summary["taskAccuracy"] == 1
    assert summary["toolProtocolRate"] == 1
    assert summary["recallAt5"] == 1
    assert summary["mrr"] == 0.5
    assert math.isclose(summary["ndcgAt5"], 1 / math.log2(3))
    assert summary["documentRequestAmplification"] == 1
    assert summary["nonTextProviderInputs"] == 0
    assert summary["releasedAfterCloseRate"] == 1
