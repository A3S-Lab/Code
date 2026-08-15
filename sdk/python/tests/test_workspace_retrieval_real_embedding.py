"""Qualification runner for real local workspace embedding models."""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import os
import tempfile
import time
import unittest
from pathlib import Path
from typing import Any, cast

from a3s_code import (
    Agent,
    CallbackEmbeddingProvider,
    DeterministicWorkspaceReranker,
    EmbeddingBatchRequest,
    EmbeddingBatchResponse,
    RecursiveWorkspaceChunkingStrategy,
    SessionOptions,
    WorkspaceRetrievalOptions,
    WorkspaceRetrievalStatus,
)
from workspace_retrieval_eval_fixture import (
    FIXTURE,
    materialize_corpus,
    percentile,
    validate_fixture_contract,
)
from workspace_retrieval_embedding_models import (
    MODEL_MATRIX,
    assess_model_matrix,
    validate_model_matrix_contract,
)


REPORT_SCHEMA_VERSION = 1
READY_TIMEOUT_SECONDS = 60.0
MAX_READY_MS = 5_000
MAX_QUERY_P95_MS = 1_000
INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Workspace Retrieval Embedding Evaluation"
  }
}
""".strip()


def load_model(
    model_id: str, revision: str, local_files_only: bool
) -> tuple[Any, int, int]:
    """Load an optional Sentence Transformers model outside the SDK package."""

    try:
        from sentence_transformers import SentenceTransformer
    except ImportError as error:
        raise RuntimeError(
            "install sentence-transformers to run the real embedding evaluation"
        ) from error

    started = time.monotonic()
    model = SentenceTransformer(
        model_id,
        revision=revision or None,
        local_files_only=local_files_only,
    )
    load_ms = int((time.monotonic() - started) * 1000)
    dimension = model.get_sentence_embedding_dimension()
    if not isinstance(dimension, int) or dimension <= 0:
        raise RuntimeError(f"invalid sentence embedding dimension: {dimension}")
    return model, dimension, load_ms


def embedding_provider(
    model: Any,
    model_id: str,
    revision: str,
    dimension: int,
    counters: dict[str, Any],
) -> CallbackEmbeddingProvider:
    async def embed(request: EmbeddingBatchRequest) -> EmbeddingBatchResponse:
        inputs = request["inputs"]
        is_query = all(
            item["id"] == FIXTURE["embedding"]["query_id"] for item in inputs
        )
        is_document = all(
            item["id"] != FIXTURE["embedding"]["query_id"] for item in inputs
        )
        if not inputs or not (is_query or is_document):
            raise AssertionError("document and query inputs must not share a batch")

        texts = [item["text"] for item in inputs]
        counters["requests"] += 1
        counters["queryRequests"] += int(is_query)
        counters["documentRequests"] += int(is_document)
        counters["queryInputs"] += len(inputs) if is_query else 0
        counters["documentInputs"] += len(inputs) if is_document else 0
        counters["inputBytes"] += sum(len(text.encode("utf-8")) for text in texts)
        counters["nonTextInputs"] += sum(
            "NON_TEXT_ASSET_SENTINEL" in text for text in texts
        )

        started = time.monotonic()
        vectors = await asyncio.to_thread(
            model.encode,
            texts,
            normalize_embeddings=True,
            convert_to_numpy=True,
            show_progress_bar=False,
        )
        counters["latencyMs"].append(int((time.monotonic() - started) * 1000))
        if len(vectors) != len(inputs):
            raise AssertionError(
                f"embedding output count = {len(vectors)}, want {len(inputs)}"
            )
        return {
            "vectors": [
                {"id": item["id"], "values": vector.tolist()}
                for item, vector in zip(inputs, vectors)
            ]
        }

    return CallbackEmbeddingProvider(
        "sentence-transformers",
        model_id,
        dimension,
        embed,
        revision=revision,
        normalization="unit",
    )


async def wait_until_ready(
    session: Any,
) -> tuple[WorkspaceRetrievalStatus, int]:
    started = time.monotonic()
    status = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
    while status["phase"] == "building":
        if time.monotonic() - started >= READY_TIMEOUT_SECONDS:
            raise TimeoutError("workspace retrieval did not become ready")
        await asyncio.sleep(0.01)
        status = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
    if status["phase"] != "ready":
        raise AssertionError(status)
    return status, int((time.monotonic() - started) * 1000)


def expected_rank(hits: list[dict[str, Any]], expected_path: str) -> int | None:
    return next(
        (
            index + 1
            for index, hit in enumerate(hits)
            if hit["chunk"]["path"] == expected_path
        ),
        None,
    )


def quality(runs: list[dict[str, Any]], prefix: str) -> dict[str, float]:
    ranks = [run[f"{prefix}Rank"] for run in runs]
    relevant = sum(rank is not None and rank <= 5 for rank in ranks)
    return {
        f"{prefix}PrecisionAt5": relevant / (len(runs) * 5),
        f"{prefix}RecallAt5": relevant / len(runs),
        f"{prefix}Mrr": sum(
            0.0 if rank is None else 1.0 / rank for rank in ranks
        )
        / len(runs),
        f"{prefix}NdcgAt5": sum(
            0.0 if rank is None or rank > 5 else 1.0 / math.log2(rank + 1)
            for rank in ranks
        )
        / len(runs),
    }


async def evaluate(
    model_id: str,
    revision: str,
    local_files_only: bool,
    rerank_mode: str = "deterministic",
) -> dict[str, Any]:
    model, dimension, model_load_ms = await asyncio.to_thread(
        load_model, model_id, revision, local_files_only
    )
    counters: dict[str, Any] = {
        "requests": 0,
        "documentRequests": 0,
        "queryRequests": 0,
        "documentInputs": 0,
        "queryInputs": 0,
        "inputBytes": 0,
        "nonTextInputs": 0,
        "latencyMs": [],
    }
    provider = embedding_provider(model, model_id, revision, dimension, counters)
    if rerank_mode not in {"rrf-only", "deterministic"}:
        raise ValueError(f"unsupported rerank mode: {rerank_mode}")
    reranker = (
        DeterministicWorkspaceReranker()
        if rerank_mode == "deterministic"
        else None
    )
    retrieval = WorkspaceRetrievalOptions(
        provider,
        reranker,
        RecursiveWorkspaceChunkingStrategy(
            FIXTURE["chunking"]["target_bytes"],
            FIXTURE["chunking"]["overlap_bytes"],
            FIXTURE["chunking"]["separators"],
        ),
    )

    with tempfile.TemporaryDirectory(prefix="a3s-real-embedding-eval-") as root_text:
        root = Path(root_text)
        digest = materialize_corpus(root)
        if digest != FIXTURE["corpus"]["expected_digest"]:
            raise AssertionError(f"fixture digest = {digest}")

        agent = await Agent.create_async(INLINE_CONFIG)
        options = SessionOptions()
        options.workspace_retrieval = retrieval
        construction_started = time.monotonic()
        session = await agent.session_async(str(root), options)
        construction_ms = int((time.monotonic() - construction_started) * 1000)
        runs: list[dict[str, Any]] = []
        status: WorkspaceRetrievalStatus | None = None
        close_ms = 0
        released_after_close = False
        try:
            status, ready_ms = await wait_until_ready(session)
            for task in FIXTURE["tasks"]:
                request = {
                    "query": task["query"],
                    "path": ".",
                    "include": "*.rs",
                    "limit": 5,
                }
                semantic_started = time.monotonic()
                semantic = await session.semantic_search_async(request)
                semantic_ms = int((time.monotonic() - semantic_started) * 1000)
                hybrid_started = time.monotonic()
                hybrid = await session.hybrid_search_async(request)
                hybrid_ms = int((time.monotonic() - hybrid_started) * 1000)
                semantic_hits = semantic["hits"]
                hybrid_hits = hybrid["hits"]
                runs.append(
                    {
                        "task": task["name"],
                        "expectedPath": task["expected_path"],
                        "semanticRank": expected_rank(
                            semantic_hits, task["expected_path"]
                        ),
                        "hybridRank": expected_rank(
                            hybrid_hits, task["expected_path"]
                        ),
                        "semanticReturned": len(semantic_hits),
                        "hybridReturned": len(hybrid_hits),
                        "semanticMs": semantic_ms,
                        "hybridMs": hybrid_ms,
                        "semanticPaths": [
                            hit["chunk"]["path"] for hit in semantic_hits
                        ],
                        "hybridPaths": [hit["chunk"]["path"] for hit in hybrid_hits],
                        "algorithm": hybrid["rerank"]["algorithm"],
                    }
                )
        finally:
            close_started = time.monotonic()
            await session.close_async()
            close_ms = int((time.monotonic() - close_started) * 1000)
            closed = cast(
                WorkspaceRetrievalStatus, session.workspace_retrieval_status()
            )
            released_after_close = (
                closed["phase"] == "closed"
                and closed["vector_records"] == 0
                and closed["vector_bytes"] == 0
            )
            await agent.close_async()

        if status is None:
            raise AssertionError("retrieval status was not captured")
        batching = status["batching"]
        lower_bound = batching["batch_limit_lower_bound"]
        amplification = (
            batching["document_provider_requests"] / lower_bound
            if lower_bound
            else float("inf")
        )
        summary: dict[str, Any] = {
            **quality(runs, "semantic"),
            **quality(runs, "hybrid"),
            "modelLoadMs": model_load_ms,
            "sessionConstructionMs": construction_ms,
            "indexReadyMs": ready_ms,
            "timeToFirstReadyMs": batching["time_to_first_ready_ms"],
            "semanticP50Ms": percentile(
                [run["semanticMs"] for run in runs], 0.5
            ),
            "semanticP95Ms": percentile(
                [run["semanticMs"] for run in runs], 0.95
            ),
            "hybridP50Ms": percentile([run["hybridMs"] for run in runs], 0.5),
            "hybridP95Ms": percentile([run["hybridMs"] for run in runs], 0.95),
            "embeddingCallP50Ms": percentile(counters["latencyMs"], 0.5),
            "embeddingCallP95Ms": percentile(counters["latencyMs"], 0.95),
            "closeMs": close_ms,
            "documentRequestAmplification": amplification,
            "nonTextProviderInputs": counters["nonTextInputs"],
            "releasedAfterClose": released_after_close,
        }
        gates = {
            "revisionLocked": bool(revision),
            "fullCoverage": (
                status["coverage_bps"] == 10_000
                and status["eligible_files"] == FIXTURE["corpus"]["text_file_count"]
                and status["indexed_files"] == FIXTURE["corpus"]["text_file_count"]
                and status["indexed_chunks"]
                == FIXTURE["corpus"]["expected_chunk_count"]
                and status["failed_files"] == 0
            ),
            "semanticRecallAt5": summary["semanticRecallAt5"] == 1.0,
            "hybridRecallAt5": summary["hybridRecallAt5"] == 1.0,
            "readyLatency": ready_ms <= MAX_READY_MS,
            "queryLatency": summary["hybridP95Ms"] <= MAX_QUERY_P95_MS,
            "requestAmplification": amplification <= 1.10,
            "nonTextEgress": counters["nonTextInputs"] == 0,
            "releasedAfterClose": released_after_close,
        }
        normalized_batching = {
            "documentInputs": batching["document_inputs"],
            "documentTextBytes": batching["document_text_bytes"],
            "documentBatches": batching["document_batches"],
            "documentProviderRequests": batching["document_provider_requests"],
            "batchLimitLowerBound": batching["batch_limit_lower_bound"],
            "inputLimitFlushes": batching["input_limit_flushes"],
            "textByteLimitFlushes": batching["text_byte_limit_flushes"],
            "vectorByteLimitFlushes": batching["vector_byte_limit_flushes"],
            "generationCompleteFlushes": batching[
                "generation_complete_flushes"
            ],
            "timeToFirstReadyMs": batching["time_to_first_ready_ms"],
            "nonTextInputs": batching["non_text_inputs"],
        }
        return {
            "schemaVersion": REPORT_SCHEMA_VERSION,
            "fixtureId": FIXTURE["fixture_id"],
            "fixtureDigest": digest,
            "provider": "sentence-transformers",
            "model": model_id,
            "revision": revision or None,
            "dimension": dimension,
            "localFilesOnly": local_files_only,
            "chunking": FIXTURE["chunking"],
            "rerankMode": rerank_mode,
            "summary": summary,
            "status": {
                "phase": status["phase"],
                "eligibleFiles": status["eligible_files"],
                "indexedFiles": status["indexed_files"],
                "indexedChunks": status["indexed_chunks"],
                "vectorRecords": status["vector_records"],
                "vectorBytes": status["vector_bytes"],
                "batching": normalized_batching,
            },
            "providerMetrics": counters,
            "runs": runs,
            "gates": gates,
            "allGatesPassed": all(gates.values()),
        }


async def evaluate_matrix(local_files_only: bool) -> dict[str, Any]:
    reports: dict[str, dict[str, Any]] = {}
    for case in MODEL_MATRIX["cases"]:
        reports[case["name"]] = await evaluate(
            case["model"],
            case["revision"],
            local_files_only,
            case["rerank"],
        )
    assessment = assess_model_matrix(reports)
    return {
        "schemaVersion": MODEL_MATRIX["schema_version"],
        "matrixId": MODEL_MATRIX["matrix_id"],
        "sourceFixtureId": MODEL_MATRIX["source_fixture_id"],
        "localFilesOnly": local_files_only,
        "reports": reports,
        "assessment": assessment,
        "allGatesPassed": assessment["allGatesPassed"],
    }


def test_workspace_retrieval_real_embedding_fixture_contract() -> None:
    validate_fixture_contract("a3s-python-real-embedding-fixture-")
    validate_model_matrix_contract()


def test_workspace_retrieval_real_embedding_model() -> None:
    model_id = os.environ.get("A3S_REAL_EMBEDDING_MODEL")
    if not model_id:
        raise unittest.SkipTest(
            "set A3S_REAL_EMBEDDING_MODEL to run the real embedding evaluation"
        )
    report = asyncio.run(
        evaluate(
            model_id,
            os.environ.get("A3S_REAL_EMBEDDING_REVISION", ""),
            os.environ.get("A3S_REAL_EMBEDDING_LOCAL_ONLY") == "1",
            os.environ.get("A3S_REAL_EMBEDDING_RERANK", "deterministic"),
        )
    )
    print(
        "WSR_REAL_EMBEDDING_EVAL="
        + json.dumps(report, ensure_ascii=False, separators=(",", ":"))
    )
    assert report["allGatesPassed"], report["gates"]


def test_workspace_retrieval_real_embedding_model_matrix() -> None:
    if os.environ.get("A3S_REAL_EMBEDDING_MATRIX") != "1":
        raise unittest.SkipTest(
            "set A3S_REAL_EMBEDDING_MATRIX=1 to run the locked model matrix"
        )
    report = asyncio.run(
        evaluate_matrix(os.environ.get("A3S_REAL_EMBEDDING_LOCAL_ONLY") == "1")
    )
    print(
        "WSR_REAL_EMBEDDING_MATRIX="
        + json.dumps(report, ensure_ascii=False, separators=(",", ":"))
    )
    assert report["allGatesPassed"], report["assessment"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validate-fixture", action="store_true")
    parser.add_argument("--matrix", action="store_true")
    parser.add_argument("--model", default=os.environ.get("A3S_REAL_EMBEDDING_MODEL"))
    parser.add_argument(
        "--revision", default=os.environ.get("A3S_REAL_EMBEDDING_REVISION", "")
    )
    parser.add_argument("--local-files-only", action="store_true")
    parser.add_argument(
        "--rerank",
        choices=("rrf-only", "deterministic"),
        default=os.environ.get("A3S_REAL_EMBEDDING_RERANK", "deterministic"),
    )
    parser.add_argument("--allow-unqualified", action="store_true")
    return parser.parse_args()


if __name__ == "__main__":
    arguments = parse_args()
    if arguments.validate_fixture:
        print(
            "Python real embedding fixture validated: "
            + validate_fixture_contract("a3s-python-real-embedding-fixture-")
        )
        validate_model_matrix_contract()
    elif arguments.matrix:
        matrix_evaluation = asyncio.run(
            evaluate_matrix(arguments.local_files_only)
        )
        print(
            "WSR_REAL_EMBEDDING_MATRIX="
            + json.dumps(
                matrix_evaluation,
                ensure_ascii=False,
                separators=(",", ":"),
            )
        )
        if not matrix_evaluation["allGatesPassed"]:
            raise SystemExit("real embedding model matrix gates failed")
    else:
        if not arguments.model:
            raise SystemExit("--model or A3S_REAL_EMBEDDING_MODEL is required")
        evaluation = asyncio.run(
            evaluate(
                arguments.model,
                arguments.revision,
                arguments.local_files_only,
                arguments.rerank,
            )
        )
        print(
            "WSR_REAL_EMBEDDING_EVAL="
            + json.dumps(evaluation, ensure_ascii=False, separators=(",", ":"))
        )
        if not evaluation["allGatesPassed"] and not arguments.allow_unqualified:
            raise SystemExit("real embedding qualification gates failed")
