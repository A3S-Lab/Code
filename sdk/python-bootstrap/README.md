# a3s-code (Python bootstrap)

`pip install a3s-code` ships this small pure-Python package. On first
`import a3s_code` it downloads the native extension matching your
interpreter and platform from the project's
[GitHub Releases](https://github.com/A3S-Lab/Code/releases), verifies
the wheel's sha256 against the release manifest, extracts the compiled
extension into a per-user cache, and exposes the normal `a3s_code` API.

Subsequent imports use the cached extension. Cache lives under
`~/.cache/a3s-code/<version>/` (or `$XDG_CACHE_HOME/a3s-code/<version>/`).

## Why

PyPI imposes a default 10 GB per-project storage cap. A Rust SDK with
~17 MB native wheels per Python × platform tripped that limit. GitHub
Releases is the canonical wheel host; this bootstrap keeps
`pip install a3s-code` working without dragging the native wheels back
through PyPI.

## Supported platforms

- macOS arm64 (Apple Silicon, macOS 11+)
- macOS x86_64 (Intel, macOS 12+)
- Linux x86_64 (glibc 2.28+)
- Windows x86_64

CPython 3.10, 3.11, 3.12, 3.13.

Python 3.14 wheels are not published yet. Use CPython 3.13 or earlier until
the native binding is rebuilt for Python 3.14.

## Environment overrides

| Variable | Effect |
|---|---|
| `A3S_CODE_CACHE_DIR` | Cache root (defaults to `$XDG_CACHE_HOME/a3s-code` or `~/.cache/a3s-code`) |
| `A3S_CODE_RELEASES_BASE_URL` | Override the release base URL — useful for air-gapped mirrors |
| `A3S_CODE_SKIP_HASH_CHECK` | `1` skips sha256 verification (do not use in production) |

## Manual install

If you do not want the bootstrap to phone home, install the native
wheel directly:

```bash
pip install \
  'https://github.com/A3S-Lab/Code/releases/download/v<VERSION>/a3s_code-<VERSION>-cp312-cp312-manylinux_2_28_x86_64.whl'
```

For an Intel Mac on macOS 12 or later, use the `macosx_12_0_x86_64` asset:

```bash
pip install \
  'https://github.com/A3S-Lab/Code/releases/download/v<VERSION>/a3s_code-<VERSION>-cp312-cp312-macosx_12_0_x86_64.whl'
```

Replace `<VERSION>` with the release to install. The Intel asset is available
from the first release produced with the Intel build matrix.
