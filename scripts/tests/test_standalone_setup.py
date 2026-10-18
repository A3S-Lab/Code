"""Standalone CI keeps published dependencies and the actual ACP member."""

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class StandaloneSetupTests(unittest.TestCase):
    def test_setup_preserves_acp_and_current_published_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative in (
                "Cargo.toml", "core/Cargo.toml", "acp/Cargo.toml", "sdk/go/bridge/Cargo.toml",
                "sdk/node/Cargo.toml", "sdk/python/Cargo.toml",
                ".github/setup-workspace.sh",
            ):
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative, target)
            original = (root / "core/Cargo.toml").read_text()
            for _ in range(2):
                result = subprocess.run(
                    ["bash", ".github/setup-workspace.sh"], cwd=root,
                    capture_output=True, text=True, timeout=10,
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual((root / "core/Cargo.toml").read_text(), original)
                self.assertIn('"acp",', (root / "Cargo.toml").read_text())
                for sdk in ("node", "python"):
                    self.assertNotIn("[patch.", (root / f"sdk/{sdk}/Cargo.toml").read_text())
            with (root / "core/Cargo.toml").open("a") as manifest:
                manifest.write('\n[dependencies.fixture_escape]\nversion = "0.1.0"\npath = "../../outside"\n')
            refused = subprocess.run(
                ["bash", ".github/setup-workspace.sh"], cwd=root,
                capture_output=True, text=True, timeout=10,
            )
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("dependency path escapes standalone workspace", refused.stderr)


if __name__ == "__main__":
    unittest.main()
