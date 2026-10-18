#!/bin/bash
# Remove SDK-only monorepo patches; keep the published workspace intact.
set -euo pipefail

python3 - <<'PYTHON'
import re
from pathlib import Path

root = Path.cwd().resolve()
for relative in ("Cargo.toml", "core/Cargo.toml", "acp/Cargo.toml",
                 "sdk/go/bridge/Cargo.toml", "sdk/node/Cargo.toml", "sdk/python/Cargo.toml"):
    path = Path(relative)
    original = path.read_text()
    kept = []
    dropping = False
    for line in original.splitlines(keepends=True):
        if line.lstrip().startswith("["):
            dropping = line.lstrip().startswith("[patch.")
        if not dropping:
            kept.append(line)
    text = "".join(kept)
    for source in re.findall(r'\bpath\s*=\s*"([^"\n]+)"', text):
        resolved = (path.parent / source).resolve()
        if resolved != root and root not in resolved.parents:
            raise SystemExit(f"{relative}: dependency path escapes standalone workspace: {source}")
    if text != original:
        path.write_text(text)
        print(f"removed monorepo patches from {relative}")
print("standalone workspace keeps Core, ACP and the Go bridge")
PYTHON
