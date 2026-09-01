"""Remove generated A3S Code native extensions before a development build."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from types import ModuleType


SDK_ROOT = Path(__file__).resolve().parents[1]
ARTIFACT_MODULE = SDK_ROOT / "python" / "a3s_code" / "_native_artifacts.py"
PACKAGE_DIR = SDK_ROOT / "python" / "a3s_code"


def _load_artifact_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "_a3s_code_native_artifacts", ARTIFACT_MODULE
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Cannot load native artifact helper: {ARTIFACT_MODULE}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> None:
    module = _load_artifact_module()
    removed = module.clean_native_extension_artifacts(PACKAGE_DIR)
    if not removed:
        print("No generated A3S Code native extensions found.")
        return
    for path in removed:
        print(f"Removed rebuildable native artifact: {path}")


if __name__ == "__main__":
    main()
