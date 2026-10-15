"""Release checks reject independently drifting shipped artifacts."""

import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/check_release_versions.sh"


class ReleaseVersionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        target = self.root / "scripts/check_release_versions.sh"
        target.parent.mkdir()
        shutil.copyfile(SCRIPT, target)
        paths = set(re.findall(r'^check_\w+\("([^"]+)"', SCRIPT.read_text(), re.M))
        for path in paths:
            destination = self.root / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, destination)
        self.version = re.search(r'^version = "([^"]+)"', (self.root / "core/Cargo.toml").read_text(), re.M)[1]

    def check(self):
        return subprocess.run(["bash", str(self.root / "scripts/check_release_versions.sh"), self.version],
                              cwd=self.temporary.name, capture_output=True, text=True, timeout=10)

    def change(self, path, old, new):
        target = self.root / path
        text = target.read_text()
        self.assertIn(old, text)
        target.write_text(text.replace(old, new, 1))

    def test_consistent_candidate(self):
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_shipped_pager_drift(self):
        self.change("vendor/a3s-code-ui/crates/codegen/a3s-code-pager-bin/Cargo.toml",
                    f'version = "{self.version}"', 'version = "0.0.0"')
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("a3s-code-pager-bin/Cargo.toml", result.stderr)

    def test_acp_dependency_drift(self):
        self.change("acp/Cargo.toml", f'a3s-code-core = {{ version = "{self.version}"',
                    'a3s-code-core = { version = "0.0.0"')
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("acp/Cargo.toml a3s-code-core", result.stderr)

    def test_pager_lock_drift(self):
        self.change("vendor/a3s-code-ui/Cargo.lock",
                    f'name = "a3s-code-version"\nversion = "{self.version}"',
                    'name = "a3s-code-version"\nversion = "0.0.0"')
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("a3s-code-version", result.stderr)

    def test_both_unreleased_heading_forms_refuse_unfolded_notes(self):
        for heading in ("## [Unreleased]", "## Unreleased"):
            with self.subTest(heading=heading):
                path = self.root / "CHANGELOG.md"
                original = path.read_text()
                path.write_text(original.replace("## [Unreleased]", heading + "\n\n- pending fix", 1))
                result = self.check()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Unreleased note(s)", result.stderr)
                path.write_text(original)


if __name__ == "__main__":
    unittest.main()
