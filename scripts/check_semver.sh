#!/usr/bin/env bash
# Compare the current public API with a verified published baseline.

set -euo pipefail

BASELINE_VERSION="${1:-5.2.8}"
PACKAGE="a3s-code-core"

case "$BASELINE_VERSION" in
  5.2.8)
    BASELINE_SHA256="059e9eefe6f2d0b816b9ec9f906878413a2f30fd1bc90a751c53b77972ff84a7"
    ;;
  5.2.7)
    BASELINE_SHA256="59993ad1e362628c7665d817318271faff5b3f775dfad0d648b1cc4a17099784"
    ;;
  5.2.4)
    BASELINE_SHA256="0066046ead6d44acac8a01a8bd1bd78c37aae02c14121f28ea77c15d9d0133a4"
    ;;
  *)
    echo "unsupported SemVer baseline: $BASELINE_VERSION" >&2
    exit 1
    ;;
esac

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/a3s-code-semver.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT

ARCHIVE="$TEMP_ROOT/${PACKAGE}-${BASELINE_VERSION}.crate"
SOURCE_ROOT="$TEMP_ROOT/${PACKAGE}-${BASELINE_VERSION}"

curl \
  --fail \
  --location \
  --silent \
  --show-error \
  --retry 3 \
  --proto '=https' \
  --tlsv1.2 \
  --output "$ARCHIVE" \
  "https://static.crates.io/crates/${PACKAGE}/${PACKAGE}-${BASELINE_VERSION}.crate"

python3 - "$ARCHIVE" "$BASELINE_SHA256" <<'PY'
import hashlib
import pathlib
import sys

archive = pathlib.Path(sys.argv[1])
expected = sys.argv[2]
actual = hashlib.sha256(archive.read_bytes()).hexdigest()
if actual != expected:
    raise SystemExit(
        f"baseline archive checksum mismatch: expected {expected}, got {actual}"
    )
PY

tar -xzf "$ARCHIVE" -C "$TEMP_ROOT"

# The published 5.2.4 manifest used the compatible range `1.4.1`, but a later
# incompatible a3s-search release now satisfies that range. Pin the dependency
# version used by 5.2.4 so its unchanged public API can still be documented.
if [[ "$BASELINE_VERSION" == "5.2.4" ]]; then
  python3 - "$SOURCE_ROOT/Cargo.toml" <<'PY'
import pathlib
import sys

manifest = pathlib.Path(sys.argv[1])
contents = manifest.read_text()
original = '[dependencies.a3s-search]\nversion = "1.4.1"'
replacement = '[dependencies.a3s-search]\nversion = "=1.4.1"'
if contents.count(original) != 1:
    raise SystemExit(f"unexpected baseline a3s-search declaration in {manifest}")
manifest.write_text(contents.replace(original, replacement))
PY
fi

cargo semver-checks check-release \
  --package "$PACKAGE" \
  --baseline-root "$SOURCE_ROOT"
