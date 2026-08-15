"""Runtime helpers for the compile-gated generation evaluation."""

from __future__ import annotations

import asyncio
import hashlib
import json
import subprocess
import time
from pathlib import Path
from typing import Any, cast

from a3s_code import WorkspaceRetrievalStatus
from workspace_retrieval_generation_fixture import TASK_MARKER


COMPLETION_MARKER = "GENERATION_OK"


def embedding_counters() -> dict[str, Any]:
    return {
        "requests": 0,
        "documentRequests": 0,
        "queryRequests": 0,
        "documentInputs": 0,
        "queryInputs": 0,
        "inputBytes": 0,
        "nonTextInputs": 0,
        "latencyMs": [],
    }


def task_prompt(task: dict[str, Any]) -> str:
    search_arguments = json.dumps(
        {
            "query": task["query"],
            "path": ".",
            "include": "*.rs",
            "limit": 5,
            "mode": "hybrid",
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
    return (
        "Complete one compile-gated Rust function using repository evidence. "
        "Follow this protocol exactly:\n"
        "1. Inspect the search schema and make exactly one search call with this "
        f"exact argument object: {search_arguments}. Do not omit a field as a "
        "default and do not alter any value.\n"
        "2. Use the returned snippets as the only policy evidence. Make exactly "
        f"one edit call on {task['target_path']}. Set old_string exactly to "
        f"{TASK_MARKER!r} and replace it with a complete Rust expression or block "
        "that preserves the existing signature. Do not change any other text or "
        "file.\n"
        f"3. After the edit succeeds, return exactly {COMPLETION_MARKER}. Make no "
        "other tool calls.\n\n"
        "The current target file is shown only to define the edit boundary:\n"
        f"{task['target_source']}"
    )


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def workspace_integrity(
    root: Path, inventory: dict[str, str], target_path: str
) -> tuple[bool, str | None]:
    observed = {
        path.relative_to(root).as_posix(): _sha256_file(path)
        for path in root.rglob("*")
        if path.is_file()
    }
    target = root.joinpath(*target_path.split("/"))
    target_digest = observed.get(target_path)
    unchanged = all(
        observed.get(path) == digest
        for path, digest in inventory.items()
        if path != target_path
    )
    exact_inventory = set(observed) == set(inventory)
    try:
        target_text = target.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        target_text = ""
    scoped_target = (
        target_digest is not None
        and target_digest != inventory[target_path]
        and TASK_MARKER not in target_text
        and len(target_text.encode("utf-8")) <= 16 * 1024
    )
    return exact_inventory and unchanged and scoped_target, target_digest


def cargo_test(root: Path, timeout_seconds: int) -> tuple[bool, int, str | None]:
    started = time.monotonic()
    try:
        result = subprocess.run(
            ["cargo", "test", "--offline", "--quiet"],
            cwd=root,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout_seconds,
            check=False,
        )
        failure_kind = None if result.returncode == 0 else "CargoTestFailed"
        return (
            result.returncode == 0,
            int((time.monotonic() - started) * 1000),
            failure_kind,
        )
    except subprocess.TimeoutExpired:
        return False, int((time.monotonic() - started) * 1000), "CargoTimeout"
    except OSError:
        return False, int((time.monotonic() - started) * 1000), "CargoUnavailable"


def event_payloads(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [event["payload"] for event in events if event["type"] == "tool_end"]


async def wait_for_incremental_ready(
    session: Any,
    counters: dict[str, Any],
    initial_status: WorkspaceRetrievalStatus,
    initial_document_inputs: int,
    timeout_seconds: float,
) -> tuple[WorkspaceRetrievalStatus, int]:
    """Require the edited source generation to publish a replacement vector."""

    started = time.monotonic()
    while time.monotonic() - started < timeout_seconds:
        status = cast(WorkspaceRetrievalStatus, session.workspace_retrieval_status())
        revision_advanced = (
            status["source_revision"] != initial_status["source_revision"]
            and status["vector_revision"] != initial_status["vector_revision"]
        )
        if (
            revision_advanced
            and status["phase"] == "ready"
            and counters["documentInputs"] > initial_document_inputs
        ):
            return status, int((time.monotonic() - started) * 1000)
        await asyncio.sleep(0.01)
    raise TimeoutError("edited workspace generation did not become ready")
