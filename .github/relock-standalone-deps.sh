#!/bin/bash
# Re-lock crates that the monorepo path patches had replaced.
# Run after setup-workspace.sh and after Rust is installed.
# The committed lock records path packages for a3s-apofasi, a3s-sandbox, and
# a3s-vec. Standalone CI must resolve the git rev and crates.io version
# declared in core/Cargo.toml before any --locked build.

set -euo pipefail

if grep -nE 'path = "\.\./' Cargo.toml; then
  echo "relock-standalone-deps: monorepo path patches are still in Cargo.toml" >&2
  exit 1
fi

apofasi_rev="$(
  python3 - <<'PY'
import re

text = open("core/Cargo.toml", encoding="utf-8").read()
match = re.search(r'a3s-apofasi = \{[^}]*rev = "([0-9a-f]+)"', text)
if match is None:
    raise SystemExit("a3s-apofasi rev is missing from core/Cargo.toml")
print(match.group(1))
PY
)"

sandbox_rev="$(
  python3 - <<'PY'
import re

text = open("core/Cargo.toml", encoding="utf-8").read()
match = re.search(r'a3s-sandbox = \{[^}]*rev = "([0-9a-f]+)"', text)
if match is None:
    raise SystemExit("a3s-sandbox rev is missing from core/Cargo.toml")
print(match.group(1))
PY
)"

vec_version="$(
  python3 - <<'PY'
import re

text = open("core/Cargo.toml", encoding="utf-8").read()
match = re.search(r'a3s-vec = \{ version = "=([0-9.]+)"', text)
if match is None:
    raise SystemExit("a3s-vec version is missing from core/Cargo.toml")
print(match.group(1))
PY
)"

# Path packages disappear from the lock when the workspace patches are removed.
# Resolve once with the features that activate all three crates, then pin.
cargo metadata --format-version 1 --features apofasi,a3s-vec-fts >/dev/null
cargo update -p a3s-apofasi --precise "$apofasi_rev"
cargo update -p a3s-sandbox --precise "$sandbox_rev"
cargo update -p a3s-vec --precise "$vec_version"

python3 - <<PY
import re
import sys

lock = open("Cargo.lock", encoding="utf-8").read()

def source(name):
    match = re.search(
        rf'\[\[package\]\]\nname = "{name}"\nversion = "[^"]+"\n(?:source = "([^"]+)"\n)?',
        lock,
    )
    if match is None:
        return None
    return match.group(1)

checks = {
    "a3s-apofasi": "git+https://github.com/A3S-Lab/Apofasi.git?rev=$apofasi_rev",
    "a3s-sandbox": "git+https://github.com/A3S-Lab/Sandbox.git?rev=$sandbox_rev",
    "a3s-vec": "registry+https://github.com/rust-lang/crates.io-index",
}
failed = False
for name, expected_prefix in checks.items():
    got = source(name)
    if got is None or not got.startswith(expected_prefix):
        print(
            "relock-standalone-deps: {0} source={1!r} expected prefix {2!r}".format(
                name, got, expected_prefix
            ),
            file=sys.stderr,
        )
        failed = True
if failed:
    raise SystemExit(1)
print("standalone lock sources verified")
PY
