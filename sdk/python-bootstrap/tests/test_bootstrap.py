"""Unit tests for the a3s-code bootstrap loader.

Live network is not required — `_http_get` is monkey-patched to serve
a constructed wheel byte string. There's a separate live integration
test at the bottom guarded by `A3S_CODE_BOOTSTRAP_LIVE=1`.
"""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
import unittest.mock as mock
import zipfile
from pathlib import Path


# Load `_bootstrap` directly from disk so the package `__init__.py`
# (which would call `ensure_native_loaded` and try to hit the network)
# doesn't run during test collection.
_BOOTSTRAP_PATH = (
    Path(__file__).resolve().parents[1] / "src" / "a3s_code" / "_bootstrap.py"
)
_spec = importlib.util.spec_from_file_location("_bootstrap", _BOOTSTRAP_PATH)
_bootstrap = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_bootstrap)


def _make_wheel(native_blob: bytes = b"fake-extension-blob") -> bytes:
    """Build a minimal in-memory wheel containing _native.something.so."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("a3s_code/__init__.py", "from ._native import *\n")
        zf.writestr("a3s_code/_native.cpython-312-x86_64-linux-gnu.so", native_blob)
        zf.writestr("a3s_code-3.2.1.dist-info/METADATA", "Metadata-Version: 2.1\n")
        zf.writestr("a3s_code-3.2.1.dist-info/WHEEL", "Wheel-Version: 1.0\n")
    return buf.getvalue()


class _FakeVersionInfo:
    """Stand-in for sys.version_info supporting attribute access."""

    def __init__(self, major: int, minor: int):
        self.major = major
        self.minor = minor
        self.micro = 0
        self.releaselevel = "final"
        self.serial = 0


class WheelFilenameTests(unittest.TestCase):
    def _filename_for(self, sys_plat: str, machine: str, py_minor: int) -> str:
        with (
            mock.patch.object(sys, "platform", sys_plat),
            mock.patch.object(_bootstrap.platform, "machine", return_value=machine),
            mock.patch.object(sys, "version_info", _FakeVersionInfo(3, py_minor)),
        ):
            return _bootstrap._wheel_filename(version="3.2.1")

    def test_linux_x86_64_cp312(self):
        self.assertEqual(
            self._filename_for("linux", "x86_64", 12),
            "a3s_code-3.2.1-cp312-cp312-manylinux_2_28_x86_64.whl",
        )

    def test_macos_arm64_cp311(self):
        self.assertEqual(
            self._filename_for("darwin", "arm64", 11),
            "a3s_code-3.2.1-cp311-cp311-macosx_11_0_arm64.whl",
        )

    def test_macos_intel_cp312(self):
        self.assertEqual(
            self._filename_for("darwin", "x86_64", 12),
            "a3s_code-3.2.1-cp312-cp312-macosx_12_0_x86_64.whl",
        )

    def test_macos_intel_abi3(self):
        with (
            mock.patch.object(sys, "platform", "darwin"),
            mock.patch.object(_bootstrap.platform, "machine", return_value="x86_64"),
            mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 14)),
        ):
            self.assertEqual(
                _bootstrap._abi3_wheel_filename("3.2.1"),
                "a3s_code-3.2.1-cp310-abi3-macosx_12_0_x86_64.whl",
            )

    def test_new_python_prefers_abi3_then_exact_fallback(self):
        with (
            mock.patch.object(sys, "platform", "darwin"),
            mock.patch.object(_bootstrap.platform, "machine", return_value="x86_64"),
            mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 14)),
        ):
            self.assertEqual(
                _bootstrap._wheel_candidates("3.2.1"),
                (
                    "a3s_code-3.2.1-cp310-abi3-macosx_12_0_x86_64.whl",
                    "a3s_code-3.2.1-cp314-cp314-macosx_12_0_x86_64.whl",
                ),
            )

    def test_existing_python_prefers_exact_then_abi3(self):
        with (
            mock.patch.object(sys, "platform", "darwin"),
            mock.patch.object(_bootstrap.platform, "machine", return_value="x86_64"),
            mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 12)),
        ):
            self.assertEqual(
                _bootstrap._wheel_candidates("3.2.1"),
                (
                    "a3s_code-3.2.1-cp312-cp312-macosx_12_0_x86_64.whl",
                    "a3s_code-3.2.1-cp310-abi3-macosx_12_0_x86_64.whl",
                ),
            )

    def test_python_below_abi3_floor_raises(self):
        with (
            mock.patch.object(sys, "platform", "darwin"),
            mock.patch.object(_bootstrap.platform, "machine", return_value="x86_64"),
            mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 9)),
        ):
            with self.assertRaises(_bootstrap.BootstrapError) as cm:
                _bootstrap._wheel_candidates("3.2.1")
        self.assertIn("CPython 3.10 or newer", str(cm.exception))

    def test_windows_amd64_cp313(self):
        self.assertEqual(
            self._filename_for("win32", "AMD64", 13),
            "a3s_code-3.2.1-cp313-cp313-win_amd64.whl",
        )

    def test_unsupported_platform_raises(self):
        with self.assertRaises(_bootstrap.BootstrapError) as cm:
            self._filename_for("freebsd", "x86_64", 12)
        self.assertIn("no native wheel published", str(cm.exception))

    def test_unsupported_linux_arch_raises(self):
        with self.assertRaises(_bootstrap.BootstrapError):
            self._filename_for("linux", "ppc64le", 12)


class CacheDirTests(unittest.TestCase):
    def setUp(self):
        self._prev_env = {
            k: os.environ.get(k) for k in ("A3S_CODE_CACHE_DIR", "XDG_CACHE_HOME")
        }
        for k in ("A3S_CODE_CACHE_DIR", "XDG_CACHE_HOME"):
            os.environ.pop(k, None)

    def tearDown(self):
        for k, v in self._prev_env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def test_default_uses_xdg_or_home(self):
        cache = _bootstrap._cache_root()
        self.assertEqual(cache.parts[-3:-1], ("a3s-code", _bootstrap.__version__))
        self.assertEqual(cache.name, _bootstrap._platform_tag())

    def test_xdg_cache_home_honored(self):
        os.environ["XDG_CACHE_HOME"] = "/tmp/xdg-test"
        cache = _bootstrap._cache_root()
        self.assertEqual(
            cache,
            Path(
                f"/tmp/xdg-test/a3s-code/{_bootstrap.__version__}/"
                f"{_bootstrap._platform_tag()}"
            ),
        )

    def test_explicit_override_wins(self):
        os.environ["XDG_CACHE_HOME"] = "/tmp/xdg-test"
        os.environ["A3S_CODE_CACHE_DIR"] = "/var/a3s-cache"
        cache = _bootstrap._cache_root()
        self.assertEqual(
            cache,
            Path(
                f"/var/a3s-cache/{_bootstrap.__version__}/"
                f"{_bootstrap._platform_tag()}"
            ),
        )


class ExtractNativeTests(unittest.TestCase):
    def test_extracts_native_extension(self):
        wheel_bytes = _make_wheel(b"native-bytes")
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "a3s_code"
            out = _bootstrap._extract_native(wheel_bytes, target)
            self.assertTrue(out.exists())
            self.assertTrue(out.name.startswith("_native."))
            self.assertEqual(out.read_bytes(), b"native-bytes")

    def test_wheel_without_native_raises(self):
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w") as zf:
            zf.writestr("a3s_code/__init__.py", "")
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(_bootstrap.BootstrapError):
                _bootstrap._extract_native(buf.getvalue(), Path(tmp) / "pkg")


class CachedNativeSelectionTests(unittest.TestCase):
    def test_selects_exact_interpreter_before_abi3(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            (cache / "_native.abi3.so").write_bytes(b"abi3")
            (cache / "_native.cpython-312-darwin.so").write_bytes(b"exact")
            with mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 12)):
                selected = _bootstrap._find_cached_native(cache)
            self.assertIsNotNone(selected)
            self.assertEqual(selected.name, "_native.cpython-312-darwin.so")

    def test_does_not_reuse_another_python_minor(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            (cache / "_native.cpython-313-darwin.so").write_bytes(b"wrong")
            with mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 14)):
                self.assertIsNone(_bootstrap._find_cached_native(cache))

    def test_uses_abi3_when_exact_is_absent(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            (cache / "_native.abi3.so").write_bytes(b"abi3")
            with mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 14)):
                selected = _bootstrap._find_cached_native(cache)
            self.assertIsNotNone(selected)
            self.assertEqual(selected.name, "_native.abi3.so")


class EnsureNativeLoadedTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.mkdtemp(prefix="a3s-bootstrap-test-")
        self._prev_cache = os.environ.get("A3S_CODE_CACHE_DIR")
        os.environ["A3S_CODE_CACHE_DIR"] = self._tmp
        # The product intentionally rejects CPython < 3.10.  macOS ships a
        # system Python 3.9 on some runners, though, and these tests exercise
        # download/cache behavior rather than the version-floor error path.
        # Run the fixture under a supported simulated minor in that case so
        # the suite remains useful when invoked with the system interpreter.
        self._supported_version_patch = None
        if (sys.version_info.major, sys.version_info.minor) < _bootstrap._MIN_PYTHON:
            self._supported_version_patch = mock.patch.object(
                sys, "version_info", _FakeVersionInfo(3, 12)
            )
            self._supported_version_patch.start()
        # Reset the module-level latch so each test starts clean.
        _bootstrap._LOADED = False

    def tearDown(self):
        if self._prev_cache is None:
            os.environ.pop("A3S_CODE_CACHE_DIR", None)
        else:
            os.environ["A3S_CODE_CACHE_DIR"] = self._prev_cache
        if self._supported_version_patch is not None:
            self._supported_version_patch.stop()
        shutil.rmtree(self._tmp, ignore_errors=True)
        _bootstrap._LOADED = False

    def test_downloads_extracts_and_registers_module(self):
        wheel_bytes = _make_wheel()
        expected_sha = hashlib.sha256(wheel_bytes).hexdigest()

        manifest = json.dumps(
            {
                "version": "3.2.1",
                "assets": [
                    {
                        "filename": _bootstrap._wheel_candidates("3.2.1")[0],
                        "sha256": expected_sha,
                    }
                ],
            }
        ).encode()

        def fake_get(url: str) -> bytes:
            if url.endswith("python-native-manifest.json"):
                return manifest
            return wheel_bytes

        # Patch _register_native so the test doesn't actually try to load
        # the fake `_native.*.so` (which is not a real shared object).
        with (
            mock.patch.object(_bootstrap, "_http_get", side_effect=fake_get),
            mock.patch.object(_bootstrap, "_register_native") as register_mock,
        ):
            cache = _bootstrap.ensure_native_loaded("3.2.1")

        # `_cache_root()` keys the cache dir on the module's own version and
        # current platform, not the version arg passed to `ensure_native_loaded`.
        self.assertEqual(
            cache,
            Path(self._tmp)
            / _bootstrap.__version__
            / _bootstrap._platform_tag(),
        )
        # Native file is extracted directly into the platform-scoped cache.
        extracted = list(cache.glob("_native.*"))
        self.assertEqual(len(extracted), 1)
        register_mock.assert_called_once_with(extracted[0])

    def test_sha256_mismatch_raises(self):
        wheel_bytes = _make_wheel()
        manifest = json.dumps(
            {
                "version": "3.2.1",
                "assets": [
                    {
                        "filename": _bootstrap._wheel_candidates("3.2.1")[0],
                        "sha256": "0" * 64,
                    }
                ],
            }
        ).encode()

        def fake_get(url: str) -> bytes:
            if url.endswith("python-native-manifest.json"):
                return manifest
            return wheel_bytes

        with (
            mock.patch.object(_bootstrap, "_http_get", side_effect=fake_get),
            mock.patch.object(_bootstrap, "_register_native"),
        ):
            with self.assertRaises(_bootstrap.BootstrapError) as cm:
                _bootstrap.ensure_native_loaded("3.2.1")
        self.assertIn("sha256 mismatch", str(cm.exception))

    def test_skip_hash_check_env(self):
        wheel_bytes = _make_wheel()
        manifest = json.dumps(
            {
                "version": "3.2.1",
                "assets": [
                    {
                        "filename": _bootstrap._wheel_candidates("3.2.1")[0],
                        "sha256": "0" * 64,
                    }
                ],
            }
        ).encode()

        def fake_get(url: str) -> bytes:
            if url.endswith("python-native-manifest.json"):
                return manifest
            return wheel_bytes

        os.environ["A3S_CODE_SKIP_HASH_CHECK"] = "1"
        try:
            with (
                mock.patch.object(_bootstrap, "_http_get", side_effect=fake_get),
                mock.patch.object(_bootstrap, "_register_native"),
            ):
                _bootstrap.ensure_native_loaded("3.2.1")
        finally:
            os.environ.pop("A3S_CODE_SKIP_HASH_CHECK", None)

    def test_idempotent_after_first_call(self):
        wheel_bytes = _make_wheel()
        manifest = json.dumps(
            {
                "version": "3.2.1",
                "assets": [
                    {
                        "filename": _bootstrap._wheel_candidates("3.2.1")[0],
                        "sha256": hashlib.sha256(wheel_bytes).hexdigest(),
                    }
                ],
            }
        ).encode()

        call_count = {"n": 0}

        def fake_get(url: str) -> bytes:
            call_count["n"] += 1
            if url.endswith("python-native-manifest.json"):
                return manifest
            return wheel_bytes

        with (
            mock.patch.object(_bootstrap, "_http_get", side_effect=fake_get),
            mock.patch.object(_bootstrap, "_register_native"),
        ):
            _bootstrap.ensure_native_loaded("3.2.1")
            calls_after_first = call_count["n"]
            _bootstrap.ensure_native_loaded("3.2.1")
            self.assertEqual(call_count["n"], calls_after_first,
                             "second call must not re-download")

    def test_python314_selects_abi3_wheel(self):
        wheel_bytes = _make_wheel()
        with mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 14)):
            abi3_name = _bootstrap._abi3_wheel_filename("3.2.1")
            manifest = json.dumps(
                {
                    "version": "3.2.1",
                    "assets": [
                        {
                            "filename": abi3_name,
                            "sha256": hashlib.sha256(wheel_bytes).hexdigest(),
                        }
                    ],
                }
            ).encode()

            calls: list[str] = []

            def fake_get(url: str) -> bytes:
                calls.append(url)
                if url.endswith("python-native-manifest.json"):
                    return manifest
                return wheel_bytes

            with (
                mock.patch.object(_bootstrap, "_http_get", side_effect=fake_get),
                mock.patch.object(_bootstrap, "_register_native") as register_mock,
            ):
                _bootstrap.ensure_native_loaded("3.2.1")

        self.assertIn(abi3_name, calls[0])
        register_mock.assert_called_once()

    def test_python314_falls_back_to_exact_wheel_on_404(self):
        wheel_bytes = _make_wheel()
        with mock.patch.object(sys, "version_info", _FakeVersionInfo(3, 14)):
            abi3_name = _bootstrap._abi3_wheel_filename("3.2.1")
            exact_name = _bootstrap._wheel_filename("3.2.1")
            manifest = json.dumps(
                {
                    "version": "3.2.1",
                    "assets": [
                        {
                            "filename": exact_name,
                            "sha256": hashlib.sha256(wheel_bytes).hexdigest(),
                        }
                    ],
                }
            ).encode()

            calls: list[str] = []

            def fake_get(url: str) -> bytes:
                calls.append(url)
                if url.endswith("python-native-manifest.json"):
                    return manifest
                if abi3_name in url:
                    raise _bootstrap.BootstrapHttpError(url, 404)
                return wheel_bytes

            with (
                mock.patch.object(_bootstrap, "_http_get", side_effect=fake_get),
                mock.patch.object(_bootstrap, "_register_native") as register_mock,
            ):
                _bootstrap.ensure_native_loaded("3.2.1")

        self.assertIn(abi3_name, calls[0])
        self.assertIn(exact_name, calls[1])
        register_mock.assert_called_once()


@unittest.skipUnless(
    os.environ.get("A3S_CODE_BOOTSTRAP_LIVE") == "1",
    "set A3S_CODE_BOOTSTRAP_LIVE=1 to exercise the live download path against GH Releases",
)
class LiveDownloadTests(unittest.TestCase):
    def test_live_fetch_v3_2_0(self):
        # 3.2.0 has native wheels on GH Release — pick whichever compatible
        # asset is available for the current runner. Older releases predate
        # the stable-ABI migration, so a newer interpreter may legitimately
        # have no matching asset and should be reported as a skip.
        try:
            _bootstrap._wheel_candidates("3.2.0")
        except _bootstrap.BootstrapError as exc:
            self.skipTest(str(exc))
        tmp = Path(tempfile.mkdtemp(prefix="a3s-live-"))
        try:
            os.environ["A3S_CODE_CACHE_DIR"] = str(tmp)
            _bootstrap._LOADED = False
            with mock.patch.object(_bootstrap, "_register_native"):
                try:
                    cache = _bootstrap.ensure_native_loaded("3.2.0")
                except _bootstrap.BootstrapError as exc:
                    self.skipTest(str(exc))
            self.assertTrue(any(cache.glob("_native.*")))
        finally:
            os.environ.pop("A3S_CODE_CACHE_DIR", None)
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
