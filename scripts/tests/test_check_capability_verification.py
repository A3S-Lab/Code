from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from scripts.check_capability_verification import (
    VerificationError,
    table_after_heading,
    verify_area_parity,
    verify_repository,
)


class CapabilityVerificationTests(unittest.TestCase):
    def test_table_after_heading_parses_a_complete_table(self) -> None:
        table = table_after_heading(
            """# Document

## Ledger

| Area | Evidence |
| --- | --- |
| Runtime | `runtime.rs` |
| Tools | `tools.rs` |
""",
            "## Ledger",
        )
        self.assertEqual(table.headers, ("Area", "Evidence"))
        self.assertEqual(table.rows[1], ("Tools", "`tools.rs`"))

    def test_area_parity_rejects_missing_areas(self) -> None:
        readme = table_after_heading(
            """## Capability map
| Area | Claim |
| --- | --- |
| Runtime | async |
| Tools | governed |
""",
            "## Capability map",
        )
        ledger = table_after_heading(
            """## Capability evidence ledger
| Area | Required deterministic evidence |
| --- | --- |
| Runtime | tests |
""",
            "## Capability evidence ledger",
        )
        with self.assertRaisesRegex(VerificationError, "missing=\\['Tools'\\]"):
            verify_area_parity(readme, ledger)

    def test_repository_verification_rejects_missing_evidence_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manual = root / "manual"
            manual.mkdir()
            readme = root / "README.md"
            ledger = manual / "CAPABILITY_VERIFICATION.md"
            readme.write_text(
                """## Capability map
| Area | Claim |
| --- | --- |
| Runtime | async |
""",
                encoding="utf-8",
            )
            ledger.write_text(
                """## Capability evidence ledger
| Area | Required deterministic evidence | Additional qualification | Performance and resource evidence | Current evidence state |
| --- | --- | --- | --- | --- |
| Runtime | [tests](../missing.rs) | None | Bounded | Required CI |

## Runtime surface ledger
| Surface | Required runtime evidence |
| --- | --- |
| Rust Core | tests |
| Node.js | tests |
| Python | tests |
| Go | tests |
| Documentation site | tests |
""",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(VerificationError, "does not exist"):
                verify_repository(readme, ledger)


if __name__ == "__main__":
    unittest.main()
