"""Shared fixture helpers for Python workspace-retrieval evaluations."""

from __future__ import annotations

import hashlib
import json
import math
import shutil
import tempfile
from pathlib import Path
from typing import Any


FIXTURE_PATH = (
    Path(__file__).resolve().parents[2]
    / "evaluation"
    / "workspace-retrieval-deepseek-v1.json"
)
FIXTURE: dict[str, Any] = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))

if FIXTURE["schema_version"] != 1 or FIXTURE["report_schema_version"] != 1:
    raise ValueError("unsupported workspace-retrieval evaluation fixture")


def generated_corpus_files() -> list[tuple[str, bytes, bool]]:
    """Return the complete corpus in stable repository-path order."""

    corpus = FIXTURE["corpus"]
    files = [
        (entry["path"], entry["content"].encode("utf-8"), True)
        for entry in corpus["source_files"]
    ]
    for index in range(corpus["unrelated_file_count"]):
        path = f"src/unrelated_{index:02}.rs"
        body = (
            f"pub fn unrelated_worker_{index:02}(value: usize) -> usize "
            f"{{ value + {index} }}\n"
        )
        files.append((path, body.encode("utf-8"), True))

    boundary = "".join(
        f"// deterministic chunk-boundary filler {index:02}\n"
        for index in range(corpus["boundary_filler_lines"])
    )
    boundary += "pub const MAX_PENDING_EMBED_BATCHES: usize = 8;\n\n"
    boundary += "pub fn admits_batch(pending: usize) -> bool {\n"
    boundary += "    pending < MAX_PENDING_EMBED_BATCHES\n}\n"
    files.append(("src/embedding_admission.rs", boundary.encode("utf-8"), True))
    files.extend(
        (entry["path"], entry["content"].encode("utf-8"), False)
        for entry in corpus["non_text_files"]
    )
    return sorted(files, key=lambda entry: entry[0])


def materialize_corpus(root: Path) -> str:
    """Write the locked corpus and return its path/content digest."""

    files = generated_corpus_files()
    digest = hashlib.sha256()
    for relative_path, content, _ in files:
        destination = root.joinpath(*relative_path.split("/"))
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        digest.update(relative_path.encode("utf-8"))
        digest.update(b"\0")
        digest.update(content)
        digest.update(b"\0")

    text_files = sum(1 for _, _, is_text in files if is_text)
    non_text_files = sum(1 for _, _, is_text in files if not is_text)
    if text_files != FIXTURE["corpus"]["text_file_count"]:
        raise AssertionError(f"text file count = {text_files}")
    if non_text_files != FIXTURE["corpus"]["non_text_file_count"]:
        raise AssertionError(f"non-text file count = {non_text_files}")
    return digest.hexdigest()


def validate_fixture_contract(prefix: str = "a3s-python-wsr-fixture-") -> str:
    """Materialize and verify the fixture without calling any model."""

    root = Path(tempfile.mkdtemp(prefix=prefix))
    try:
        digest = materialize_corpus(root)
        if digest != FIXTURE["corpus"]["expected_digest"]:
            raise AssertionError(
                f"fixture digest = {digest}, "
                f"want {FIXTURE['corpus']['expected_digest']}"
            )
        return digest
    finally:
        shutil.rmtree(root, ignore_errors=True)


def percentile(values: list[int], fraction: float) -> int:
    """Return the nearest-rank percentile used by all evaluation reports."""

    ordered = sorted(values)
    if not ordered:
        return 0
    return ordered[max(0, math.ceil(fraction * len(ordered)) - 1)]
