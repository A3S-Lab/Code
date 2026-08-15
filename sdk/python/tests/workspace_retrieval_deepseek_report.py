"""Normalized report calculations for the real DeepSeek SDK runners."""

from __future__ import annotations

import math
from typing import Any

from workspace_retrieval_eval_fixture import percentile


def summarize(runs: list[dict[str, Any]]) -> dict[str, Any]:
    """Summarize independently observed retrieval and lifecycle runs."""

    ranks = [run["expectedPathRank"] for run in runs]
    relevant = sum(rank is not None and rank <= 5 for rank in ranks)
    returned = sum(run["resultCount"] for run in runs)
    lower_bound = sum(run["batching"]["batchLimitLowerBound"] for run in runs)
    document_requests = sum(
        run["batching"]["documentProviderRequests"] for run in runs
    )
    return {
        "taskAccuracy": sum(run["completionCorrect"] for run in runs) / len(runs),
        "toolProtocolRate": sum(run["toolProtocolOk"] for run in runs) / len(runs),
        "precisionAt5": relevant / (len(runs) * 5),
        "returnedResultPrecision": relevant / returned,
        "recallAt5": relevant / len(runs),
        "mrr": sum(0 if rank is None else 1 / rank for rank in ranks) / len(runs),
        "ndcgAt5": sum(
            0 if rank is None or rank > 5 else 1 / math.log2(rank + 1)
            for rank in ranks
        )
        / len(runs),
        "documentRequestAmplification": document_requests / lower_bound,
        "meanReturnedResults": returned / len(runs),
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
        "timeToFirstReadyP50Ms": percentile(
            [run["batching"]["timeToFirstReadyMs"] for run in runs], 0.5
        ),
        "timeToFirstReadyP95Ms": percentile(
            [run["batching"]["timeToFirstReadyMs"] for run in runs], 0.95
        ),
        "turnP50Ms": percentile([run["turnElapsedMs"] for run in runs], 0.5),
        "turnP95Ms": percentile([run["turnElapsedMs"] for run in runs], 0.95),
        "closeP50Ms": percentile([run["closeMs"] for run in runs], 0.5),
        "closeP95Ms": percentile([run["closeMs"] for run in runs], 0.95),
        "totalTokens": sum(run["totalTokens"] for run in runs),
        "nonTextProviderInputs": sum(
            run["provider"]["nonTextInputs"] for run in runs
        ),
        "releasedAfterCloseRate": sum(run["releasedAfterClose"] for run in runs)
        / len(runs),
    }
