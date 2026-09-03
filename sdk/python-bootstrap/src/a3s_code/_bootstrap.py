"""Lazy loader for the a3s-code native extension.

This module is part of the pure-Python bootstrap published to PyPI under
the `a3s-code` name. On first import it resolves a compatible native
wheel for the current platform, downloads it from the project's GitHub
Releases, verifies the wheel's sha256 against the
release manifest, extracts the compiled `_native` extension into a
per-user cache, and registers it explicitly as `a3s_code._native` because it
lives outside the installed package directory. A cross-process advisory lock
keeps the first download and sidecar extraction single-flight for applications
sharing that cache.

Override the cache location via `A3S_CODE_CACHE_DIR`. Override the
release source via `A3S_CODE_RELEASES_BASE_URL` (default points at the
GitHub Releases page for `A3S-Lab/Code`). Skip the integrity check via
`A3S_CODE_SKIP_HASH_CHECK=1` (not recommended outside of CI).
"""

from __future__ import annotations

import errno
import hashlib
import io
import json
import os
import platform
import stat
import sys
import threading
import time
import urllib.error
import urllib.request
import zipfile
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator, Optional

# Version is the bootstrap's own version, which equals the matching native
# wheel version on GH Releases. Bumped by the release workflow.
__version__ = "8.2.0"

_DEFAULT_BASE_URL = "https://github.com/A3S-Lab/Code/releases/download"
_REQUEST_TIMEOUT_S = 120
_USER_AGENT = f"a3s-code-bootstrap/{__version__}"
_MIN_PYTHON = (3, 10)
_MAX_WHEEL_BYTES = 512 * 1024 * 1024
_MAX_MEMBER_BYTES = 256 * 1024 * 1024
_ABI3_PYTHON_TAG = "cp310-abi3"
_INSTALL_LOCK_TIMEOUT_S = _REQUEST_TIMEOUT_S + 30
_INSTALL_LOCK_POLL_S = 0.05
_LOAD_LOCK = threading.Lock()
_LOADED = False


class BootstrapError(RuntimeError):
    """Raised when the native extension cannot be located, downloaded, or verified."""


class BootstrapHttpError(BootstrapError):
    """HTTP failure with a status code that callers can safely classify."""

    def __init__(self, url: str, status_code: int):
        self.url = url
        self.status_code = status_code
        super().__init__(f"GET {url} failed: HTTP {status_code}")


def _lock_contention(error: OSError) -> bool:
    """Return whether an advisory-lock failure means another process owns it."""

    # POSIX flock reports EACCES/EAGAIN. Windows msvcrt.locking commonly uses
    # errno 13 (permission denied) for the same contention condition.
    return isinstance(error, BlockingIOError) or getattr(error, "errno", None) in {
        errno.EAGAIN,
        errno.EACCES,  # Windows sharing violation uses the same value
        35,  # EAGAIN on macOS
        36,  # EAGAIN on some BSDs
    }


@contextmanager
def _install_lock(
    cache_dir: Path, timeout_s: float = _INSTALL_LOCK_TIMEOUT_S
) -> Iterator[None]:
    """Acquire a cross-process lock for native wheel and Moli extraction.

    The lock is advisory and released by the operating system if a bootstrap
    process exits unexpectedly. Keeping it in the same platform/version cache
    means independent Python applications perform one download and one atomic
    extraction, while the in-process ``_LOAD_LOCK`` still protects module
    registration.
    """

    cache_dir.mkdir(parents=True, exist_ok=True)
    lock_path = cache_dir / ".install.lock"
    try:
        handle = lock_path.open("a+b")
    except OSError as exc:
        raise BootstrapError(f"could not open bootstrap install lock {lock_path}: {exc}") from exc

    locked = False
    try:
        # Windows locking operates on a byte range and requires at least one
        # byte in the file. The write is harmless on POSIX and races are safe.
        handle.seek(0, os.SEEK_END)
        if handle.tell() == 0:
            handle.write(b"\0")
            handle.flush()
        deadline = time.monotonic() + timeout_s
        while True:
            try:
                handle.seek(0)
                if os.name == "nt":
                    import msvcrt

                    msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl

                    fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                locked = True
                break
            except OSError as exc:
                if not _lock_contention(exc):
                    raise BootstrapError(
                        f"could not acquire bootstrap install lock {lock_path}: {exc}"
                    ) from exc
                if time.monotonic() >= deadline:
                    raise BootstrapError(
                        f"timed out waiting for bootstrap install lock {lock_path}"
                    ) from exc
                time.sleep(_INSTALL_LOCK_POLL_S)
        yield
    finally:
        if locked:
            try:
                handle.seek(0)
                if os.name == "nt":
                    import msvcrt

                    msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    import fcntl

                    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
            except OSError:
                # Closing the descriptor also releases the advisory lock. Do
                # not hide the caller's extraction/import exception here.
                pass
        handle.close()


def _base_url() -> str:
    return os.environ.get("A3S_CODE_RELEASES_BASE_URL", _DEFAULT_BASE_URL).rstrip("/")


def _cache_root() -> Path:
    override = os.environ.get("A3S_CODE_CACHE_DIR")
    if override:
        base = Path(override).expanduser() / __version__
    else:
        base = (
            Path(os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache"))
            / "a3s-code"
            / __version__
        )
    # Keep native binaries for different architectures/OS baselines isolated.
    # This matters when an Apple Silicon and an Intel interpreter share a home
    # directory (for example, under Rosetta); a cached arm64 extension must
    # never be handed to an x86_64 process.
    return base / _platform_tag()


def _platform_tag() -> str:
    """Return the wheel platform tag for the current interpreter/platform.

    Mirrors the matrix produced by `.github/workflows/publish-python.yml`.
    Raises `BootstrapError` for unsupported combinations so callers can
    surface a clear install hint.
    """
    sys_plat = sys.platform
    machine = platform.machine().lower()
    if sys_plat == "darwin":
        if machine in ("arm64", "aarch64"):
            return "macosx_11_0_arm64"
        if machine in ("x86_64", "amd64"):
            # Intel wheels are built with MACOSX_DEPLOYMENT_TARGET=12.0 so
            # they can run on supported Intel Macs back to macOS 12.
            return "macosx_12_0_x86_64"
    elif sys_plat == "linux":
        if machine in ("x86_64", "amd64"):
            return "manylinux_2_28_x86_64"
        if machine in ("arm64", "aarch64"):
            return "manylinux_2_28_aarch64"
    elif sys_plat == "win32":
        if machine in ("amd64", "x86_64"):
            return "win_amd64"
        if machine in ("arm64", "aarch64"):
            return "win_arm64"
    raise BootstrapError(
        f"a3s-code: no native wheel published for {sys_plat}/{machine}. "
        "Supported platforms: macOS arm64 (11+), macOS Intel (12+), "
        "Linux x86_64/arm64 (glibc 2.28+), Windows x86_64/arm64."
    )


def _wheel_filename(version: str = __version__) -> str:
    if platform.python_implementation() != "CPython":
        raise BootstrapError(
            "a3s-code publishes native wheels for CPython only; "
            f"found {platform.python_implementation()}"
        )
    if (sys.version_info.major, sys.version_info.minor) < _MIN_PYTHON:
        raise BootstrapError(
            "a3s-code requires CPython 3.10 or newer; "
            f"found {sys.version_info.major}.{sys.version_info.minor}"
        )
    py_tag = f"cp{sys.version_info.major}{sys.version_info.minor}"
    return f"a3s_code-{version}-{py_tag}-{py_tag}-{_platform_tag()}.whl"


def _abi3_wheel_filename(version: str = __version__) -> str:
    """Return the stable-ABI wheel name used by current releases."""
    if platform.python_implementation() != "CPython":
        raise BootstrapError(
            "a3s-code publishes native wheels for CPython only; "
            f"found {platform.python_implementation()}"
        )
    if (sys.version_info.major, sys.version_info.minor) < _MIN_PYTHON:
        raise BootstrapError(
            "a3s-code requires CPython 3.10 or newer; "
            f"found {sys.version_info.major}.{sys.version_info.minor}"
        )
    return f"a3s_code-{version}-{_ABI3_PYTHON_TAG}-{_platform_tag()}.whl"


def _wheel_candidates(version: str = __version__) -> tuple[str, ...]:
    """Return compatible wheel names in migration-safe preference order.

    Existing releases contain one wheel per CPython minor, so Python 3.10–3.13
    keep using those exact assets first.  New releases publish a single
    ``cp310-abi3`` wheel; Python 3.14+ prefers that asset and all interpreters
    fall back to it when an older release has no exact match.
    """
    exact = _wheel_filename(version)
    abi3 = _abi3_wheel_filename(version)
    if sys.version_info.minor >= 14:
        return (abi3, exact)
    return (exact, abi3)


def _release_url(filename: str, version: str = __version__) -> str:
    return f"{_base_url()}/v{version}/{filename}"


def _http_get(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": _USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=_REQUEST_TIMEOUT_S) as resp:
            return resp.read()
    except urllib.error.HTTPError as exc:
        raise BootstrapHttpError(url, exc.code) from exc
    except urllib.error.URLError as exc:
        raise BootstrapError(f"GET {url} failed: {exc.reason}") from exc


def _expected_sha256(wheel_name: str, version: str = __version__) -> Optional[str]:
    """Look up the published sha256 for `wheel_name` in the release manifest.

    Returns `None` if the manifest is unreachable — bootstrap will then
    proceed without integrity verification but emit a warning. Override
    with `A3S_CODE_SKIP_HASH_CHECK=1` for hermetic offline mirrors.
    """
    manifest_url = f"{_base_url()}/v{version}/python-native-manifest.json"
    try:
        data = json.loads(_http_get(manifest_url))
    except BootstrapError as exc:
        sys.stderr.write(
            f"a3s-code: warning: manifest fetch failed ({exc}); skipping hash check\n"
        )
        return None
    for asset in data.get("assets", []):
        if asset.get("filename") == wheel_name:
            return asset.get("sha256")
    return None


def _download_wheel(version: str) -> tuple[str, bytes]:
    """Download the first compatible wheel, retaining old-release fallback."""
    candidates = _wheel_candidates(version)
    not_found: list[str] = []
    for wheel_name in candidates:
        try:
            return wheel_name, _http_get(_release_url(wheel_name, version))
        except BootstrapHttpError as exc:
            if exc.status_code == 404:
                not_found.append(wheel_name)
                continue
            raise

    tried = ", ".join(not_found)
    raise BootstrapError(
        "a3s-code: no compatible native wheel was published for "
        f"CPython {sys.version_info.major}.{sys.version_info.minor} on "
        f"{sys.platform}/{platform.machine()}. Tried: {tried}"
    )


def _safe_wheel_member(name: str) -> tuple[str, ...]:
    """Validate and split a wheel member before reading it.

    Wheels are ZIP files supplied by a remote release.  Do not allow an
    archive entry to escape the cache through absolute paths, parent
    components, or Windows separators (which are accepted by some extraction
    tools even on POSIX).
    """
    if not name or "\\" in name or name.startswith("/"):
        raise BootstrapError(f"downloaded wheel contains an unsafe member path: {name!r}")
    parts = tuple(part for part in name.split("/") if part)
    if not parts or any(part in {".", ".."} for part in parts):
        raise BootstrapError(f"downloaded wheel contains an unsafe member path: {name!r}")
    return parts


def _write_wheel_member(zf: zipfile.ZipFile, info: zipfile.ZipInfo, destination: Path) -> None:
    """Write one regular, bounded wheel member atomically."""
    mode = (info.external_attr >> 16) & 0o170000
    if mode == stat.S_IFLNK:
        raise BootstrapError(f"downloaded wheel contains a symlink member: {info.filename!r}")
    if info.is_dir() or info.file_size > _MAX_MEMBER_BYTES:
        raise BootstrapError(f"downloaded wheel member is too large or not a file: {info.filename!r}")

    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(f".{destination.name}.part-{os.getpid()}-{threading.get_ident()}")
    try:
        with zf.open(info) as source, temporary.open("xb") as target:
            remaining = _MAX_MEMBER_BYTES + 1
            while remaining:
                chunk = source.read(min(1024 * 1024, remaining))
                if not chunk:
                    break
                target.write(chunk)
                remaining -= len(chunk)
            if remaining == 0:
                raise BootstrapError(f"downloaded wheel member is too large: {info.filename!r}")
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, destination)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _configure_cached_moli(cache_dir: Path) -> None:
    """Expose a wheel-bundled Moli executable to the Rust core.

    The bootstrap package and the native wheel are separate distributions.
    Without copying this sidecar into the bootstrap cache, a normal
    ``pip install a3s-code`` would silently discard the bundled browser and
    force every process to download it again.  An operator-supplied override
    always wins.
    """
    if os.environ.get("A3S_CODE_MOLI_EXECUTABLE"):
        return
    executable = "moli.exe" if os.name == "nt" else "moli"
    candidate = cache_dir / "moli" / executable
    try:
        if candidate.is_file() and (os.name == "nt" or os.access(candidate, os.X_OK)):
            os.environ["A3S_CODE_MOLI_EXECUTABLE"] = str(candidate)
            os.environ.setdefault("A3S_CODE_MOLI_DIR", str(candidate.parent))
    except OSError:
        return


def _extract_native(wheel_bytes: bytes, target_dir: Path) -> Path:
    """Extract the native extension and bundled Moli sidecar.

    Returns the path to the extracted extension.  The sidecar is placed under
    ``target_dir/moli`` and is selected by :func:`_configure_cached_moli`.
    """
    if len(wheel_bytes) > _MAX_WHEEL_BYTES:
        raise BootstrapError("downloaded wheel exceeds the bootstrap size limit")
    target_dir.mkdir(parents=True, exist_ok=True)
    native_path: Optional[Path] = None
    moli_path: Optional[Path] = None
    with zipfile.ZipFile(io.BytesIO(wheel_bytes)) as zf:
        for info in zf.infolist():
            parts = _safe_wheel_member(info.filename)
            base = parts[-1]
            # Match _native.<abi>.{so,pyd,dylib}; ignore metadata files.
            if base.startswith("_native.") and not base.endswith(".dist-info"):
                if native_path is not None:
                    raise BootstrapError("downloaded wheel contains multiple native extensions")
                native_path = target_dir / base
                _write_wheel_member(zf, info, native_path)
                continue

            # Maturin places the packaged browser under a3s_code/moli/. Keep
            # the match strict so an unrelated file named `moli` is not run.
            if base in {"moli", "moli.exe"} and len(parts) >= 3 and parts[-3:-1] == ("a3s_code", "moli"):
                if moli_path is not None:
                    raise BootstrapError("downloaded wheel contains multiple Moli executables")
                moli_path = target_dir / "moli" / base
                _write_wheel_member(zf, info, moli_path)

    if native_path is None:
        raise BootstrapError(
            "downloaded wheel did not contain a _native extension; "
            "the release artifact appears to be corrupt"
        )
    if moli_path is not None and os.name != "nt":
        try:
            moli_path.chmod(0o755)
        except OSError as exc:
            raise BootstrapError(f"could not mark bundled Moli executable: {exc}") from exc
    _configure_cached_moli(target_dir)
    return native_path


def _find_cached_native(cache_dir: Path) -> Optional[Path]:
    if not cache_dir.is_dir():
        return None
    exact_tag = f"cp{sys.version_info.major}{sys.version_info.minor}"
    cpython_tag = f"cpython-{sys.version_info.major}{sys.version_info.minor}"
    exact: list[Path] = []
    abi3: list[Path] = []
    for child in sorted(cache_dir.iterdir()):
        if not child.is_file() or not child.name.startswith("_native."):
            continue
        name = child.name.lower()
        if f".{exact_tag}" in name or cpython_tag in name:
            exact.append(child)
        elif ".abi3." in name or name.endswith(".abi3"):
            abi3.append(child)
    # Prefer a native build tied to this interpreter, then the stable ABI.
    if exact:
        return exact[0]
    if abi3:
        return abi3[0]
    return None


def _register_native(native_path: Path) -> None:
    """Load `native_path` as the `a3s_code._native` module.

    `_native` is a compiled extension, not a regular Python file, so use
    `importlib.machinery.ExtensionFileLoader` + the matching spec instead
    of plain `spec_from_file_location` (which works but doesn't set the
    right loader for extensions on all Python versions).
    """
    import importlib.machinery
    import importlib.util

    fullname = "a3s_code._native"
    loader = importlib.machinery.ExtensionFileLoader(fullname, str(native_path))
    spec = importlib.util.spec_from_loader(fullname, loader, origin=str(native_path))
    if spec is None:
        raise BootstrapError(f"failed to build import spec for {native_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[fullname] = module
    spec.loader.exec_module(module)


def ensure_native_loaded(version: str = __version__) -> Path:
    """Idempotently ensure the `_native` extension is registered as
    `a3s_code._native` in `sys.modules`. Returns the cache directory the
    extension was loaded from. Safe across threads — first caller wins.
    """
    global _LOADED
    cache = _cache_root()

    if _LOADED:
        return cache

    with _LOAD_LOCK:
        if _LOADED:
            return cache

        native = _find_cached_native(cache)
        if native is None:
            # Re-check after taking the process lock: another independent
            # Python application may have completed the download while this
            # process was starting. This keeps both the extension and bundled
            # Moli sidecar single-flight across processes.
            with _install_lock(cache):
                native = _find_cached_native(cache)
                if native is None:
                    wheel_name, wheel_bytes = _download_wheel(version)
                    url = _release_url(wheel_name, version)
                    sys.stderr.write(
                        f"a3s-code: fetching native wheel {wheel_name} "
                        f"from {url} (first import only)...\n"
                    )

                    if os.environ.get("A3S_CODE_SKIP_HASH_CHECK") != "1":
                        expected = _expected_sha256(wheel_name, version)
                        if expected is not None:
                            actual = hashlib.sha256(wheel_bytes).hexdigest()
                            if actual != expected:
                                raise BootstrapError(
                                    f"sha256 mismatch for {wheel_name}: "
                                    f"expected {expected}, got {actual}"
                                )

                    native = _extract_native(wheel_bytes, cache)
                else:
                    _configure_cached_moli(cache)
        else:
            _configure_cached_moli(cache)

        _register_native(native)
        _LOADED = True
        return cache
