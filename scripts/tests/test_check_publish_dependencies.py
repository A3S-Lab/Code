from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError


SCRIPT = Path(__file__).parents[1] / "check_publish_dependencies.py"
SPEC = importlib.util.spec_from_file_location("check_publish_dependencies", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class StubResponse:
    status = 200

    def __enter__(self) -> "StubResponse":
        return self

    def __exit__(self, *_args: object) -> None:
        return None


class PublishDependencyPreflightTests(unittest.TestCase):
    def test_cli_reports_all_manifests_and_checks_valid_dependencies_after_errors(self) -> None:
        requests: list[str] = []

        class Registry(BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                requests.append(self.path)
                self.send_response(
                    404 if "/missing/" in self.path or "/fixture-core/" in self.path else 200
                )
                self.end_headers()

            def log_message(self, *_args: object) -> None:
                pass

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            core = root / "core" / "Cargo.toml"
            core.parent.mkdir()
            package = '[package]\nname = "fixture-core"\nversion = "9.1.3"\n'
            core.write_text(
                package + '[dependencies]\nprivate = { path = "../private" }\n'
                'shared = { version = "=1.2.3", git = "https://example.test/shared" }\n'
                '[target.\'cfg(unix)\'.build-dependencies]\n'
                'bad_range = { path = "../range", version = "^1.0" }\n'
                'invalid_name = { path = "../invalid", version = "1.0.0", package = 42 }\n'
            )
            acp = root / "acp" / "Cargo.toml"
            acp.parent.mkdir()
            internal = 'core = { package = "fixture-core", version = "9.1.3", path = "../core" }\n'
            acp.write_text(
                '[dependencies]\nshared = { version = "=1.2.3", path = "../shared" }\n'
                'missing = { version = "0.2.4", path = "../missing" }\n'
                + internal
                + 'wrong_version = { package = "fixture-core", version = "0.0.0", path = "../core" }\n'
                'counterfeit = { package = "fixture-core", version = "9.1.3", path = "../counterfeit" }\n'
            )
            broken = root / "broken.toml"
            broken.write_text("[dependencies\n")
            absent = root / "absent.toml"
            registry = ThreadingHTTPServer(("127.0.0.1", 0), Registry)
            thread = threading.Thread(target=registry.serve_forever, daemon=True)
            thread.start()
            try:
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), str(core), str(acp), str(broken),
                     str(absent), "--candidate-manifest", str(core), "--api-base",
                     f"http://127.0.0.1:{registry.server_port}"],
                    capture_output=True, text=True, timeout=10,
                )
                failed_requests = requests.copy()
                requests.clear()
                core.write_text(package)
                acp.write_text("[dependencies]\n" + internal)
                valid = subprocess.run(
                    [sys.executable, str(SCRIPT), str(core), str(acp),
                     "--candidate-manifest", str(core), "--api-base",
                     f"http://127.0.0.1:{registry.server_port}"],
                    capture_output=True, text=True, timeout=10,
                )
            finally:
                registry.shutdown()
                registry.server_close()
                thread.join(timeout=5)

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        for detail in ("private", "bad_range", "invalid_name", "broken.toml",
                       "absent.toml", "missing 0.2.4", "HTTP 404", "wrong_version",
                       "fixture-core 9.1.3"):
            self.assertIn(detail, result.stderr)
        self.assertIn(str(acp), result.stderr)
        self.assertIn("verified shared 1.2.3", result.stdout)
        self.assertCountEqual(
            failed_requests, ["/missing/0.2.4", "/shared/1.2.3", "/fixture-core/9.1.3"]
        )
        self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
        self.assertIn("local release candidate fixture-core 9.1.3", valid.stdout)
        self.assertIn("registry delivery remains required", valid.stdout)
        self.assertEqual(requests, [])
        self.assertFalse(thread.is_alive())

    def test_path_dependency_requires_a_publishable_registry_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "Cargo.toml"
            path.write_text('[dependencies]\nlocal = { path = "../local" }\n')
            with self.assertRaisesRegex(ValueError, "Path dependency.*must declare a registry version"):
                MODULE.publish_dependencies(path)
            path.write_text('[dependencies]\nlocal = { path = "../local", version = "=1.2.3" }\n')
            self.assertEqual(MODULE.publish_dependencies(path), [MODULE.PublishDependency("local", "1.2.3")])

    def test_extracts_only_git_dependencies_with_concrete_registry_baselines(self) -> None:
        manifest = """
[package]
name = "fixture"
version = "1.0.0"

[dependencies]
plain = "1.0"
renamed = { package = "upstream", version = "=2.3.4", git = "https://example.test/repo" }
source = { version = "1.2.3", git = "https://example.test/repo" }
"""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "Cargo.toml"
            path.write_text(manifest, encoding="utf-8")
            dependencies = MODULE.publish_dependencies(path)

        self.assertEqual(
            dependencies,
            [
                MODULE.PublishDependency("source", "1.2.3"),
                MODULE.PublishDependency("upstream", "2.3.4"),
            ],
        )

    def test_rejects_git_dependency_without_a_registry_version(self) -> None:
        manifest = """
[package]
name = "fixture"
version = "1.0.0"

[dependencies]
source = { git = "https://example.test/repo" }
"""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "Cargo.toml"
            path.write_text(manifest, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "must declare a registry version"):
                MODULE.publish_dependencies(path)

    def test_registry_check_distinguishes_present_and_missing_versions(self) -> None:
        dependency = MODULE.PublishDependency("source", "1.2.3")

        def present(_request: object, timeout: int) -> StubResponse:
            self.assertEqual(timeout, 15)
            return StubResponse()

        def missing(request: object, timeout: int) -> StubResponse:
            raise HTTPError(request.full_url, 404, "Not Found", {}, None)

        self.assertIsNone(MODULE.registry_error(dependency, "https://example.test", present))
        self.assertEqual(
            MODULE.registry_error(dependency, "https://example.test", missing),
            "HTTP 404",
        )
        with self.assertRaisesRegex(ValueError, "attempts must be positive"):
            MODULE.registry_error(
                dependency, "https://example.test", present, attempts=0
            )


if __name__ == "__main__":
    unittest.main()
