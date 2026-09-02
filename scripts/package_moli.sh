#!/usr/bin/env bash
# Download and stage the pinned Moli runtime for a release artifact.
#
# Usage:
#   package_moli.sh <rust-target> <output-directory> [--allow-unsupported]
#
# The normal path is deliberately strict: a supported target must produce an
# executable and a provenance record, otherwise the release job fails.  The
# optional flag is reserved for existing musl package lanes, for which Moli
# v1.1.1 publishes no musl binary; those packages receive an explicit marker
# and the Code runtime can report the limitation instead of pretending that a
# glibc executable is compatible.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="${A3S_CODE_MOLI_MANIFEST:-${SCRIPT_DIR}/../core/resources/moli-runtime-manifest.json}"

usage() {
  echo "usage: $0 <rust-target> <output-directory> [--allow-unsupported]" >&2
  exit 64
}

[[ $# -ge 2 && $# -le 3 ]] || usage
TARGET="$1"
OUTPUT_DIR="$2"
ALLOW_UNSUPPORTED="${3:-}"
[[ -z "$ALLOW_UNSUPPORTED" || "$ALLOW_UNSUPPORTED" == "--allow-unsupported" ]] || usage

[[ -f "$MANIFEST" ]] || { echo "Moli manifest not found: $MANIFEST" >&2; exit 1; }

json_value() {
  local expression="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -er --arg target "$TARGET" "$expression" "$MANIFEST"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "$MANIFEST" "$TARGET" "$expression" <<'PY'
import json
import sys

manifest_path, target, expression = sys.argv[1:]
with open(manifest_path, encoding="utf-8") as stream:
    manifest = json.load(stream)
if expression == '.version':
    value = manifest['version']
elif expression == '.assets[$target].archive':
    value = manifest.get('assets', {}).get(target, {}).get('archive')
elif expression == '.assets[$target].format':
    value = manifest.get('assets', {}).get(target, {}).get('format')
elif expression == '.assets[$target].sha256':
    value = manifest.get('assets', {}).get(target, {}).get('sha256')
else:
    raise SystemExit(f'unsupported manifest expression: {expression}')
if value is None:
    raise SystemExit(1)
print(value)
PY
  else
    echo "jq or python3 is required to read $MANIFEST" >&2
    exit 1
  fi
}

VERSION="$(json_value '.version')"
ARCHIVE=""
FORMAT=""
EXPECTED_SHA256=""
if ARCHIVE="$(json_value '.assets[$target].archive' 2>/dev/null)"; then
  FORMAT="$(json_value '.assets[$target].format')"
  EXPECTED_SHA256="$(json_value '.assets[$target].sha256')"
else
  if [[ "$ALLOW_UNSUPPORTED" != "--allow-unsupported" ]]; then
    echo "Moli v${VERSION} has no prebuilt asset for ${TARGET}" >&2
    echo "Use an explicit browser backend or add a verified upstream asset before publishing this target." >&2
    exit 2
  fi
  mkdir -p "$OUTPUT_DIR"
  printf 'target=%s\nversion=%s\nreason=no-upstream-prebuilt-asset\n' "$TARGET" "$VERSION" \
    > "$OUTPUT_DIR/MOLI_UNAVAILABLE"
  exit 0
fi

case "$EXPECTED_SHA256" in
  ''|*[!0123456789abcdefABCDEF]*)
    echo "Invalid Moli SHA-256 in manifest for ${TARGET}" >&2
    exit 1
    ;;
esac
[[ ${#EXPECTED_SHA256} -eq 64 ]] || { echo "Moli digest must be 64 hex characters" >&2; exit 1; }

case "$FORMAT" in
  tar.gz) ;;
  zip) ;;
  *) echo "Unsupported Moli archive format ${FORMAT}" >&2; exit 1 ;;
esac

BASE_URL="${A3S_CODE_MOLI_RELEASE_BASE_URL:-https://github.com/lexmount/moli/releases/download/v${VERSION}}"
case "$BASE_URL" in
  https://*) ;;
  *) echo "A3S_CODE_MOLI_RELEASE_BASE_URL must use https" >&2; exit 1 ;;
esac

mkdir -p "$OUTPUT_DIR"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/a3s-code-moli-package.XXXXXX")"
trap 'rm -rf -- "$TEMP_ROOT"' EXIT
ARCHIVE_PATH="$TEMP_ROOT/$ARCHIVE"
EXTRACT_DIR="$TEMP_ROOT/extract"
mkdir -p "$EXTRACT_DIR"

validate_member_name() {
  local member="$1"
  # Archives are untrusted input even after digest verification: a future
  # manifest entry may point at a compromised mirror or an accidentally
  # malformed release. Never allow an absolute path, parent traversal, or a
  # Windows separator to escape the staging directory.
  case "$member" in
    ''|/*|\\*|*\\*|../*|*/../*|*/..|..)
      echo "Unsafe Moli archive member path: $member" >&2
      return 1
      ;;
  esac
}

validate_archive_members() {
  local member
  if [[ "$FORMAT" == "tar.gz" ]]; then
    local tar_members
    tar_members="$(tar -tzf "$ARCHIVE_PATH")" || return 1
    while IFS= read -r member; do
      validate_member_name "$member" || return 1
    done <<< "$tar_members"
    # Do not materialize links from an archive into the package. The runtime
    # resolver requires a regular executable and the release should contain a
    # single auditable binary, not a link to an arbitrary archive member.
    if tar -tvzf "$ARCHIVE_PATH" | awk '$1 ~ /^[lh]/ { exit 1 }'; then
      :
    else
      echo "Moli archive contains a hardlink or symlink" >&2
      return 1
    fi
  else
    local zip_members
    zip_members="$(unzip -Z1 "$ARCHIVE_PATH")" || return 1
    while IFS= read -r member; do
      validate_member_name "$member" || return 1
    done <<< "$zip_members"
  fi
}

curl --fail --location --retry 5 --retry-delay 2 --tlsv1.2 \
  "${BASE_URL%/}/${ARCHIVE}" --output "$ARCHIVE_PATH"

actual_sha256=""
if command -v sha256sum >/dev/null 2>&1; then
  actual_sha256="$(sha256sum "$ARCHIVE_PATH" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual_sha256="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print $1}')"
else
  echo "sha256sum or shasum is required to verify Moli" >&2
  exit 1
fi
expected_lower="$(printf '%s' "$EXPECTED_SHA256" | tr '[:upper:]' '[:lower:]')"
[[ "$actual_sha256" == "$expected_lower" ]] || {
  echo "Moli archive SHA-256 mismatch for ${TARGET}: expected ${expected_lower}, got ${actual_sha256}" >&2
  exit 1
}

validate_archive_members

if [[ "$FORMAT" == "tar.gz" ]]; then
  tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR"
else
  if command -v unzip >/dev/null 2>&1; then
    unzip -q "$ARCHIVE_PATH" -d "$EXTRACT_DIR"
  elif command -v 7z >/dev/null 2>&1; then
    7z x -y "-o${EXTRACT_DIR}" "$ARCHIVE_PATH" >/dev/null
  else
    echo "unzip or 7z is required to extract the Moli Windows archive" >&2
    exit 1
  fi
fi

EXECUTABLE_NAME="moli"
if [[ "$TARGET" == *windows* ]]; then
  EXECUTABLE_NAME="moli.exe"
fi
BINARIES=()
while IFS= read -r binary_path; do
  [[ -n "$binary_path" ]] && BINARIES+=("$binary_path")
done < <(find "$EXTRACT_DIR" -type f -name "$EXECUTABLE_NAME" -print)
[[ ${#BINARIES[@]} -eq 1 ]] || {
  echo "Expected exactly one ${EXECUTABLE_NAME} in the verified Moli archive; found ${#BINARIES[@]}" >&2
  exit 1
}

DEST="$OUTPUT_DIR/$EXECUTABLE_NAME"
rm -f -- "$DEST"
cp "${BINARIES[0]}" "$DEST"
if [[ "$EXECUTABLE_NAME" == "moli" ]]; then
  chmod 0755 "$DEST"
fi

# Preserve the upstream notices beside the executable. This makes every
# self-contained package auditable without reaching back to the network.
ROOT="$(dirname -- "${BINARIES[0]}")"
for notice in LICENSE-APACHE LICENSE-MIT README.md RELEASING.md VERSION license-metadata.json; do
  [[ -f "$ROOT/$notice" ]] && cp "$ROOT/$notice" "$OUTPUT_DIR/$notice"
done
if [[ -d "$ROOT/licenses" ]]; then
  mkdir -p "$OUTPUT_DIR/licenses"
  cp -R "$ROOT/licenses/." "$OUTPUT_DIR/licenses/"
fi

if command -v jq >/dev/null 2>&1; then
  jq -n --arg schema "a3s-code/moli-runtime-package/v1" \
    --arg repository "https://github.com/lexmount/moli" \
    --arg version "$VERSION" --arg target "$TARGET" --arg archive "$ARCHIVE" \
    --arg sha256 "$expected_lower" \
    '{schema:$schema,repository:$repository,version:$version,target:$target,archive:$archive,archive_sha256:$sha256}' \
    > "$OUTPUT_DIR/moli-runtime.json"
else
  printf '{"schema":"a3s-code/moli-runtime-package/v1","repository":"https://github.com/lexmount/moli","version":"%s","target":"%s","archive":"%s","archive_sha256":"%s"}\n' \
    "$VERSION" "$TARGET" "$ARCHIVE" "$expected_lower" > "$OUTPUT_DIR/moli-runtime.json"
fi

echo "Staged Moli v${VERSION} for ${TARGET} in ${OUTPUT_DIR}"
