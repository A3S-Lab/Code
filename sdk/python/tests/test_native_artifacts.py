from __future__ import annotations

from pathlib import Path

import pytest

from a3s_code._native_artifacts import (
    clean_native_extension_artifacts,
    ensure_unambiguous_native_extension,
    find_native_extension_artifacts,
)


def _touch(directory: Path, name: str) -> Path:
    path = directory / name
    path.write_bytes(b"test artifact")
    return path


def test_native_artifact_discovery_is_exact_and_deterministic(tmp_path: Path) -> None:
    expected = (
        _touch(tmp_path, "_native.abi3.so"),
        _touch(tmp_path, "_native.cpython-313-x86_64-linux-gnu.so"),
        _touch(tmp_path, "_native.pyd"),
    )
    _touch(tmp_path, "_native_artifacts.py")
    _touch(tmp_path, "another_native.pyd")

    assert find_native_extension_artifacts(tmp_path) == expected


def test_ambiguous_native_artifacts_fail_closed(tmp_path: Path) -> None:
    _touch(tmp_path, "_native.cp313-win_amd64.pyd")
    _touch(tmp_path, "_native.pyd")

    with pytest.raises(ImportError, match="import order is ambiguous"):
        ensure_unambiguous_native_extension(tmp_path)


def test_cleanup_removes_only_rebuildable_native_artifacts(tmp_path: Path) -> None:
    stale = _touch(tmp_path, "_native.cp313-win_amd64.pyd")
    current = _touch(tmp_path, "_native.pyd")
    retained = _touch(tmp_path, "callback_provider.pyd")

    assert clean_native_extension_artifacts(tmp_path) == (stale, current)
    assert not stale.exists()
    assert not current.exists()
    assert retained.exists()
    ensure_unambiguous_native_extension(tmp_path)
