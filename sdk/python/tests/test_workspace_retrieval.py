"""Deterministic asyncio integration coverage for workspace retrieval."""

from __future__ import annotations

import asyncio
import tempfile
from pathlib import Path
from typing import cast

from a3s_code import (
    Agent,
    CallbackEmbeddingProvider,
    EmbeddingBatchRequest,
    EmbeddingBatchResponse,
    SessionOptions,
    WorkspaceHybridSearchResult,
    WorkspaceRetrievalOptions,
    WorkspaceRetrievalStatus,
    WorkspaceSemanticSearchResult,
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

            async def embed(request: EmbeddingBatchRequest) -> EmbeddingBatchResponse:
                nonlocal provider_calls
                provider_calls += 1
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

                semantic = cast(
                    WorkspaceSemanticSearchResult,
                    await session.semantic_search_async(
                        {"query": "cleanup session resources", "limit": 3}
                    ),
                )
                assert semantic["hits"][0]["chunk"]["path"] == "src/session_cleanup.rs"
                assert semantic["hits"][0]["chunk"]["digest_verified"] is True

                hybrid = cast(
                    WorkspaceHybridSearchResult,
                    await session.hybrid_search_async(
                        {"query": "terminate_owned_tasks", "limit": 3}
                    ),
                )
                assert hybrid["hits"][0]["chunk"]["path"] == "src/session_cleanup.rs"
                assert hybrid["hits"][0]["exact_identifier"] is True
                assert any(
                    rank["channel"] == "exact"
                    for rank in hybrid["hits"][0]["channels"]
                )
                assert provider_calls >= 2
            finally:
                await session.close_async()

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
            await agent.close_async()

    asyncio.run(scenario())
