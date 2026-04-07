from __future__ import annotations

import hashlib
import importlib.metadata
import importlib.util
import json
import os
import shutil
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

from packaging.tags import sys_tags
from packaging.utils import canonicalize_name, parse_wheel_filename

PACKAGE_NAME = "a3s-code"
DIST_NAME = "a3s_code"
REPO = "A3S-Lab/Code"
MANIFEST_NAME = "python-native-manifest.json"
ENV_BASE_URL = "A3S_CODE_RELEASE_BASE_URL"
ENV_CACHE_DIR = "A3S_CODE_CACHE_DIR"
ENV_OFFLINE = "A3S_CODE_OFFLINE"


def _package_version() -> str:
    return importlib.metadata.version(PACKAGE_NAME)


def _release_tag() -> str:
    return f"v{_package_version()}"


def _release_base_url() -> str:
    override = os.environ.get(ENV_BASE_URL)
    if override:
        return override.rstrip("/")
    return f"https://github.com/{REPO}/releases/download/{_release_tag()}"


def _cache_root() -> Path:
    override = os.environ.get(ENV_CACHE_DIR)
    if override:
        return Path(override).expanduser()
    if sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData/Local"))
    elif sys.platform == "darwin":
        base = Path.home() / "Library/Caches"
    else:
        base = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    return base / "a3s-code"


def _download_bytes(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "a3s-code-bootstrap/1"})
    with urllib.request.urlopen(request) as response:
        return response.read()


def _load_manifest() -> dict:
    manifest_url = f"{_release_base_url()}/{MANIFEST_NAME}"
    return json.loads(_download_bytes(manifest_url).decode("utf-8"))


def _wheel_assets(manifest: dict) -> list[dict]:
    return list(manifest.get("assets", []))


def _select_asset(assets: list[dict]) -> dict:
    tag_list = list(sys_tags())
    for tag in tag_list:
        for asset in assets:
            filename = asset["filename"]
            try:
                name, _, _, wheel_tags = parse_wheel_filename(filename)
            except Exception:
                continue
            if canonicalize_name(name) != canonicalize_name(DIST_NAME):
                continue
            if tag in wheel_tags:
                return asset
    raise ImportError("No compatible native a3s-code wheel found for this platform")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _download_wheel(asset: dict, destination: Path) -> Path:
    destination.mkdir(parents=True, exist_ok=True)
    wheel_path = destination / asset["filename"]
    if wheel_path.exists():
        if _sha256(wheel_path) == asset["sha256"]:
            return wheel_path
        wheel_path.unlink()

    if os.environ.get(ENV_OFFLINE) == "1":
        raise ImportError(f"Offline mode enabled and native wheel is missing: {wheel_path}")

    tmp_dir = Path(tempfile.mkdtemp(prefix="a3s-code-wheel-"))
    try:
        tmp_path = tmp_dir / asset["filename"]
        tmp_path.write_bytes(_download_bytes(f"{_release_base_url()}/{asset['filename']}"))
        if _sha256(tmp_path) != asset["sha256"]:
            raise ImportError(f"SHA256 mismatch for {asset['filename']}")
        tmp_path.replace(wheel_path)
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)
    return wheel_path


def _extract_native_module(wheel_path: Path, destination: Path) -> Path:
    destination.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(wheel_path) as archive:
        native_members = [
            member
            for member in archive.namelist()
            if member.startswith("a3s_code/")
            and "/_native" in member
            and member.endswith((".so", ".pyd"))
        ]
        if not native_members:
            raise ImportError(f"No native module found in wheel: {wheel_path.name}")
        member = native_members[0]
        extracted = destination / Path(member).name
        if not extracted.exists():
            with archive.open(member) as src, extracted.open("wb") as dst:
                shutil.copyfileobj(src, dst)
        return extracted


def load_native_module():
    version = _package_version()
    cache_dir = _cache_root() / version
    assets = _wheel_assets(_load_manifest())
    asset = _select_asset(assets)
    wheel_path = _download_wheel(asset, cache_dir / "wheels")
    native_path = _extract_native_module(wheel_path, cache_dir / "native")

    spec = importlib.util.spec_from_file_location("a3s_code._native", native_path)
    if spec is None or spec.loader is None:
        raise ImportError(f"Unable to load native module from {native_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["a3s_code._native"] = module
    spec.loader.exec_module(module)
    return module
