#!/usr/bin/env python3
"""Fail fast when a Git dependency has no publishable crates.io baseline."""

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


def publish_dependencies(manifest_path: Path) -> list[PublishDependency]:
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    dependencies: set[PublishDependency] = set()

    for table in _dependency_tables(manifest):
        for local_name, specification in table.items():
            if not isinstance(specification, dict) or "git" not in specification:
                continue
            declared = specification.get("version")
            if not isinstance(declared, str):
                raise ValueError(
                    f"Git dependency {local_name!r} must declare a registry version"
                )
            version = declared.removeprefix("=")
            if not EXACT_VERSION.fullmatch(version):
                raise ValueError(
                    f"Git dependency {local_name!r} must use a concrete registry "
                    f"baseline, got {declared!r}"
                )
            package_name = specification.get("package", local_name)
            if not isinstance(package_name, str):
                raise ValueError(f"Git dependency {local_name!r} has an invalid package name")
            dependencies.add(PublishDependency(package_name, version))

    return sorted(dependencies)


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
    parser.add_argument("manifest", type=Path)
    parser.add_argument(
        "--api-base",
        default="https://crates.io/api/v1/crates",
        help="Registry version API base URL",
    )
    args = parser.parse_args()

    try:
        dependencies = publish_dependencies(args.manifest)
    except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
        print(f"publish dependency preflight failed: {error}", file=sys.stderr)
        return 1

    failures: list[str] = []
    for dependency in dependencies:
        error = registry_error(dependency, args.api_base)
        if error is None:
            print(f"verified {dependency.name} {dependency.version}")
        else:
            failures.append(f"{dependency.name} {dependency.version}: {error}")

    if failures:
        print(
            "publish dependency preflight failed; publish these registry baselines first:",
            file=sys.stderr,
        )
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
