"""A3S Code Python SDK.

This is the pure-Python bootstrap distributed on PyPI. The actual native
extension is fetched from GitHub Releases on first import — see
`a3s_code._bootstrap` for the loader logic and the environment variables
that customize cache location / source URL.
"""

from . import _bootstrap as _bootstrap

_bootstrap.ensure_native_loaded()

# The bootstrap extracted `_native.<abi>.so` into a per-version cache and
# registered it explicitly as `a3s_code._native` before returning.
from ._native import *  # noqa: E402,F401,F403

__version__ = _bootstrap.__version__
