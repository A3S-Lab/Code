#!/usr/bin/env python3
"""Validate that every advertised capability has an evidence-ledger entry."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import unquote


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
README_PATH = REPOSITORY_ROOT / "README.md"
LEDGER_PATH = REPOSITORY_ROOT / "manual" / "CAPABILITY_VERIFICATION.md"
LOCAL_LINK = re.compile(r"\[[^\]]+\]\(([^)]+)\)")


class VerificationError(RuntimeError):
    """Raised when the capability ledger is incomplete or inconsistent."""


@dataclass(frozen=True)
class MarkdownTable:
    headers: tuple[str, ...]
    rows: tuple[tuple[str, ...], ...]


def _is_separator_row(cells: tuple[str, ...]) -> bool:
    return bool(cells) and all(re.fullmatch(r":?-{3,}:?", cell) for cell in cells)


def _split_row(line: str) -> tuple[str, ...]:
    if not line.startswith("|") or not line.endswith("|"):
        raise VerificationError(f"invalid Markdown table row: {line}")
    return tuple(cell.strip() for cell in line[1:-1].split("|"))


def table_after_heading(text: str, heading: str) -> MarkdownTable:
    """Return the first Markdown table following an exact heading."""

    lines = text.splitlines()
    try:
        heading_index = lines.index(heading)
    except ValueError as error:
        raise VerificationError(f"missing heading {heading!r}") from error

    table_start = next(
        (index for index in range(heading_index + 1, len(lines)) if lines[index].startswith("|")),
        None,
    )
    if table_start is None:
        raise VerificationError(f"missing table after {heading!r}")

    table_lines: list[str] = []
    for line in lines[table_start:]:
        if not line.startswith("|"):
            break
        table_lines.append(line)
    if len(table_lines) < 3:
        raise VerificationError(f"table after {heading!r} has no data rows")

    headers = _split_row(table_lines[0])
    separator = _split_row(table_lines[1])
    if len(separator) != len(headers) or not _is_separator_row(separator):
        raise VerificationError(f"table after {heading!r} has an invalid separator row")

    rows = tuple(_split_row(line) for line in table_lines[2:])
    for row in rows:
        if len(row) != len(headers):
            raise VerificationError(
                f"table after {heading!r} has {len(row)} cells; expected {len(headers)}"
            )
    return MarkdownTable(headers=headers, rows=rows)


def _column(table: MarkdownTable, name: str) -> tuple[str, ...]:
    try:
        index = table.headers.index(name)
    except ValueError as error:
        raise VerificationError(f"missing {name!r} column in {table.headers!r}") from error
    return tuple(row[index] for row in table.rows)


def verify_area_parity(readme: MarkdownTable, ledger: MarkdownTable) -> tuple[str, ...]:
    advertised = _column(readme, "Area")
    evidenced = _column(ledger, "Area")
    if len(set(advertised)) != len(advertised):
        raise VerificationError("README capability map contains duplicate areas")
    if len(set(evidenced)) != len(evidenced):
        raise VerificationError("capability evidence ledger contains duplicate areas")
    if advertised != evidenced:
        missing = [area for area in advertised if area not in evidenced]
        unexpected = [area for area in evidenced if area not in advertised]
        order_mismatch = not missing and not unexpected
        raise VerificationError(
            "capability area mismatch: "
            f"missing={missing}, unexpected={unexpected}, order_mismatch={order_mismatch}"
        )
    return advertised


def _local_link_targets(cell: str) -> tuple[str, ...]:
    return tuple(
        target
        for target in LOCAL_LINK.findall(cell)
        if "://" not in target and not target.startswith(("#", "mailto:"))
    )


def verify_evidence_links(
    ledger: MarkdownTable, ledger_path: Path, repository_root: Path
) -> None:
    area_index = ledger.headers.index("Area")
    evidence_index = ledger.headers.index("Required deterministic evidence")
    qualification_index = ledger.headers.index("Additional qualification")
    performance_index = ledger.headers.index("Performance and resource evidence")
    state_index = ledger.headers.index("Current evidence state")

    for row in ledger.rows:
        area = row[area_index]
        required_targets = _local_link_targets(row[evidence_index])
        if not required_targets:
            raise VerificationError(f"{area}: required evidence has no local repository link")

        all_targets = (
            required_targets
            + _local_link_targets(row[qualification_index])
            + _local_link_targets(row[performance_index])
        )
        for target in all_targets:
            path_text = unquote(target.split("#", 1)[0])
            resolved = (ledger_path.parent / path_text).resolve()
            try:
                resolved.relative_to(repository_root)
            except ValueError as error:
                raise VerificationError(
                    f"{area}: evidence link escapes the repository: {target}"
                ) from error
            if not resolved.exists():
                raise VerificationError(f"{area}: evidence path does not exist: {target}")

        if not row[performance_index].strip():
            raise VerificationError(f"{area}: performance/resource evidence is empty")
        if not row[state_index].strip():
            raise VerificationError(f"{area}: current evidence state is empty")


def verify_runtime_surfaces(ledger_text: str) -> tuple[str, ...]:
    runtime = table_after_heading(ledger_text, "## Runtime surface ledger")
    surfaces = _column(runtime, "Surface")
    expected = ("Rust Core", "Node.js", "Python", "Go", "Documentation site")
    if surfaces != expected:
        raise VerificationError(
            f"runtime surface ledger mismatch: expected={expected}, actual={surfaces}"
        )
    return surfaces


def verify_repository(readme_path: Path = README_PATH, ledger_path: Path = LEDGER_PATH) -> str:
    readme_text = readme_path.read_text(encoding="utf-8")
    ledger_text = ledger_path.read_text(encoding="utf-8")
    readme = table_after_heading(readme_text, "## Capability map")
    ledger = table_after_heading(ledger_text, "## Capability evidence ledger")
    areas = verify_area_parity(readme, ledger)
    verify_evidence_links(ledger, ledger_path, readme_path.parent.resolve())
    surfaces = verify_runtime_surfaces(ledger_text)
    return f"capability verification complete: {len(areas)} areas, {len(surfaces)} runtime surfaces"


def main() -> int:
    try:
        print(verify_repository())
    except (OSError, VerificationError) as error:
        print(f"capability verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
