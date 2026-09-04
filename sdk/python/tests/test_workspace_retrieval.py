"""Deterministic asyncio integration coverage for workspace retrieval."""

from __future__ import annotations

import asyncio
import json
import tempfile
from pathlib import Path
from typing import cast

from a3s_code import (
    Agent,
    CallbackEmbeddingProvider,
    DeterministicWorkspaceReranker,
    EmbeddingBatchRequest,
    EmbeddingBatchResponse,
    FixedWindowWorkspaceChunkingStrategy,
    LineWorkspaceChunkingStrategy,
    RecursiveWorkspaceChunkingStrategy,
    SessionOptions,
    WorkspaceHybridSearchResult,
    WorkspaceRetrievalOptions,
    WorkspaceLexicalEngineOption,
    WorkspaceRetrievalStatus,
    WorkspaceSemanticSearchResult,
)


CHUNKING_FIXTURE = json.loads(
    (
        Path(__file__).resolve().parents[3]
        / "core"
        / "tests"
        / "fixtures"
        / "workspace-chunking-sdk-v1.json"
    ).read_text(encoding="utf-8")
)


INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
""".strip()


def vector_for(text: str) -> list[float]:
    lower = text.lower()
    if "cleanup" in lower or "release every session resource" in lower:
        return [1.0, 0.0, 0.0, 0.0]
    return [0.0, 1.0, 0.0, 0.0]


def test_async_workspace_retrieval_lifecycle() -> None:
    async def scenario() -> None:
        with tempfile.TemporaryDirectory(prefix="a3s-python-retrieval-") as root:
            workspace = Path(root, "workspace")
            source = workspace / "src" / "session_cleanup.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "pub fn terminate_owned_tasks() {\n"
                "    // release every session resource\n"
                "}\n",
                encoding="utf-8",
            )

            provider_calls = 0
            provider_inputs: list[str] = []

            async def embed(request: EmbeddingBatchRequest) -> EmbeddingBatchResponse:
                nonlocal provider_calls
                provider_calls += 1
                provider_inputs.extend(item["text"] for item in request["inputs"])
                await asyncio.sleep(0)
                return {
                    "vectors": [
                        {"id": item["id"], "values": vector_for(item["text"])}
                        for item in request["inputs"]
                    ]
                }

            provider = CallbackEmbeddingProvider(
                "python-fixture",
                "deterministic-v1",
                4,
                embed,
                normalization="unit",
            )
            retrieval = WorkspaceRetrievalOptions(provider)
            assert retrieval.lexical_engine == WorkspaceLexicalEngineOption.ZvecRust
            retrieval.lexical_engine = WorkspaceLexicalEngineOption.ZvecRust
            assert retrieval.lexical_engine == WorkspaceLexicalEngineOption.ZvecRust
            retrieval.lexical_engine = WorkspaceLexicalEngineOption.Portable
            retrieval.max_records = 100
            retrieval.max_bytes = 1024 * 1024
            options = SessionOptions()
            options.workspace_retrieval = retrieval

            agent = await Agent.create_async(INLINE_CONFIG)
            try:
                agent.session(str(workspace), options)
            except RuntimeError as error:
                assert "requires session_async()" in str(error)
            else:
                raise AssertionError(
                    "synchronous session creation must reject an asyncio-bound provider"
                )
            session = await agent.session_async(str(workspace), options)
            try:
                status = cast(
                    WorkspaceRetrievalStatus, session.workspace_retrieval_status()
                )
                assert status["phase"] != "disabled"
                async def wait_until_ready() -> WorkspaceRetrievalStatus:
                    observed = status
                    while observed["phase"] == "building":
                        await asyncio.sleep(0.02)
                        observed = cast(
                            WorkspaceRetrievalStatus,
                            session.workspace_retrieval_status(),
                        )
                    return observed

                status = await asyncio.wait_for(wait_until_ready(), timeout=10)
                assert status["phase"] == "ready", status
                assert status["indexed_chunks"] > 0
                assert status["vector_records"] == status["indexed_chunks"]
                assert status["lexical_engine"] == "portable_bm25_v1"
                batching = status["batching"]
                assert batching["document_inputs"] > 0
                assert batching["document_text_bytes"] > 0
                assert batching["batch_limit_lower_bound"] > 0
                assert (
                    batching["document_provider_requests"] * 10
                    <= batching["batch_limit_lower_bound"] * 11
                )
                assert batching["non_text_inputs"] == 0
                assert batching["time_to_first_ready_ms"] is not None

                semantic = cast(
                    WorkspaceSemanticSearchResult,
                    await session.semantic_search_async(
                        {"query": "cleanup session resources", "limit": 3}
                    ),
                )
                assert semantic["hits"][0]["chunk"]["path"] == "src/session_cleanup.rs"
                assert semantic["hits"][0]["chunk"]["digest_verified"] is True
                assert semantic["status"]["phase"] == "ready"
                assert semantic["status"]["lexical_engine"] == "portable_bm25_v1"

                hybrid = cast(
                    WorkspaceHybridSearchResult,
                    await session.hybrid_search_async(
                        {"query": "terminate_owned_tasks", "limit": 3}
                    ),
                )
                assert hybrid["hits"][0]["chunk"]["path"] == "src/session_cleanup.rs"
                assert hybrid["hits"][0]["exact_identifier"] is True
                assert (
                    hybrid["hits"][0]["rerank_score"]
                    == hybrid["hits"][0]["fused_score"]
                )
                assert hybrid["hits"][0]["redundancy_score"] == 0.0
                assert any(
                    rank["channel"] == "exact"
                    for rank in hybrid["hits"][0]["channels"]
                )
                assert hybrid["rerank"]["requested_mode"] == "rrf_only"
                assert hybrid["rerank"]["applied_mode"] == "rrf_only"
                assert hybrid["rerank"]["algorithm"] == "rrf_k60"
                assert hybrid["rerank"]["fallback"] is None
                assert provider_calls >= 2
            finally:
                await session.close_async()

            line_chunking = LineWorkspaceChunkingStrategy()
            assert repr(line_chunking) == "LineWorkspaceChunkingStrategy()"
            fixed_case = next(
                case
                for case in CHUNKING_FIXTURE["cases"]
                if case["name"] == "fixed_window"
            )
            fixed_chunking = FixedWindowWorkspaceChunkingStrategy(
                fixed_case["target_bytes"], fixed_case["overlap_bytes"]
            )
            assert fixed_chunking.target_bytes == fixed_case["target_bytes"]
            assert fixed_chunking.overlap_bytes == fixed_case["overlap_bytes"]
            recursive_case = next(
                case
                for case in CHUNKING_FIXTURE["cases"]
                if case["name"] == "recursive"
            )
            recursive_chunking = RecursiveWorkspaceChunkingStrategy(
                recursive_case["target_bytes"],
                recursive_case["overlap_bytes"],
                recursive_case["separators"],
            )
            assert recursive_chunking.separators == recursive_case["separators"]
            WorkspaceRetrievalOptions(provider, None, line_chunking)
            WorkspaceRetrievalOptions(provider, None, recursive_chunking)
            for invalid in CHUNKING_FIXTURE["invalid_windows"]:
                try:
                    FixedWindowWorkspaceChunkingStrategy(
                        invalid["target_bytes"], invalid["overlap_bytes"]
                    )
                except ValueError:
                    pass
                else:
                    raise AssertionError(f"invalid chunk window accepted: {invalid}")
            try:
                RecursiveWorkspaceChunkingStrategy(64, 0, [])
            except ValueError as error:
                assert "separators" in str(error)
            else:
                raise AssertionError("empty recursive separators must fail")
            try:
                WorkspaceRetrievalOptions(provider, None, "fixed_window")
            except TypeError:
                pass
            else:
                raise AssertionError("primitive chunking selectors must fail")

            chunk_workspace = Path(root, "chunk-workspace")
            chunk_workspace.mkdir()
            (chunk_workspace / "fixture.txt").write_text(
                fixed_case["content"], encoding="utf-8"
            )
            input_offset = len(provider_inputs)
            chunk_options = SessionOptions()
            chunk_options.workspace_retrieval = WorkspaceRetrievalOptions(
                provider, None, fixed_chunking
            )
            chunk_session = await agent.session_async(
                str(chunk_workspace), chunk_options
            )
            try:
                chunk_status = cast(
                    WorkspaceRetrievalStatus,
                    chunk_session.workspace_retrieval_status(),
                )
                while chunk_status["phase"] == "building":
                    await asyncio.sleep(0.02)
                    chunk_status = cast(
                        WorkspaceRetrievalStatus,
                        chunk_session.workspace_retrieval_status(),
                    )
                assert chunk_status["phase"] == "ready", chunk_status
                assert chunk_status["indexed_chunks"] == len(fixed_case["ranges"])
                expected_texts = [
                    fixed_case["content"][item["start"] : item["end"]]
                    for item in fixed_case["ranges"]
                ]
                assert provider_inputs[input_offset:] == expected_texts
            finally:
                await chunk_session.close_async()

            reranker = DeterministicWorkspaceReranker()
            assert reranker.max_candidates == 100
            assert reranker.max_feature_bytes_per_candidate == 4096
            assert reranker.max_fingerprints_per_candidate == 128
            assert reranker.max_scratch_bytes == 4 * 1024 * 1024
            reranked_options = SessionOptions()
            reranked_options.workspace_retrieval = WorkspaceRetrievalOptions(
                provider, reranker
            )
            reranked_session = await agent.session_async(
                str(workspace), reranked_options
            )
            try:
                reranked_status = cast(
                    WorkspaceRetrievalStatus,
                    reranked_session.workspace_retrieval_status(),
                )
                while reranked_status["phase"] == "building":
                    await asyncio.sleep(0.02)
                    reranked_status = cast(
                        WorkspaceRetrievalStatus,
                        reranked_session.workspace_retrieval_status(),
                    )
                assert reranked_status["phase"] == "ready", reranked_status
                reranked = cast(
                    WorkspaceHybridSearchResult,
                    await reranked_session.hybrid_search_async(
                        {"query": "terminate_owned_tasks", "limit": 3}
                    ),
                )
                assert reranked["rerank"]["requested_mode"] == "deterministic"
                assert reranked["rerank"]["applied_mode"] == "deterministic"
                assert (
                    reranked["rerank"]["algorithm"]
                    == "rrf_k60+deterministic_mmr_v1"
                )
                assert reranked["rerank"]["accounted_scratch_bytes"] > 0
                assert reranked["rerank"]["fallback"] is None
            finally:
                await reranked_session.close_async()

            invalid_reranker = DeterministicWorkspaceReranker()
            invalid_reranker.max_candidates = 0
            invalid_options = SessionOptions()
            invalid_options.workspace_retrieval = WorkspaceRetrievalOptions(
                provider, invalid_reranker
            )
            calls_before_invalid = provider_calls
            try:
                await agent.session_async(str(workspace), invalid_options)
            except ValueError as error:
                assert "rerank.max_candidates" in str(error)
            else:
                raise AssertionError("invalid reranker bounds must fail")
            assert provider_calls == calls_before_invalid

            try:
                WorkspaceRetrievalOptions(provider, "deterministic")
            except TypeError:
                pass
            else:
                raise AssertionError("primitive reranker selectors must fail")

            provider_started = asyncio.Event()
            provider_cancelled = asyncio.Event()

            async def slow_embed(
                request: EmbeddingBatchRequest,
            ) -> EmbeddingBatchResponse:
                del request
                provider_started.set()
                try:
                    await asyncio.Future()
                except asyncio.CancelledError:
                    provider_cancelled.set()
                    raise
                raise AssertionError("unreachable")

            slow_options = SessionOptions()
            slow_options.workspace_retrieval = WorkspaceRetrievalOptions(
                CallbackEmbeddingProvider(
                    "python-fixture",
                    "slow-v1",
                    4,
                    slow_embed,
                    normalization="unit",
                )
            )
            slow_session = await agent.session_async(str(workspace), slow_options)
            await asyncio.wait_for(provider_started.wait(), timeout=10)
            await slow_session.close_async()
            await asyncio.wait_for(provider_cancelled.wait(), timeout=2)
            closed = cast(
                WorkspaceRetrievalStatus,
                slow_session.workspace_retrieval_status(),
            )
            assert closed["phase"] == "closed"
            assert closed["vector_records"] == 0
            assert closed["vector_bytes"] == 0
            await agent.close_async()

    asyncio.run(scenario())
