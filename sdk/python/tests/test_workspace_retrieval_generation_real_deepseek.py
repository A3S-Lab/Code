"""Compile-gated real-model evaluation for retrieval-dependent generation."""

from __future__ import annotations

import argparse
import asyncio
import importlib.metadata
import json
import os
import platform
import sys
import tempfile
import time
import unittest
from pathlib import Path
from typing import Any, cast

from a3s_code import (
    Agent,
    PermissionPolicy,
    RecursiveWorkspaceChunkingStrategy,
    SessionOptions,
    WorkspaceRetrievalOptions,
    WorkspaceRetrievalStatus,
)
from workspace_retrieval_generation_fixture import (
    FIXTURE,
    TASK_MARKER,
    materialize_task,
    validate_generation_fixture_contract,
    write_hidden_test,
)
from workspace_retrieval_generation_report import summarize_generation_runs
from workspace_retrieval_generation_runtime import (
    COMPLETION_MARKER,
    cargo_test,
    embedding_counters,
    event_payloads,
    task_prompt,
    wait_for_incremental_ready,
    workspace_integrity,
)
from workspace_retrieval_real_embedding_support import (
    load_sentence_transformer,
    sentence_transformer_provider,
    wait_for_retrieval_ready,
)


async def run_generation_task(
    agent: Any,
    model: Any,
    dimension: int,
    task: dict[str, Any],
    repetition: int,
) -> dict[str, Any]:
    evaluation = FIXTURE["evaluation"]
    embedding = FIXTURE["embedding"]
    with tempfile.TemporaryDirectory(prefix="a3s-wsr-generation-") as root_text:
        root = Path(root_text)
        corpus = materialize_task(root, task)
        if corpus["digest"] != task["expected_digest"]:
            raise AssertionError(f"generation corpus drifted for {task['name']}")
        counters = embedding_counters()
        provider = sentence_transformer_provider(
            model,
            embedding["model"],
            embedding["revision"],
            dimension,
            counters,
            query_id=embedding["query_id"],
            non_text_sentinel="NON_TEXT_ASSET_SENTINEL",
        )
        retrieval = WorkspaceRetrievalOptions(
            provider,
            None,
            RecursiveWorkspaceChunkingStrategy(
                FIXTURE["chunking"]["target_bytes"],
                FIXTURE["chunking"]["overlap_bytes"],
                FIXTURE["chunking"]["separators"],
            ),
        )
        options = SessionOptions()
        options.session_id = f"wsr-generation-{repetition}-{task['name']}"
        options.model = FIXTURE["chat_model"]
        options.planning_mode = "disabled"
        options.goal_tracking = False
        options.permission_policy = PermissionPolicy(
            allow=["search(*)", "edit(*)"], default_decision="deny"
        )
        options.guidelines = (
            "This is a bounded retrieval-dependent generation evaluation. Follow "
            "the exact two-tool protocol, use only returned repository evidence, "
            "and edit only src/solution.rs."
        )
        options.max_parse_retries = 1
        options.max_tool_rounds = 3
        options.manual_delegation_enabled = False
        options.auto_parallel = False
        options.temperature = 0.0
        options.workspace_retrieval = retrieval

        construction_started = time.monotonic()
        session = await agent.session_async(str(root), options)
        construction_ms = int((time.monotonic() - construction_started) * 1000)
        status, ready_ms = await wait_for_retrieval_ready(
            session, evaluation["ready_timeout_seconds"]
        )
        initial_batching = status["batching"]
        initial_document_inputs = counters["documentInputs"]
        turn_failure: str | None = None
        result: Any | None = None
        events: list[dict[str, Any]] = []
        turn_started = time.monotonic()
        try:
            result = await asyncio.wait_for(
                session.send_async(task_prompt(task)),
                timeout=evaluation["turn_timeout_seconds"],
            )
            runs = await session.runs_async()
            if len(runs) == 1:
                events = await session.run_events_async(runs[0]["id"])
            else:
                turn_failure = "UnexpectedRunCount"
        except Exception as error:  # noqa: BLE001 - classify without source text
            turn_failure = type(error).__name__
        turn_ms = int((time.monotonic() - turn_started) * 1000)

        calls = event_payloads(events)
        searches = [call for call in calls if call.get("name") == "search"]
        edits = [call for call in calls if call.get("name") == "edit"]
        search = searches[0] if len(searches) == 1 else {}
        edit = edits[0] if len(edits) == 1 else {}
        search_args = search.get("args") or {}
        edit_args = edit.get("args") or {}
        search_results = (search.get("metadata") or {}).get("results") or []
        returned_paths = {
            entry.get("path") for entry in search_results if entry.get("path")
        }
        expected_paths = set(task["expected_evidence_paths"])
        returned_evidence = len(expected_paths.intersection(returned_paths))
        search_protocol_checks = {
            "callCount": len(searches) == 1,
            "exitCode": search.get("exit_code") == 0,
            "query": search_args.get("query") == task["query"],
            "path": search_args.get("path") == ".",
            "include": search_args.get("include") == "*.rs",
            "limit": search_args.get("limit") == 5,
            "mode": search_args.get("mode") == "hybrid",
        }
        search_protocol_ok = all(search_protocol_checks.values())
        edit_protocol_checks = {
            "callCount": len(edits) == 1,
            "exitCode": edit.get("exit_code") == 0,
            "target": edit_args.get("file_path") == task["target_path"],
            "marker": edit_args.get("old_string") == TASK_MARKER,
        }
        edit_protocol_ok = all(edit_protocol_checks.values())
        completion_ok = (
            result is not None and result.text.strip() == COMPLETION_MARKER
        )
        tool_protocol_ok = (
            turn_failure is None
            and len(calls) == 2
            and result is not None
            and result.tool_calls_count == 2
            and search_protocol_ok
            and edit_protocol_ok
            and completion_ok
        )

        post_edit_status = status
        incremental_ready_ms: int | None = None
        incremental_failure: str | None = None
        if edit_protocol_ok:
            try:
                post_edit_status, incremental_ready_ms = (
                    await wait_for_incremental_ready(
                        session,
                        counters,
                        status,
                        initial_document_inputs,
                        evaluation["ready_timeout_seconds"],
                    )
                )
            except Exception as error:  # noqa: BLE001 - classify without source text
                incremental_failure = type(error).__name__
        post_edit_batching = post_edit_status["batching"]
        incremental_ready = incremental_ready_ms is not None

        close_started = time.monotonic()
        await session.close_async()
        close_ms = int((time.monotonic() - close_started) * 1000)
        closed = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
        released = (
            closed["phase"] == "closed"
            and closed["vector_records"] == 0
            and closed["vector_bytes"] == 0
        )
        integrity, target_digest = workspace_integrity(
            root, corpus["inventory"], task["target_path"]
        )
        write_hidden_test(root, task)
        cargo_passed, cargo_ms, cargo_failure = await asyncio.to_thread(
            cargo_test, root, evaluation["cargo_timeout_seconds"]
        )
        batch_limit_lower_bound = initial_batching["batch_limit_lower_bound"]
        if incremental_ready:
            batch_limit_lower_bound += post_edit_batching["batch_limit_lower_bound"]
        amplification = (
            counters["documentRequests"] / batch_limit_lower_bound
            if batch_limit_lower_bound
            else float("inf")
        )
        incremental_document_inputs = (
            counters["documentInputs"] - initial_document_inputs
        )
        infrastructure_gates = {
            "ready": (
                status["phase"] == "ready"
                and post_edit_status["phase"] == "ready"
            ),
            "incrementalReindex": incremental_ready,
            "revisionAdvanced": (
                post_edit_status["source_revision"] != status["source_revision"]
                and post_edit_status["vector_revision"] != status["vector_revision"]
            ),
            "fullCoverage": (
                status["coverage_bps"] == 10_000
                and post_edit_status["coverage_bps"] == 10_000
            ),
            "fileCoverage": (
                post_edit_status["eligible_files"] == corpus["textFileCount"]
                and post_edit_status["indexed_files"] == corpus["textFileCount"]
                and post_edit_status["failed_files"] == 0
            ),
            "batchingAgreement": (
                initial_batching["document_inputs"] == status["indexed_chunks"]
                and post_edit_batching["document_inputs"]
                == incremental_document_inputs
                and counters["documentInputs"]
                == initial_batching["document_inputs"]
                + post_edit_batching["document_inputs"]
            ),
            "queryAgreement": (
                counters["queryRequests"] == 1 and counters["queryInputs"] == 1
            ),
            "requestAmplification": amplification
            <= evaluation["maximum_document_request_amplification"],
            "nonTextEgress": counters["nonTextInputs"] == 0,
            "releasedAfterClose": released,
        }
        infrastructure_ok = all(infrastructure_gates.values())
        passed = (
            tool_protocol_ok
            and returned_evidence == len(expected_paths)
            and integrity
            and cargo_passed
            and infrastructure_ok
        )
        failure_kind = turn_failure or incremental_failure or cargo_failure
        if failure_kind is None and not tool_protocol_ok:
            failure_kind = "ToolProtocolViolation"
        if failure_kind is None and returned_evidence != len(expected_paths):
            failure_kind = "EvidenceCoverageFailure"
        if failure_kind is None and not integrity:
            failure_kind = "WorkspaceIntegrityFailure"
        if failure_kind is None and not infrastructure_ok:
            failure_kind = "InfrastructureGateFailure"
        return {
            "task": task["name"],
            "repetition": repetition,
            "passed": passed,
            "failureKind": failure_kind,
            "toolProtocolOk": tool_protocol_ok,
            "searchProtocolOk": search_protocol_ok,
            "searchProtocolChecks": search_protocol_checks,
            "editProtocolOk": edit_protocol_ok,
            "editProtocolChecks": edit_protocol_checks,
            "completionMarkerOk": completion_ok,
            "expectedEvidenceCount": len(expected_paths),
            "returnedEvidenceCount": returned_evidence,
            "returnedPaths": sorted(returned_paths),
            "workspaceIntegrity": integrity,
            "targetDigest": target_digest,
            "cargoPassed": cargo_passed,
            "cargoElapsedMs": cargo_ms,
            "sessionConstructionMs": construction_ms,
            "indexReadyMs": ready_ms,
            "incrementalReadyMs": incremental_ready_ms,
            "turnElapsedMs": turn_ms,
            "closeMs": close_ms,
            "totalTokens": 0 if result is None else result.total_tokens,
            "phase": post_edit_status["phase"],
            "coverageBps": post_edit_status["coverage_bps"],
            "corpusTextFiles": corpus["textFileCount"],
            "eligibleFiles": post_edit_status["eligible_files"],
            "indexedFiles": post_edit_status["indexed_files"],
            "failedFiles": post_edit_status["failed_files"],
            "indexedChunks": post_edit_status["indexed_chunks"],
            "vectorRecords": post_edit_status["vector_records"],
            "vectorBytes": post_edit_status["vector_bytes"],
            "initialSourceRevision": status["source_revision"],
            "postEditSourceRevision": post_edit_status["source_revision"],
            "initialVectorRevision": status["vector_revision"],
            "postEditVectorRevision": post_edit_status["vector_revision"],
            "documentProviderRequests": counters["documentRequests"],
            "initialBatchingDocumentInputs": initial_batching["document_inputs"],
            "incrementalBatchingDocumentInputs": post_edit_batching[
                "document_inputs"
            ],
            "providerDocumentInputs": counters["documentInputs"],
            "batchLimitLowerBound": batch_limit_lower_bound,
            "documentRequestAmplification": amplification,
            "timeToFirstReadyMs": initial_batching["time_to_first_ready_ms"],
            "postEditTimeToFirstReadyMs": post_edit_batching[
                "time_to_first_ready_ms"
            ],
            "nonTextProviderInputs": counters["nonTextInputs"],
            "releasedAfterClose": released,
            "infrastructureGates": infrastructure_gates,
        }


async def evaluate_generation(
    repetitions: int,
    selected_tasks: set[str] | None,
    local_files_only: bool,
) -> dict[str, Any]:
    validate_generation_fixture_contract()
    evaluation = FIXTURE["evaluation"]
    if repetitions < 1 or repetitions > evaluation["maximum_repetitions"]:
        raise ValueError("generation repetitions are outside the locked bounds")
    tasks = [
        task
        for task in FIXTURE["tasks"]
        if selected_tasks is None or task["name"] in selected_tasks
    ]
    if not tasks:
        raise ValueError("no generation tasks selected")
    embedding = FIXTURE["embedding"]
    model, dimension, model_load_ms = await asyncio.to_thread(
        load_sentence_transformer,
        embedding["model"],
        embedding["revision"],
        local_files_only,
    )
    if dimension != embedding["dimension"]:
        raise AssertionError(f"embedding dimension = {dimension}")
    evaluation_root = os.environ.get("A3S_REAL_EVAL_ROOT")
    if not evaluation_root:
        raise RuntimeError("A3S_REAL_EVAL_ROOT must point to the a3s monorepo root")
    config_path = Path(evaluation_root, ".a3s", "config.acl").resolve()
    if not config_path.is_file():
        raise RuntimeError("repository .a3s/config.acl is required")
    agent = await Agent.create_async(str(config_path))
    try:
        runs = []
        for repetition in range(repetitions):
            for task in tasks:
                runs.append(
                    await run_generation_task(
                        agent, model, dimension, task, repetition + 1
                    )
                )
    finally:
        await agent.close_async()
    summary = summarize_generation_runs(runs)
    all_task_names = {task["name"] for task in FIXTURE["tasks"]}
    gates = {
        "revisionLocked": bool(embedding["revision"]),
        "fullTaskMatrix": (
            {task["name"] for task in tasks} == all_task_names
            and repetitions >= evaluation["default_repetitions"]
        ),
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
    }
    return {
        "schemaVersion": FIXTURE["report_schema_version"],
        "fixtureId": FIXTURE["fixture_id"],
        "chatModel": FIXTURE["chat_model"],
        "embeddingModel": embedding["model"],
        "embeddingRevision": embedding["revision"],
        "embeddingDimension": dimension,
        "embeddingRuntime": {
            "pythonVersion": platform.python_version(),
            "sentenceTransformersVersion": importlib.metadata.version(
                "sentence-transformers"
            ),
            "transformersVersion": importlib.metadata.version("transformers"),
            "torchVersion": importlib.metadata.version("torch"),
            "device": str(model.device),
        },
        "modelLoadMs": model_load_ms,
        "localFilesOnly": local_files_only,
        "repetitions": repetitions,
        "taskNames": [task["name"] for task in tasks],
        "summary": summary,
        "gates": gates,
        "runs": runs,
        "allGatesPassed": all(gates.values()),
    }


def test_workspace_retrieval_generation_fixture_contract() -> None:
    validate_generation_fixture_contract()


def test_workspace_retrieval_generation_real_deepseek() -> None:
    if os.environ.get("A3S_REAL_GENERATION_EVAL") != "1":
        raise unittest.SkipTest("set A3S_REAL_GENERATION_EVAL=1 to run generation")
    report = asyncio.run(
        evaluate_generation(
            FIXTURE["evaluation"]["default_repetitions"],
            None,
            os.environ.get("A3S_REAL_EMBEDDING_LOCAL_ONLY") == "1",
        )
    )
    print(
        "WSR_GENERATION_EVAL="
        + json.dumps(report, ensure_ascii=False, separators=(",", ":"))
    )
    assert report["allGatesPassed"], report["gates"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validate-fixture", action="store_true")
    parser.add_argument("--local-files-only", action="store_true")
    parser.add_argument(
        "--repetitions",
        type=int,
        default=FIXTURE["evaluation"]["default_repetitions"],
    )
    parser.add_argument(
        "--task", action="append", choices=[task["name"] for task in FIXTURE["tasks"]]
    )
    parser.add_argument("--allow-unqualified", action="store_true")
    return parser.parse_args()


if __name__ == "__main__":
    arguments = parse_args()
    if arguments.validate_fixture:
        print(
            "Python generation fixture validated: "
            + json.dumps(validate_generation_fixture_contract(), sort_keys=True)
        )
        raise SystemExit(0)
    generation = asyncio.run(
        evaluate_generation(
            arguments.repetitions,
            None if arguments.task is None else set(arguments.task),
            arguments.local_files_only,
        )
    )
    print(
        "WSR_GENERATION_EVAL="
        + json.dumps(generation, ensure_ascii=False, separators=(",", ":"))
    )
    if not generation["allGatesPassed"] and not arguments.allow_unqualified:
        raise SystemExit("workspace retrieval generation gates failed")
