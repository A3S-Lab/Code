#!/usr/bin/env python3
"""Verify the scoped-capability architecture contract and Roadmap alignment."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import unquote


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
CONTRACT_PATH = REPOSITORY_ROOT / "manual" / "SCOPED_CAPABILITY_ARCHITECTURE.md"
ROADMAP_PATH = REPOSITORY_ROOT / "ROADMAP.md"

EXPECTED_GATES = (
    "CAP-FND1",
    "USE-BRIDGE1",
    "CAP-SET1",
    "CAP-SCOPE1",
    "CAP-PROJ1",
    "CAP-DEP1",
    "HOST-CAP1",
    "CAP-PROFILE1",
    "CAP-GA1",
)
EXPECTED_STATES = ("Delivered",) * 8 + ("Planned",)
EXPECTED_INVARIANTS = tuple(f"CAP-I{index:02d}" for index in range(1, 13))
EXPECTED_OWNERS = (
    "Host Plugin Manager",
    "A3S Use",
    "A3S Code",
    "Runtime and Gateway",
    "A3S Flow",
    "Knowledge host",
)


class ArchitectureContractError(ValueError):
    """Raised when the architecture contract is incomplete or inconsistent."""


@dataclass(frozen=True)
class MarkdownTable:
    headers: tuple[str, ...]
    rows: tuple[tuple[str, ...], ...]


def _cells(line: str) -> tuple[str, ...]:
    return tuple(cell.strip() for cell in line.strip().strip("|").split("|"))


def _is_separator(line: str) -> bool:
    cells = _cells(line)
    return bool(cells) and all(re.fullmatch(r":?-{3,}:?", cell) for cell in cells)


def table_after_heading(text: str, heading: str) -> MarkdownTable:
    lines = text.splitlines()
    try:
        heading_index = lines.index(heading)
    except ValueError as error:
        raise ArchitectureContractError(f"missing heading: {heading}") from error

    table_start = None
    for index in range(heading_index + 1, len(lines) - 1):
        if lines[index].lstrip().startswith("#"):
            break
        if lines[index].strip().startswith("|") and _is_separator(lines[index + 1]):
            table_start = index
            break
    if table_start is None:
        raise ArchitectureContractError(f"missing table after heading: {heading}")

    headers = _cells(lines[table_start])
    rows: list[tuple[str, ...]] = []
    for line in lines[table_start + 2 :]:
        if not line.strip().startswith("|"):
            break
        row = _cells(line)
        if len(row) != len(headers):
            raise ArchitectureContractError(
                f"table after {heading} has {len(row)} cells; expected {len(headers)}"
            )
        rows.append(row)
    if not rows:
        raise ArchitectureContractError(f"empty table after heading: {heading}")
    return MarkdownTable(headers=headers, rows=tuple(rows))


def _column(table: MarkdownTable, name: str) -> tuple[str, ...]:
    try:
        index = table.headers.index(name)
    except ValueError as error:
        raise ArchitectureContractError(f"missing table column: {name}") from error
    return tuple(row[index].strip().strip("`") for row in table.rows)


def _verify_gate_table(text: str, heading: str) -> None:
    table = table_after_heading(text, heading)
    gates = _column(table, "Gate")
    states = _column(table, "State")
    if gates != EXPECTED_GATES:
        raise ArchitectureContractError(
            f"gate order after {heading} differs: expected={EXPECTED_GATES}, actual={gates}"
        )
    if states != EXPECTED_STATES:
        raise ArchitectureContractError(
            f"gate states after {heading} differ: expected={EXPECTED_STATES}, actual={states}"
        )


def _verify_invariants(contract_text: str) -> None:
    table = table_after_heading(contract_text, "## Architectural invariants")
    invariants = _column(table, "ID")
    if invariants != EXPECTED_INVARIANTS:
        raise ArchitectureContractError(
            "architecture invariant IDs must be complete and canonically ordered: "
            f"expected={EXPECTED_INVARIANTS}, actual={invariants}"
        )


def _verify_owners(contract_text: str) -> None:
    table = table_after_heading(contract_text, "## Ownership boundary")
    owners = _column(table, "Owner")
    if owners != EXPECTED_OWNERS:
        raise ArchitectureContractError(
            f"ownership boundary differs: expected={EXPECTED_OWNERS}, actual={owners}"
        )


def _verify_local_links(
    contract_text: str,
    contract_path: Path,
    repository_root: Path = REPOSITORY_ROOT,
) -> None:
    for target in re.findall(r"\[[^\]]+\]\(([^)]+)\)", contract_text):
        if target.startswith(("https://", "http://", "#")):
            continue
        path_text = unquote(target.split("#", 1)[0])
        resolved = (contract_path.parent / path_text).resolve()
        try:
            resolved.relative_to(repository_root.resolve())
        except ValueError as error:
            raise ArchitectureContractError(
                f"architecture evidence link escapes the repository: {target}"
            ) from error
        if not resolved.exists():
            raise ArchitectureContractError(
                f"architecture evidence path does not exist: {target}"
            )


def verify_contract(
    contract_path: Path = CONTRACT_PATH,
    roadmap_path: Path = ROADMAP_PATH,
) -> str:
    contract_text = contract_path.read_text(encoding="utf-8")
    roadmap_text = roadmap_path.read_text(encoding="utf-8")

    _verify_owners(contract_text)
    _verify_invariants(contract_text)
    _verify_gate_table(contract_text, "## Delivery gates")
    _verify_gate_table(roadmap_text, "### 3.2 Scoped capability program")
    _verify_local_links(contract_text, contract_path, roadmap_path.parent)
    return (
        "scoped capability architecture complete: "
        f"{len(EXPECTED_OWNERS)} owners, {len(EXPECTED_INVARIANTS)} invariants, "
        f"{len(EXPECTED_GATES)} gates"
    )


def main() -> int:
    try:
        print(verify_contract())
    except (OSError, ArchitectureContractError) as error:
        print(f"scoped capability architecture failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
