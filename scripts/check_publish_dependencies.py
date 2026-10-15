#!/usr/bin/env python3
"""Fail fast when a Git/path dependency has no publishable registry baseline."""

from __future__ import annotations

import argparse
import re
import sys
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen


EXACT_VERSION = re.compile(
    r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)
DEPENDENCY_SECTIONS = ("dependencies", "build-dependencies", "dev-dependencies")


@dataclass(frozen=True, order=True)
class PublishDependency:
    name: str
    version: str


def _dependency_tables(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    tables: list[dict[str, Any]] = []
    for section in DEPENDENCY_SECTIONS:
        table = manifest.get(section)
        if isinstance(table, dict):
            tables.append(table)

    targets = manifest.get("target")
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for section in DEPENDENCY_SECTIONS:
                table = target.get(section)
                if isinstance(table, dict):
                    tables.append(table)
    return tables


def inspect_publish_dependencies(
    manifest_path: Path,
    candidates: dict[Path, PublishDependency] | None = None,
) -> tuple[list[PublishDependency], list[str]]:
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    dependencies: set[PublishDependency] = set()
    failures: list[str] = []

    for table in _dependency_tables(manifest):
        for local_name, specification in table.items():
            if not isinstance(specification, dict) or not (
                "git" in specification or "path" in specification
            ):
                continue
            source_kind = "Git" if "git" in specification else "Path"
            declared = specification.get("version")
            if not isinstance(declared, str):
                failures.append(
                    f"{source_kind} dependency {local_name!r} must declare a registry version"
                )
                continue
            version = declared.removeprefix("=")
            if not EXACT_VERSION.fullmatch(version):
                failures.append(
                    f"{source_kind} dependency {local_name!r} must use a concrete registry "
                    f"baseline, got {declared!r}"
                )
                continue
            package_name = specification.get("package", local_name)
            if not isinstance(package_name, str):
                failures.append(
                    f"{source_kind} dependency {local_name!r} has an invalid package name"
                )
                continue
            dependency = PublishDependency(package_name, version)
            source_path = specification.get("path")
            if candidates and isinstance(source_path, str) and "git" not in specification:
                source_manifest = (manifest_path.parent / source_path / "Cargo.toml").resolve()
                candidate = candidates.get(source_manifest)
                if candidate is not None:
                    if dependency != candidate:
                        failures.append(
                            f"candidate dependency {local_name!r} does not match "
                            f"{source_manifest}: expected {candidate.name} {candidate.version}"
                        )
                    continue
            dependencies.add(dependency)

    return sorted(dependencies), failures


def publish_dependencies(manifest_path: Path) -> list[PublishDependency]:
    dependencies, failures = inspect_publish_dependencies(manifest_path)
    if failures:
        raise ValueError("; ".join(failures))
    return dependencies


def registry_error(
    dependency: PublishDependency,
    api_base: str,
    opener: Callable[..., Any] = urlopen,
    attempts: int = 3,
) -> str | None:
    if attempts < 1:
        raise ValueError("registry check attempts must be positive")

    url = (
        f"{api_base.rstrip('/')}/{quote(dependency.name, safe='')}/"
        f"{quote(dependency.version, safe='')}"
    )
    request = Request(url, headers={"User-Agent": "a3s-code-release-preflight"})

    for attempt in range(1, attempts + 1):
        try:
            with opener(request, timeout=15) as response:
                if response.status == 200:
                    return None
                detail = f"HTTP {response.status}"
        except HTTPError as error:
            detail = f"HTTP {error.code}"
            if error.code == 404:
                return detail
        except URLError as error:
            detail = str(error.reason)

        if attempt < attempts:
            time.sleep(attempt)

    return detail


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path, nargs="+")
    parser.add_argument(
        "--candidate-manifest", type=Path, action="append", default=[],
        help="Local package published by this release; validate exact path/name/version "
             "instead of requiring it to be already published",
    )
    parser.add_argument(
        "--api-base",
        default="https://crates.io/api/v1/crates",
        help="Registry version API base URL",
    )
    args = parser.parse_args()

    failures: list[str] = []
    candidates: dict[Path, PublishDependency] = {}
    for path in args.candidate_manifest:
        try:
            package = tomllib.loads(path.read_text(encoding="utf-8"))["package"]
            name, version = package["name"], package["version"]
            if not isinstance(name, str) or not isinstance(version, str) or not EXACT_VERSION.fullmatch(version):
                raise ValueError("candidate must declare a package name and concrete version")
            candidates[path.resolve()] = PublishDependency(name, version)
        except (OSError, tomllib.TOMLDecodeError, ValueError, KeyError, TypeError) as error:
            failures.append(f"candidate {path}: {error}")

    dependencies: dict[PublishDependency, list[Path]] = {}
    for manifest in args.manifest:
        try:
            declared, manifest_failures = inspect_publish_dependencies(manifest, candidates)
        except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
            failures.append(f"{manifest}: {error}")
            continue
        failures.extend(f"{manifest}: {error}" for error in manifest_failures)
        for dependency in declared:
            dependencies.setdefault(dependency, []).append(manifest)

    for dependency, manifests in sorted(dependencies.items()):
        error = registry_error(dependency, args.api_base)
        if error is None:
            print(f"verified {dependency.name} {dependency.version}")
        else:
            origins = ", ".join(str(manifest) for manifest in manifests)
            failures.append(f"{dependency.name} {dependency.version} ({origins}): {error}")

    for path, candidate in sorted(candidates.items()):
        print(f"local release candidate {candidate.name} {candidate.version} ({path}); "
              "registry delivery remains required")

    if failures:
        print(
            "publish dependency preflight failed; resolve these manifest and registry obligations:",
            file=sys.stderr,
        )
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
