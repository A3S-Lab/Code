from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
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
