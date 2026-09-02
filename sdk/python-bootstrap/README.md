# a3s-code (Python bootstrap)

`pip install a3s-code` ships this small pure-Python package. On first
`import a3s_code` it downloads the native extension matching your
platform from the project's
[GitHub Releases](https://github.com/A3S-Lab/Code/releases), verifies
the wheel's sha256 against the release manifest, extracts the compiled
extension into a per-user cache, and exposes the normal `a3s_code` API.

The v8.1.0 release uses one CPython 3.10 stable-ABI (`cp310-abi3`) wheel per
platform. That wheel is installable by CPython 3.10, 3.11, 3.12, 3.13, and
3.14. Supported native targets are macOS arm64/x86_64, Linux glibc 2.28+
arm64/x86_64, and Windows arm64/x86_64. Every supported wheel contains its
target Moli sidecar and provenance record. Linux musl is intentionally omitted
because upstream Moli publishes no musl asset; use a system/explicit browser
or another backend there. The loader still falls back to exact per-minor names
when reading older release manifests.

Subsequent imports use the cached extension. Cache lives under a
platform-specific directory, `~/.cache/a3s-code/<version>/<platform-tag>/`
(or `$A3S_CODE_CACHE_DIR/<version>/<platform-tag>/` when overridden).
The extracted Moli executable is placed in the same version/target cache and
is selected through `A3S_CODE_MOLI_EXECUTABLE`; atomic extraction and the
bootstrap's cross-process installer lock make concurrent first imports
single-flight. Rust, Node.js, and Go use the Core-managed Moli cache when they
are not using a package-local sidecar.

## Why

PyPI imposes a default 10 GB per-project storage cap. A Rust SDK with
~17 MB native wheels per Python × platform tripped that limit. GitHub
Releases is the canonical wheel host; this bootstrap keeps
`pip install a3s-code` working without dragging the native wheels back
through PyPI.

## Supported platforms

- macOS arm64 (Apple Silicon, macOS 11+)
- macOS x86_64 (Intel, macOS 12+)
- Linux x86_64 and arm64 (glibc 2.28+)
- Windows x86_64 and arm64

CPython 3.10, 3.11, 3.12, 3.13, and 3.14.

The bootstrap itself needs a working pip in the interpreter that runs it. If
`python3.14 -m pip` reports `No module named pip`, initialize pip and retry:

```bash
python3.14 -m ensurepip --upgrade
python3.14 -m pip install --upgrade pip
python3.14 -m pip install a3s-code
```

## Environment overrides

| Variable | Effect |
|---|---|
| `A3S_CODE_CACHE_DIR` | Cache root (defaults to `$XDG_CACHE_HOME/a3s-code` or `~/.cache/a3s-code`) |
| `A3S_CODE_RELEASES_BASE_URL` | Override the release base URL — useful for air-gapped mirrors |
| `A3S_CODE_SKIP_HASH_CHECK` | `1` skips sha256 verification (do not use in production) |
| `A3S_CODE_MOLI_EXECUTABLE` | Explicit verified Moli executable; skips package discovery |
| `A3S_CODE_MOLI_CACHE_DIR` | Core-managed shared Moli cache root (the bootstrap uses its version/target cache for a bundled sidecar) |
| `A3S_CODE_MOLI_RELEASE_BASE_URL` | HTTPS mirror for the pinned Moli release |

## Manual install

If you do not want the bootstrap to phone home, install the native
wheel directly:

```bash
pip install \
  'https://github.com/A3S-Lab/Code/releases/download/v<VERSION>/a3s_code-<VERSION>-cp310-abi3-manylinux_2_28_x86_64.whl'
```

For an Intel Mac on macOS 12 or later, use the `macosx_12_0_x86_64` asset:

```bash
pip install \
  'https://github.com/A3S-Lab/Code/releases/download/v<VERSION>/a3s_code-<VERSION>-cp310-abi3-macosx_12_0_x86_64.whl'
```

Replace `<VERSION>` with the release to install. The Intel asset is available
from the first release produced with the Intel build matrix.

The Intel macOS 12 build does not ship the optional local ONNX embedding
adapter. Keep retrieval model-free or configure an explicitly authorized
remote embedding provider when using that platform.
