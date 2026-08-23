from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from scripts.check_scoped_capability_architecture import (
    ArchitectureContractError,
    EXPECTED_GATES,
    EXPECTED_INVARIANTS,
    EXPECTED_OWNERS,
    EXPECTED_STATES,
    table_after_heading,
    verify_contract,
)


def table(headers: tuple[str, ...], rows: list[tuple[str, ...]]) -> str:
    header = "| " + " | ".join(headers) + " |"
    separator = "| " + " | ".join("---" for _ in headers) + " |"
    body = "\n".join("| " + " | ".join(row) + " |" for row in rows)
    return "\n".join((header, separator, body))


class ScopedCapabilityArchitectureTests(unittest.TestCase):
    def test_table_after_heading_parses_rows(self) -> None:
        parsed = table_after_heading(
            """# Contract

## Gates

| Gate | State |
| --- | --- |
| `CAP-FND1` | Delivered |
""",
            "## Gates",
        )
        self.assertEqual(parsed.headers, ("Gate", "State"))
        self.assertEqual(parsed.rows, (("`CAP-FND1`", "Delivered"),))

    def test_repository_contract_accepts_canonical_tables_and_local_links(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manual = root / "manual"
            manual.mkdir()
            evidence = root / "evidence.rs"
            evidence.write_text("// evidence\n", encoding="utf-8")
            contract = manual / "SCOPED_CAPABILITY_ARCHITECTURE.md"
            roadmap = root / "ROADMAP.md"
            contract.write_text(
                "\n\n".join(
                    (
                        "## Ownership boundary\n\n"
                        + table(
                            ("Owner", "Owns", "Must not own"),
                            [(owner, "owned", "excluded") for owner in EXPECTED_OWNERS],
                        ),
                        "## Architectural invariants\n\n"
                        + table(
                            ("ID", "Invariant"),
                            [(f"`{identifier}`", "held") for identifier in EXPECTED_INVARIANTS],
                        ),
                        "## Delivery gates\n\n"
                        + table(
                            ("Gate", "State", "Outcome", "Exit criteria"),
                            [
                                (f"`{gate}`", state, "outcome", "verified")
                                for gate, state in zip(EXPECTED_GATES, EXPECTED_STATES)
                            ],
                        ),
                        "[evidence](../evidence.rs)",
                    )
                ),
                encoding="utf-8",
            )
            roadmap.write_text(
                "### 3.2 Scoped capability program\n\n"
                + table(
                    ("Gate", "State", "Code-owned outcome", "Exit criteria"),
                    [
                        (f"`{gate}`", state, "outcome", "verified")
                        for gate, state in zip(EXPECTED_GATES, EXPECTED_STATES)
                    ],
                ),
                encoding="utf-8",
            )

            message = verify_contract(contract, roadmap)
            self.assertIn("12 invariants", message)

    def test_contract_rejects_reordered_gates(self) -> None:
        gates = list(EXPECTED_GATES)
        gates[1], gates[2] = gates[2], gates[1]
        text = "## Delivery gates\n\n" + table(
            ("Gate", "State"),
            [(f"`{gate}`", state) for gate, state in zip(gates, EXPECTED_STATES)],
        )
        with self.assertRaisesRegex(ArchitectureContractError, "gate order"):
            from scripts.check_scoped_capability_architecture import _verify_gate_table

            _verify_gate_table(text, "## Delivery gates")

    def test_contract_rejects_missing_local_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            contract = root / "contract.md"
            contract.write_text("[missing](missing.rs)\n", encoding="utf-8")
            from scripts.check_scoped_capability_architecture import _verify_local_links

            with self.assertRaisesRegex(ArchitectureContractError, "does not exist"):
                _verify_local_links(
                    contract.read_text(encoding="utf-8"), contract, root
                )


if __name__ == "__main__":
    unittest.main()
