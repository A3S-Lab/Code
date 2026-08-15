"""Versioned real-embedding model matrix and comparison gates."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from workspace_retrieval_eval_fixture import FIXTURE


MODEL_MATRIX_PATH = (
    Path(__file__).resolve().parents[2]
    / "evaluation"
    / "workspace-retrieval-embedding-models-v1.json"
)
MODEL_MATRIX: dict[str, Any] = json.loads(
    MODEL_MATRIX_PATH.read_text(encoding="utf-8")
)


def validate_model_matrix_contract() -> None:
    if MODEL_MATRIX.get("schema_version") != 1:
        raise ValueError("unsupported real-embedding model matrix")
    if MODEL_MATRIX.get("source_fixture_id") != FIXTURE["fixture_id"]:
        raise ValueError("real-embedding matrix fixture identity drifted")

    cases = MODEL_MATRIX.get("cases")
    if not isinstance(cases, list) or len(cases) < 2:
        raise ValueError("real-embedding matrix requires at least two cases")
    names: set[str] = set()
    for case in cases:
        name = case.get("name")
        if not isinstance(name, str) or not name or name in names:
            raise ValueError(f"invalid or duplicate matrix case name: {name}")
        names.add(name)
        if not isinstance(case.get("model"), str) or not case["model"]:
            raise ValueError(f"matrix case {name} has no model")
        revision = case.get("revision")
        if (
            not isinstance(revision, str)
            or len(revision) != 40
            or any(character not in "0123456789abcdef" for character in revision)
        ):
            raise ValueError(f"matrix case {name} has an unlocked revision")
        if not isinstance(case.get("dimension"), int) or case["dimension"] <= 0:
            raise ValueError(f"matrix case {name} has an invalid dimension")
        if case.get("rerank") not in {"rrf-only", "deterministic"}:
            raise ValueError(f"matrix case {name} has an invalid rerank mode")
        if not isinstance(case.get("expected_qualified"), bool):
            raise ValueError(f"matrix case {name} lacks a qualification outcome")
        failed = case.get("expected_failed_tasks")
        if not isinstance(failed, list) or not all(
            isinstance(task, str) for task in failed
        ):
            raise ValueError(f"matrix case {name} has invalid failed tasks")

    comparisons = MODEL_MATRIX.get("comparisons")
    if not isinstance(comparisons, dict):
        raise ValueError("real-embedding matrix comparisons are missing")
    if comparisons.get("preferred") not in names:
        raise ValueError("preferred matrix case is unknown")
    if comparisons.get("challenger") not in names:
        raise ValueError("challenger matrix case is unknown")
    metrics = comparisons.get("metrics")
    if not isinstance(metrics, list) or not metrics:
        raise ValueError("real-embedding comparison metrics are missing")


def failed_tasks(report: dict[str, Any]) -> list[str]:
    return sorted(
        run["task"]
        for run in report["runs"]
        if run["semanticRank"] is None or run["hybridRank"] is None
    )


def assess_model_matrix(reports: dict[str, dict[str, Any]]) -> dict[str, Any]:
    validate_model_matrix_contract()
    case_gates: dict[str, bool] = {}
    for case in MODEL_MATRIX["cases"]:
        report = reports[case["name"]]
        case_gates[case["name"]] = (
            report["dimension"] == case["dimension"]
            and report["allGatesPassed"] == case["expected_qualified"]
            and failed_tasks(report) == sorted(case["expected_failed_tasks"])
        )

    comparison = MODEL_MATRIX["comparisons"]
    preferred = reports[comparison["preferred"]]["summary"]
    challenger = reports[comparison["challenger"]]["summary"]
    comparison_gates = {
        metric: preferred[metric] >= challenger[metric]
        for metric in comparison["metrics"]
    }
    return {
        "caseExpectations": case_gates,
        "preferredNotWorse": comparison_gates,
        "allGatesPassed": all(case_gates.values())
        and all(comparison_gates.values()),
    }


validate_model_matrix_contract()
