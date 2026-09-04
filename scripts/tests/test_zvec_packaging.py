from __future__ import annotations

import hashlib
import json
import os
import stat
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).parents[2]
PACKAGE_SCRIPT = ROOT / "scripts" / "package_zvec.sh"
RUSTFLAGS_SCRIPT = ROOT / "scripts" / "zvec_rustflags.sh"


class ZvecPackagingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.archive = self.root / "fixture.tar.gz"
        self._write_fake_curl()

    def tearDown(self) -> None:
        self.directory.cleanup()

    def _write_fake_curl(self) -> None:
        # The production script still validates an HTTPS release URL. The
        # hermetic test replaces only the downloader and serves a local,
        # digest-locked archive through an executable in PATH.
        curl = self.bin / "curl"
        curl.write_text(
            "#!/usr/bin/env python3\n"
            "import os, shutil, sys\n"
            "args = sys.argv[1:]\n"
            "output = args[args.index('--output') + 1]\n"
            "shutil.copy2(os.environ['FIXTURE_ARCHIVE'], output)\n",
            encoding="utf-8",
        )
        curl.chmod(curl.stat().st_mode | stat.S_IXUSR)

    def _archive(self, *, symlink: bool = False) -> str:
        source = self.root / "source"
        source.mkdir()
        (source / "TARGET").write_text("test-target\n", encoding="utf-8")
        (source / "libfixture.so").write_bytes(b"verified native runtime")
        with tarfile.open(self.archive, "w:gz") as stream:
            stream.add(source / "TARGET", arcname="TARGET")
            stream.add(source / "libfixture.so", arcname="libfixture.so")
            if symlink:
                link = source / "link.so"
                link.symlink_to("libfixture.so")
                stream.add(link, arcname="link.so", recursive=False)
        return hashlib.sha256(self.archive.read_bytes()).hexdigest()

    def _manifest(self, digest: str | None) -> Path:
        manifest = self.root / "manifest.json"
        asset = {
            "archive": "fixture.tar.gz",
            "format": "tar.gz",
            "library": "libfixture.so",
        }
        if digest is not None:
            asset["sha256"] = digest
        manifest.write_text(
            json.dumps({"version": "0.0.1", "assets": {"test-target": asset}}),
            encoding="utf-8",
        )
        return manifest

    def _run_package(self, target: str = "test-target", *extra: str) -> subprocess.CompletedProcess[str]:
        output = self.root / "output"
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{self.bin}{os.pathsep}{env['PATH']}",
                "FIXTURE_ARCHIVE": str(self.archive),
                "A3S_CODE_ZVEC_MANIFEST": str(self.root / "manifest.json"),
                "A3S_CODE_ZVEC_RELEASE_BASE_URL": "https://fixture.invalid/release",
            }
        )
        return subprocess.run(
            ["bash", str(PACKAGE_SCRIPT), target, str(output), *extra],
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_stages_only_the_verified_runtime_and_writes_provenance(self) -> None:
        digest = self._archive()
        self._manifest(digest)
        result = self._run_package()
        self.assertEqual(result.returncode, 0, result.stderr)

        output = self.root / "output"
        self.assertEqual((output / "libfixture.so").read_bytes(), b"verified native runtime")
        metadata = json.loads((output / "zvec-runtime.json").read_text(encoding="utf-8"))
        self.assertEqual(metadata["schema"], "a3s-code/zvec-runtime-package/v1")
        self.assertEqual(metadata["target"], "test-target")
        self.assertEqual(metadata["archive_sha256"], digest)
        self.assertFalse((output / "ZVEC_UNAVAILABLE").exists())

    def test_digest_mismatch_fails_before_publishing_a_library(self) -> None:
        self._archive()
        self._manifest("0" * 64)
        result = self._run_package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256 mismatch", result.stderr)
        self.assertFalse((self.root / "output" / "libfixture.so").exists())

    def test_symlinked_archive_member_is_rejected(self) -> None:
        digest = self._archive(symlink=True)
        self._manifest(digest)
        result = self._run_package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("hardlink or symlink", result.stderr)
        self.assertFalse((self.root / "output" / "libfixture.so").exists())

    def test_unsupported_target_requires_an_explicit_marker_override(self) -> None:
        self._manifest(None)
        # A target absent from the manifest must not silently fall back to a
        # downloaded or host-native library.
        result = self._run_package("unsupported-target")
        self.assertEqual(result.returncode, 2, result.stderr)

        result = self._run_package("unsupported-target", "--allow-unsupported")
        self.assertEqual(result.returncode, 0, result.stderr)
        marker = (self.root / "output" / "ZVEC_UNAVAILABLE").read_text(encoding="utf-8")
        self.assertIn("reason=no-upstream-prebuilt-asset", marker)

    def test_rustflags_are_relocatable_and_reject_unsafe_inputs(self) -> None:
        cases = {
            "aarch64-apple-darwin": "@loader_path/zvec",
            "x86_64-unknown-linux-gnu": "$ORIGIN/zvec",
            "x86_64-pc-windows-msvc": "",
        }
        for target, expected in cases.items():
            result = subprocess.run(
                ["bash", str(RUSTFLAGS_SCRIPT), target, "zvec"],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, (target, result.stderr))
            self.assertIn(expected, result.stdout)

        unsafe = subprocess.run(
            ["bash", str(RUSTFLAGS_SCRIPT), "x86_64-unknown-linux-gnu", "../zvec"],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertNotEqual(unsafe.returncode, 0)
        self.assertIn("unsafe", unsafe.stderr)


if __name__ == "__main__":
    unittest.main()
