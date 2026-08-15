"""Cross-SDK real-DeepSeek qualification for Python workspace retrieval."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import sys
import tempfile
import time
import unittest
from pathlib import Path
from typing import Any, cast

from workspace_retrieval_eval_fixture import (
    FIXTURE,
    materialize_corpus,
    validate_fixture_contract,
)
from workspace_retrieval_deepseek_report import summarize

from a3s_code import (
    Agent,
    CallbackEmbeddingProvider,
    DeterministicWorkspaceReranker,
    EmbeddingBatchRequest,
    EmbeddingBatchResponse,
    PermissionPolicy,
    RecursiveWorkspaceChunkingStrategy,
    SessionOptions,
    WorkspaceRetrievalOptions,
    WorkspaceRetrievalStatus,
)


READY_TIMEOUT_SECONDS = 10.0
TURN_TIMEOUT_SECONDS = 240.0


def stable_bucket(text: str, buckets: int) -> int:
    value = 2166136261
    for byte in text.encode("utf-8"):
        value ^= byte
        value = (value * 16777619) & 0xFFFFFFFF
    return value % buckets


def vector_for(input_id: str, text: str) -> list[float]:
    tasks = FIXTURE["tasks"]
    if input_id == FIXTURE["embedding"]["query_id"]:
        matches = [index for index, task in enumerate(tasks) if text.strip() == task["query"]]
        assert len(matches) == 1, f"unexpected evaluation query: {text}"
        axis = matches[0]
    else:
        matches = [
            index
            for index, task in enumerate(tasks)
            if task["expected_identifier"] in text
        ]
        axis = (
            matches[0]
            if matches
            else len(tasks)
            + stable_bucket(text, FIXTURE["embedding"]["dimension"] - len(tasks))
        )
    vector = [0.0] * FIXTURE["embedding"]["dimension"]
    vector[axis] = 1.0
    return vector


def evaluation_provider(counters: dict[str, int]) -> CallbackEmbeddingProvider:
    async def embed(request: EmbeddingBatchRequest) -> EmbeddingBatchResponse:
        inputs = request["inputs"]
        query = all(item["id"] == FIXTURE["embedding"]["query_id"] for item in inputs)
        documents = all(item["id"] != FIXTURE["embedding"]["query_id"] for item in inputs)
        assert query or documents, "document and query inputs must not share a batch"
        counters["requests"] += 1
        counters["query_requests"] += int(query)
        counters["document_requests"] += int(documents)
        for item in inputs:
            is_query = item["id"] == FIXTURE["embedding"]["query_id"]
            counters["input_bytes"] += len(item["text"].encode("utf-8"))
            counters["query_inputs"] += int(is_query)
            counters["document_inputs"] += int(not is_query)
            counters["non_text_inputs"] += int(
                "NON_TEXT_ASSET_SENTINEL" in item["text"]
            )
        await asyncio.sleep(0)
        return {
            "vectors": [
                {
                    "id": item["id"],
                    "values": vector_for(item["id"], item["text"]),
                }
                for item in inputs
            ]
        }

    embedding = FIXTURE["embedding"]
    return CallbackEmbeddingProvider(
        embedding["provider"],
        embedding["model"],
        embedding["dimension"],
        embed,
        revision=embedding["revision"],
        normalization="unit",
    )


async def wait_until_ready(session: Any) -> tuple[WorkspaceRetrievalStatus, int]:
    started = time.monotonic()
    status = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
    while status["phase"] == "building":
        if time.monotonic() - started >= READY_TIMEOUT_SECONDS:
            raise TimeoutError("workspace retrieval did not become ready")
        await asyncio.sleep(0.01)
        status = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
    assert status["phase"] == "ready", status
    return status, int((time.monotonic() - started) * 1000)


def assert_ready_status(
    status: WorkspaceRetrievalStatus, counters: dict[str, int]
) -> None:
    corpus = FIXTURE["corpus"]
    batching = status["batching"]
    assert status["coverage_bps"] == 10_000
    assert status["eligible_files"] == corpus["text_file_count"]
    assert status["indexed_files"] == corpus["text_file_count"]
    assert status["indexed_chunks"] == corpus["expected_chunk_count"]
    assert status["failed_files"] == 0
    assert status["vector_records"] == corpus["expected_chunk_count"]
    assert batching["document_inputs"] == corpus["expected_chunk_count"]
    assert batching["document_provider_requests"] == 1
    assert batching["batch_limit_lower_bound"] == 1
    assert batching["non_text_inputs"] == 0
    assert batching["time_to_first_ready_ms"] is not None
    assert counters["document_requests"] == 1
    assert counters["document_inputs"] == corpus["expected_chunk_count"]
    assert counters["non_text_inputs"] == 0


def task_prompt(task: dict[str, str]) -> str:
    return (
        "Inspect the search tool schema. Make exactly one search call and no other "
        f"tool call. Use query exactly: {task['query']}. Set path to '.', include "
        "to '*.rs', limit to 5, and mode to 'hybrid'. After the result, return "
        "exactly the Rust function or constant declaration name that directly "
        "answers the query and is supported by the evidence, or NOT_FOUND when "
        "no relevant declaration is present. Never return a path, file stem, "
        "module name, prose, or Markdown."
    )


def normalized_answer(text: str) -> str:
    return text.strip().strip("`").strip()


async def run_task(agent: Any, task: dict[str, str], ordinal: int) -> dict[str, Any]:
    workspace = Path(tempfile.mkdtemp(prefix="a3s-python-wsr-real-"))
    session = None
    try:
        corpus_digest = materialize_corpus(workspace)
        assert corpus_digest == FIXTURE["corpus"]["expected_digest"]
        counters = {
            "requests": 0,
            "document_requests": 0,
            "query_requests": 0,
            "document_inputs": 0,
            "query_inputs": 0,
            "input_bytes": 0,
            "non_text_inputs": 0,
        }
        chunking = RecursiveWorkspaceChunkingStrategy(
            FIXTURE["chunking"]["target_bytes"],
            FIXTURE["chunking"]["overlap_bytes"],
            FIXTURE["chunking"]["separators"],
        )
        retrieval = WorkspaceRetrievalOptions(
            evaluation_provider(counters),
            DeterministicWorkspaceReranker(),
            chunking,
        )
        options = SessionOptions()
        options.session_id = f"wsr-sdk-python-{ordinal}"
        options.model = FIXTURE["chat_model"]
        options.planning_mode = "disabled"
        options.goal_tracking = False
        options.permission_policy = PermissionPolicy(
            allow=["search(*)"], default_decision="deny"
        )
        options.guidelines = (
            "This is a deterministic repository retrieval evaluation. Follow the "
            "requested one-tool protocol exactly. Never guess an identifier that "
            "is absent from the tool evidence."
        )
        options.max_parse_retries = 1
        options.max_tool_rounds = 2
        options.manual_delegation_enabled = False
        options.auto_parallel = False
        options.temperature = 0.0
        options.workspace_retrieval = retrieval
        construction_started = time.monotonic()
        session = await agent.session_async(str(workspace), options)
        session_construction_ms = int((time.monotonic() - construction_started) * 1000)
        status, index_ready_ms = await wait_until_ready(session)
        assert_ready_status(status, counters)
        turn_started = time.monotonic()
        result = await asyncio.wait_for(
            session.send_async(task_prompt(task)), timeout=TURN_TIMEOUT_SECONDS
        )
        turn_elapsed_ms = int((time.monotonic() - turn_started) * 1000)
        runs = await session.runs_async()
        assert len(runs) == 1
        assert runs[0]["status"] == "completed"
        events = await session.run_events_async(runs[0]["id"])
        calls = [event["payload"] for event in events if event["type"] == "tool_end"]
        call = calls[0] if calls else {}
        args = call.get("args") or {}
        metadata = call.get("metadata") or {}
        results = metadata.get("results") or []
        expected_indexes = [
            index
            for index, entry in enumerate(results)
            if entry.get("path") == task["expected_path"]
        ]
        expected_path_rank = expected_indexes[0] + 1 if expected_indexes else None
        protocol_ok = (
            result.tool_calls_count == 1
            and len(calls) == 1
            and call.get("name") == "search"
            and call.get("exit_code") == 0
            and args.get("query") == task["query"]
            and args.get("path") == "."
            and args.get("include") == "*.rs"
            and args.get("limit") == 5
            and args.get("mode") == "hybrid"
        )
        completion_correct = (
            normalized_answer(result.text) == task["expected_identifier"]
        )
        assert protocol_ok, calls
        assert completion_correct, result.text
        assert expected_path_rank is not None, results
        rerank = metadata.get("rerank") or {}
        assert rerank.get("requestedMode") == FIXTURE["rerank"]["requested_mode"]
        assert rerank.get("appliedMode") == FIXTURE["rerank"]["requested_mode"]
        assert metadata.get("algorithm") == FIXTURE["rerank"]["algorithm"]
        assert counters["query_requests"] == 1
        assert counters["query_inputs"] == 1
        close_started = time.monotonic()
        await session.close_async()
        close_ms = int((time.monotonic() - close_started) * 1000)
        closed = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
        assert closed["phase"] == "closed"
        assert closed["vector_records"] == 0
        assert closed["vector_bytes"] == 0
        return {
            "task": task["name"],
            "completionCorrect": completion_correct,
            "toolProtocolOk": protocol_ok,
            "expectedPathRank": expected_path_rank,
            "resultCount": len(results),
            "algorithm": metadata.get("algorithm"),
            "rerankRequestedMode": rerank.get("requestedMode"),
            "rerankAppliedMode": rerank.get("appliedMode"),
            "sessionConstructionMs": session_construction_ms,
            "indexReadyMs": index_ready_ms,
            "turnElapsedMs": turn_elapsed_ms,
            "closeMs": close_ms,
            "promptTokens": result.prompt_tokens,
            "completionTokens": result.completion_tokens,
            "totalTokens": result.total_tokens,
            "phase": status["phase"],
            "coverageBps": status["coverage_bps"],
            "eligibleFiles": status["eligible_files"],
            "indexedFiles": status["indexed_files"],
            "indexedChunks": status["indexed_chunks"],
            "vectorRecords": status["vector_records"],
            "vectorBytes": status["vector_bytes"],
            "batching": {
                "documentInputs": status["batching"]["document_inputs"],
                "documentTextBytes": status["batching"]["document_text_bytes"],
                "documentBatches": status["batching"]["document_batches"],
                "documentProviderRequests": status["batching"][
                    "document_provider_requests"
                ],
                "batchLimitLowerBound": status["batching"]["batch_limit_lower_bound"],
                "inputLimitFlushes": status["batching"]["input_limit_flushes"],
                "textByteLimitFlushes": status["batching"][
                    "text_byte_limit_flushes"
                ],
                "vectorByteLimitFlushes": status["batching"][
                    "vector_byte_limit_flushes"
                ],
                "generationCompleteFlushes": status["batching"][
                    "generation_complete_flushes"
                ],
                "timeToFirstReadyMs": status["batching"]["time_to_first_ready_ms"],
                "nonTextInputs": status["batching"]["non_text_inputs"],
            },
            "provider": {
                "requests": counters["requests"],
                "documentRequests": counters["document_requests"],
                "queryRequests": counters["query_requests"],
                "documentInputs": counters["document_inputs"],
                "queryInputs": counters["query_inputs"],
                "inputBytes": counters["input_bytes"],
                "nonTextInputs": counters["non_text_inputs"],
            },
            "releasedAfterClose": True,
        }
    finally:
        if session is not None:
            current = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
            if current["phase"] != "closed":
                await session.close_async()
        shutil.rmtree(workspace, ignore_errors=True)


async def evaluate() -> dict[str, Any]:
    evaluation_root = os.environ.get("A3S_REAL_EVAL_ROOT")
    if not evaluation_root:
        raise RuntimeError("A3S_REAL_EVAL_ROOT must point to the a3s monorepo root")
    config_path = Path(evaluation_root, ".a3s", "config.acl").resolve()
    if not config_path.is_file():
        raise RuntimeError("repository .a3s/config.acl is required")
    agent = await Agent.create_async(str(config_path))
    try:
        runs = [
            await run_task(agent, task, ordinal)
            for ordinal, task in enumerate(FIXTURE["tasks"])
        ]
        summary = summarize(runs)
        assert summary["taskAccuracy"] == 1
        assert summary["toolProtocolRate"] == 1
        assert summary["recallAt5"] == 1
        assert summary["documentRequestAmplification"] <= 1.1
        assert summary["nonTextProviderInputs"] == 0
        assert summary["releasedAfterCloseRate"] == 1
        return {
            "schemaVersion": FIXTURE["report_schema_version"],
            "fixtureId": FIXTURE["fixture_id"],
            "fixtureDigest": FIXTURE["corpus"]["expected_digest"],
            "sdk": "python",
            "chatModel": FIXTURE["chat_model"],
            "chunking": FIXTURE["chunking"],
            "rerank": FIXTURE["rerank"],
            "summary": summary,
            "runs": runs,
            "allGatesPassed": True,
        }
    finally:
        await agent.close_async()


def test_workspace_retrieval_real_fixture_contract() -> None:
    validate_fixture_contract()


def test_workspace_retrieval_real_deepseek() -> None:
    if not os.environ.get("A3S_REAL_EVAL_ROOT"):
        raise unittest.SkipTest("set A3S_REAL_EVAL_ROOT to run the DeepSeek evaluation")
    report = asyncio.run(evaluate())
    print(f"WSR_SDK_DEEPSEEK_EVAL={json.dumps(report, ensure_ascii=False, separators=(',', ':'))}")


if __name__ == "__main__":
    if "--validate-fixture" in sys.argv:
        print(f"Python workspace retrieval fixture validated: {validate_fixture_contract()}")
    else:
        report = asyncio.run(evaluate())
        print(f"WSR_SDK_DEEPSEEK_EVAL={json.dumps(report, ensure_ascii=False, separators=(',', ':'))}")
