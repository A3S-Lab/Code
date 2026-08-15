"""Shared real Sentence Transformers support for workspace evaluations."""

from __future__ import annotations

import asyncio
import time
from typing import Any, cast

from a3s_code import (
    CallbackEmbeddingProvider,
    EmbeddingBatchRequest,
    EmbeddingBatchResponse,
    WorkspaceRetrievalStatus,
)


def load_sentence_transformer(
    model_id: str, revision: str, local_files_only: bool
) -> tuple[Any, int, int]:
    """Load a revision-locked optional model outside the SDK package."""

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


def sentence_transformer_provider(
    model: Any,
    model_id: str,
    revision: str,
    dimension: int,
    counters: dict[str, Any],
    *,
    query_id: str,
    non_text_sentinel: str,
) -> CallbackEmbeddingProvider:
    """Create a measured callback provider backed by a loaded local model."""

    async def embed(request: EmbeddingBatchRequest) -> EmbeddingBatchResponse:
        inputs = request["inputs"]
        is_query = all(item["id"] == query_id for item in inputs)
        is_document = all(item["id"] != query_id for item in inputs)
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
            non_text_sentinel in text for text in texts
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


async def wait_for_retrieval_ready(
    session: Any, timeout_seconds: float
) -> tuple[WorkspaceRetrievalStatus, int]:
    """Wait for a terminal retrieval build phase and require full readiness."""

    started = time.monotonic()
    status = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
    while status["phase"] == "building":
        if time.monotonic() - started >= timeout_seconds:
            raise TimeoutError("workspace retrieval did not become ready")
        await asyncio.sleep(0.01)
        status = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
    if status["phase"] != "ready":
        raise AssertionError(status)
    return status, int((time.monotonic() - started) * 1000)
