"""Versioned fixture helpers for retrieval-dependent code generation."""

from __future__ import annotations

import hashlib
import json
import shutil
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any


FIXTURE_PATH = (
    Path(__file__).resolve().parents[2]
    / "evaluation"
    / "workspace-retrieval-generation-v1.json"
)
FIXTURE: dict[str, Any] = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))
TASK_MARKER = 'unimplemented!("GENERATION_TASK")'

if FIXTURE["schema_version"] != 1 or FIXTURE["report_schema_version"] != 1:
    raise ValueError("unsupported workspace-retrieval generation fixture")


def _safe_relative_path(value: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if (
        not value
        or "\\" in value
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        raise ValueError(f"unsafe generation fixture path: {value!r}")
    return path


def _task_files(task: dict[str, Any]) -> list[tuple[str, bytes, bool]]:
    modules = ["solution"]
    source_files: list[tuple[str, bytes, bool]] = []
    for entry in task["source_files"]:
        path = _safe_relative_path(entry["path"])
        if path.suffix != ".rs" or path.parent != PurePosixPath("src"):
            raise ValueError(f"generation source must be a direct src/*.rs file: {path}")
        modules.append(path.stem)
        filler = "".join(
            f"// deterministic chunk-boundary filler {index:02}\n"
            for index in range(entry.get("generated_boundary_filler_lines", 0))
        )
        source_files.append(
            (entry["path"], (filler + entry["content"]).encode("utf-8"), True)
        )

    lib_source = "".join(f"pub mod {module};\n" for module in sorted(modules))
    files: list[tuple[str, bytes, bool]] = [
        (
            "Cargo.toml",
            (
                "[package]\n"
                'name = "wsr-generation-fixture"\n'
                'version = "0.1.0"\n'
                'edition = "2021"\n\n'
                "[lib]\n"
                'path = "src/lib.rs"\n'
            ).encode("utf-8"),
            True,
        ),
        ("src/lib.rs", lib_source.encode("utf-8"), True),
        (task["target_path"], task["target_source"].encode("utf-8"), True),
        *source_files,
    ]
    for index in range(FIXTURE["corpus"]["distractor_file_count"]):
        files.append(
            (
                f"src/distractor_{index:02}.rs",
                (
                    f"pub fn background_worker_{index:02}(value: usize) -> usize "
                    f"{{ value.wrapping_add({index}) }}\n"
                ).encode("utf-8"),
                True,
            )
        )
    files.extend(
        (entry["path"], entry["content"].encode("utf-8"), False)
        for entry in FIXTURE["corpus"]["non_text_files"]
    )
    return sorted(files, key=lambda entry: entry[0])


def task_digest(task: dict[str, Any]) -> str:
    """Return the stable digest for one task's model-visible corpus."""

    digest = hashlib.sha256()
    for relative_path, content, _ in _task_files(task):
        digest.update(relative_path.encode("utf-8"))
        digest.update(b"\0")
        digest.update(content)
        digest.update(b"\0")
    return digest.hexdigest()


def materialize_task(root: Path, task: dict[str, Any]) -> dict[str, Any]:
    """Write one model-visible task corpus and return locked inventory data."""

    inventory: dict[str, str] = {}
    text_files = 0
    non_text_files = 0
    for relative_path, content, is_text in _task_files(task):
        safe_path = _safe_relative_path(relative_path)
        destination = root.joinpath(*safe_path.parts)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        inventory[relative_path] = hashlib.sha256(content).hexdigest()
        text_files += int(is_text)
        non_text_files += int(not is_text)
    return {
        "digest": task_digest(task),
        "inventory": inventory,
        "textFileCount": text_files,
        "nonTextFileCount": non_text_files,
    }


def write_hidden_test(root: Path, task: dict[str, Any]) -> None:
    """Materialize the independent compile oracle after the agent has closed."""

    path = _safe_relative_path(task["hidden_test_path"])
    if not path.parts or path.parts[0] != "tests" or path.suffix != ".rs":
        raise ValueError("hidden generation tests must be Rust files below tests/")
    destination = root.joinpath(*path.parts)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(task["hidden_test_source"], encoding="utf-8")


def validate_generation_fixture_contract() -> dict[str, str]:
    """Validate paths, labels, model-visible digests, and hidden-oracle isolation."""

    names: set[str] = set()
    digests: dict[str, str] = {}
    for task in FIXTURE["tasks"]:
        name = task["name"]
        if not name or name in names:
            raise ValueError(f"duplicate or empty generation task name: {name!r}")
        names.add(name)
        target = _safe_relative_path(task["target_path"])
        if target != PurePosixPath("src/solution.rs"):
            raise ValueError(f"unexpected generation target: {target}")
        if task["target_source"].count(TASK_MARKER) != 1:
            raise ValueError(f"task {name} must contain exactly one generation marker")
        source_paths = {entry["path"] for entry in task["source_files"]}
        expected_paths = set(task["expected_evidence_paths"])
        if len(expected_paths) < 2 or not expected_paths.issubset(source_paths):
            raise ValueError(f"task {name} has invalid evidence labels")
        if task["hidden_test_path"] in source_paths:
            raise ValueError(f"task {name} exposes its hidden compile oracle")
        digest = task_digest(task)
        if digest != task.get("expected_digest"):
            raise AssertionError(
                f"task {name} digest = {digest}, want {task.get('expected_digest')}"
            )
        digests[name] = digest

    root = Path(tempfile.mkdtemp(prefix="a3s-wsr-generation-fixture-"))
    try:
        for task in FIXTURE["tasks"]:
            task_root = root / task["name"]
            observed = materialize_task(task_root, task)
            if observed["digest"] != digests[task["name"]]:
                raise AssertionError(f"materialized digest drifted for {task['name']}")
            if task["hidden_test_path"] in observed["inventory"]:
                raise AssertionError(f"hidden test leaked into {task['name']} corpus")
    finally:
        shutil.rmtree(root, ignore_errors=True)
    return digests
