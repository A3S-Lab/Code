"""Registry delivery must never report a partial Core/ACP upload as complete."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[2] / ".github/publish-code-crates.sh"


class PublishCodeCratesTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "core").mkdir()
        (self.root / "core/Cargo.toml").write_text('[package]\nversion = "9.1.4"\n')
        (self.root / "scripts").mkdir()
        (self.root / "scripts/check_release_versions.sh").write_text("exit 0\n")
        self.calls = self.root / "uploads"
        self.registry = self.root / "registry"
        self.registry.write_text(json.dumps({"status": "404"}))
        self.bin = self.root / "bin"
        self.bin.mkdir()
        (self.bin / "python3").symlink_to(sys.executable)
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ["PATH"])
        self.env.update(CARGO_REGISTRY_TOKEN="test-only", A3S_TEST_UPLOADS=str(self.calls),
                        A3S_TEST_REGISTRY=str(self.registry))
        self.command("curl", '''import json, os, pathlib, sys
data = json.loads(pathlib.Path(os.environ["A3S_TEST_REGISTRY"]).read_text())
if data.get("network_failure"):
    sys.exit(7)
output = sys.argv[sys.argv.index("--output") + 1]
package, version = sys.argv[-1].split("/")[-2:]
pathlib.Path(output).write_text(json.dumps({"version": {
    "crate": package, "num": data.get("version", version), "yanked": data.get("yanked", False)}}))
sys.stdout.write(data["status"])
''')
        self.command("cargo", '''import os, pathlib, sys
assert sys.argv[1] == "publish"
package = sys.argv[sys.argv.index("-p") + 1]
with pathlib.Path(os.environ["A3S_TEST_UPLOADS"]).open("a") as output:
    output.write(package + "\\n")
if package == os.environ.get("A3S_TEST_FAIL_PACKAGE"):
    sys.exit(1)
''')
    def command(self, name, source):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o755)

    def run_publish(self):
        return subprocess.run(["bash", str(SCRIPT)], cwd=self.root, env=self.env,
                              text=True, capture_output=True)

    def uploaded(self):
        return self.calls.read_text().splitlines() if self.calls.exists() else []

    def test_both_packages_upload_in_dependency_order(self):
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.uploaded(), ["a3s-code-core", "a3s-code-acp"])

    def test_failed_core_never_uploads_acp(self):
        self.env["A3S_TEST_FAIL_PACKAGE"] = "a3s-code-core"
        self.assertNotEqual(self.run_publish().returncode, 0)
        self.assertEqual(self.uploaded(), ["a3s-code-core"])

    def test_failed_acp_does_not_report_release_success(self):
        self.env["A3S_TEST_FAIL_PACKAGE"] = "a3s-code-acp"
        self.assertNotEqual(self.run_publish().returncode, 0)
        self.assertEqual(self.uploaded(), ["a3s-code-core", "a3s-code-acp"])

    def test_verified_existing_packages_are_resumable(self):
        self.registry.write_text(json.dumps({"status": "200"}))
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.uploaded(), [])

    def test_yanked_or_wrong_version_refuses_activation(self):
        for extra in [{"yanked": True}, {"version": "9.1.3"}]:
            with self.subTest(extra=extra):
                self.registry.write_text(json.dumps(dict(status="200", **extra)))
                self.assertNotEqual(self.run_publish().returncode, 0)
                self.assertEqual(self.uploaded(), [])

    def test_unavailable_registry_is_not_an_absent_package(self):
        for response in [{"status": "503"}, {"status": "404", "network_failure": True}]:
            with self.subTest(response=response):
                self.registry.write_text(json.dumps(response))
                self.assertNotEqual(self.run_publish().returncode, 0)
                self.assertEqual(self.uploaded(), [])

    def test_missing_credential_never_uploads(self):
        self.env.pop("CARGO_REGISTRY_TOKEN")
        self.assertNotEqual(self.run_publish().returncode, 0)
        self.assertEqual(self.uploaded(), [])


if __name__ == "__main__":
    unittest.main()
