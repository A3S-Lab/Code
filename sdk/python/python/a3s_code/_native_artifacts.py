"""Deterministic discovery and cleanup for generated native extensions."""

from __future__ import annotations

from pathlib import Path


def _is_native_extension_artifact(path: Path) -> bool:
    name = path.name
    return (
        path.is_file()
        and name.startswith("_native.")
        and path.suffix.lower() in {".pyd", ".so", ".dylib"}
    )


def find_native_extension_artifacts(
    package_dir: Path | None = None,
) -> tuple[Path, ...]:
    """Return generated ``_native`` extension files in deterministic order."""

    root = package_dir or Path(__file__).resolve().parent
    return tuple(
        sorted(path for path in root.iterdir() if _is_native_extension_artifact(path))
    )


def ensure_unambiguous_native_extension(package_dir: Path | None = None) -> None:
    """Fail closed when Python could silently select a stale native binary."""

    artifacts = find_native_extension_artifacts(package_dir)
    if len(artifacts) <= 1:
        return

    names = ", ".join(path.name for path in artifacts)
    raise ImportError(
        "Multiple A3S Code native extensions are present and import order is "
        f"ambiguous: {names}. Remove generated native artifacts and rebuild. "
        "From a source checkout, run "
        "`python sdk/python/scripts/clean_native_artifacts.py`."
    )


def clean_native_extension_artifacts(
    package_dir: Path | None = None,
) -> tuple[Path, ...]:
    """Remove rebuildable ``_native`` extension files and return their paths."""

    artifacts = find_native_extension_artifacts(package_dir)
    for artifact in artifacts:
        artifact.unlink()
    return artifacts


__all__ = [
    "clean_native_extension_artifacts",
    "ensure_unambiguous_native_extension",
    "find_native_extension_artifacts",
]
