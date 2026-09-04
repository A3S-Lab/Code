#!/usr/bin/env bash
# Download and stage the pinned zvec C API library used by the optional
# zvec-rust lexical backend.
#
# Usage:
#   package_zvec.sh <rust-target> <output-directory> [--allow-unsupported]
#
# The script is intentionally separate from the Rust build script. Release
# jobs stage a verified library first, then compile with ZVEC_LIB_DIR pointing
# at that directory. This keeps a build from silently fetching native code.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="${A3S_CODE_ZVEC_MANIFEST:-${SCRIPT_DIR}/../core/resources/zvec-runtime-manifest.json}"

usage() {
  echo "usage: $0 <rust-target> <output-directory> [--allow-unsupported]" >&2
  exit 64
}

[[ $# -ge 2 && $# -le 3 ]] || usage
TARGET="$1"
OUTPUT_DIR="$2"
ALLOW_UNSUPPORTED="${3:-}"
[[ -z "$ALLOW_UNSUPPORTED" || "$ALLOW_UNSUPPORTED" == "--allow-unsupported" ]] || usage
[[ -f "$MANIFEST" ]] || { echo "zvec manifest not found: $MANIFEST" >&2; exit 1; }

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
asset = manifest.get("assets", {}).get(target)
if expression == ".version":
    value = manifest.get("version")
elif expression == ".assets[$target].archive":
    value = asset and asset.get("archive")
elif expression == ".assets[$target].format":
    value = asset and asset.get("format")
elif expression == ".assets[$target].library":
    value = asset and asset.get("library")
elif expression == ".assets[$target].import_library":
    value = asset and asset.get("import_library")
elif expression == ".assets[$target].sha256":
    value = asset and asset.get("sha256")
else:
    raise SystemExit(f"unsupported manifest expression: {expression}")
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
LIBRARY=""
IMPORT_LIBRARY=""
EXPECTED_SHA256=""
if ARCHIVE="$(json_value '.assets[$target].archive' 2>/dev/null)"; then
  FORMAT="$(json_value '.assets[$target].format')"
  LIBRARY="$(json_value '.assets[$target].library')"
  IMPORT_LIBRARY="$(json_value '.assets[$target].import_library' 2>/dev/null || true)"
  [[ "$IMPORT_LIBRARY" == "null" ]] && IMPORT_LIBRARY=""
  EXPECTED_SHA256="$(json_value '.assets[$target].sha256')"
else
  if [[ "$ALLOW_UNSUPPORTED" != "--allow-unsupported" ]]; then
    echo "zvec-rust v${VERSION} has no prebuilt asset for ${TARGET}" >&2
    echo "Provide a reviewed ZVEC_LIB_DIR or add a verified upstream asset before publishing this target." >&2
    exit 2
  fi
  mkdir -p "$OUTPUT_DIR"
  printf 'target=%s\nversion=%s\nreason=no-upstream-prebuilt-asset\n' "$TARGET" "$VERSION" \
    > "$OUTPUT_DIR/ZVEC_UNAVAILABLE"
  exit 0
fi

case "$EXPECTED_SHA256" in
  ''|*[!0123456789abcdefABCDEF]*)
    echo "Invalid zvec SHA-256 in manifest for ${TARGET}" >&2
    exit 1
    ;;
esac
[[ ${#EXPECTED_SHA256} -eq 64 ]] || { echo "zvec digest must be 64 hex characters" >&2; exit 1; }
[[ "$FORMAT" == "tar.gz" ]] || { echo "Unsupported zvec archive format ${FORMAT}" >&2; exit 1; }
[[ "$LIBRARY" != */* && "$LIBRARY" != *\\* && "$LIBRARY" != *..* ]] || {
  echo "Unsafe zvec library name ${LIBRARY}" >&2
  exit 1
}
if [[ -n "$IMPORT_LIBRARY" && ( "$IMPORT_LIBRARY" == */* || "$IMPORT_LIBRARY" == *\\* || "$IMPORT_LIBRARY" == *..* ) ]]; then
  echo "Unsafe zvec import-library name ${IMPORT_LIBRARY}" >&2
  exit 1
fi

BASE_URL="${A3S_CODE_ZVEC_RELEASE_BASE_URL:-https://github.com/zvec-ai/zvec-rust/releases/download/v${VERSION}}"
case "$BASE_URL" in
  https://*) ;;
  *) echo "A3S_CODE_ZVEC_RELEASE_BASE_URL must use https" >&2; exit 1 ;;
esac

mkdir -p "$OUTPUT_DIR"
# Remove only files owned by this staging contract. This handles a directory
# that was previously populated by the explicit unsupported-target marker.
rm -f -- "$OUTPUT_DIR/ZVEC_UNAVAILABLE" "$OUTPUT_DIR/zvec-runtime.json" \
  "$OUTPUT_DIR/$LIBRARY"
if [[ -n "$IMPORT_LIBRARY" ]]; then
  rm -f -- "$OUTPUT_DIR/$IMPORT_LIBRARY"
fi
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/a3s-code-zvec-package.XXXXXX")"
trap 'rm -rf -- "$TEMP_ROOT"' EXIT
ARCHIVE_PATH="$TEMP_ROOT/$ARCHIVE"
EXTRACT_DIR="$TEMP_ROOT/extract"
mkdir -p "$EXTRACT_DIR"

validate_member_name() {
  local member="$1"
  case "$member" in
    ''|/*|\\*|*\\*|*:*|../*|*/../*|*/..|..)
      echo "Unsafe zvec archive member path: $member" >&2
      return 1
      ;;
  esac
}

curl --fail --location --retry 5 --retry-delay 2 --tlsv1.2 \
  "${BASE_URL%/}/${ARCHIVE}" --output "$ARCHIVE_PATH"

actual_sha256=""
if command -v sha256sum >/dev/null 2>&1; then
  actual_sha256="$(sha256sum "$ARCHIVE_PATH" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual_sha256="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print $1}')"
else
  echo "sha256sum or shasum is required to verify zvec" >&2
  exit 1
fi
expected_lower="$(printf '%s' "$EXPECTED_SHA256" | tr '[:upper:]' '[:lower:]')"
[[ "$actual_sha256" == "$expected_lower" ]] || {
  echo "zvec archive SHA-256 mismatch for ${TARGET}: expected ${expected_lower}, got ${actual_sha256}" >&2
  exit 1
}

TAR_MEMBERS="$(tar -tzf "$ARCHIVE_PATH")"
while IFS= read -r member; do
  validate_member_name "$member"
done <<< "$TAR_MEMBERS"
if ! tar -tvzf "$ARCHIVE_PATH" | awk '$1 ~ /^[lh]/ { found=1 } END { exit found }'; then
  echo "zvec archive contains a hardlink or symlink" >&2
  exit 1
fi

tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR"

ARCHIVE_TARGET_FILE="$EXTRACT_DIR/TARGET"
if [[ -f "$ARCHIVE_TARGET_FILE" ]]; then
  ARCHIVE_TARGET="$(tr -d '\r\n' < "$ARCHIVE_TARGET_FILE")"
  [[ "$ARCHIVE_TARGET" == "$TARGET" ]] || {
    echo "zvec archive target mismatch: expected ${TARGET}, got ${ARCHIVE_TARGET}" >&2
    exit 1
  }
else
  echo "Verified zvec archive is missing its TARGET provenance file" >&2
  exit 1
fi

LIBRARIES=()
while IFS= read -r library_path; do
  [[ -n "$library_path" ]] && LIBRARIES+=("$library_path")
done < <(find "$EXTRACT_DIR" -type f -name "$LIBRARY" -print)
[[ ${#LIBRARIES[@]} -eq 1 ]] || {
  echo "Expected exactly one ${LIBRARY} in the verified zvec archive; found ${#LIBRARIES[@]}" >&2
  exit 1
}
cp "${LIBRARIES[0]}" "$OUTPUT_DIR/$LIBRARY"

if [[ -n "$IMPORT_LIBRARY" ]]; then
  IMPORTS=()
  while IFS= read -r import_path; do
    [[ -n "$import_path" ]] && IMPORTS+=("$import_path")
  done < <(find "$EXTRACT_DIR" -type f -name "$IMPORT_LIBRARY" -print)
  [[ ${#IMPORTS[@]} -eq 1 ]] || {
    echo "Expected exactly one ${IMPORT_LIBRARY} in the verified zvec archive; found ${#IMPORTS[@]}" >&2
    exit 1
  }
  cp "${IMPORTS[0]}" "$OUTPUT_DIR/$IMPORT_LIBRARY"
fi

if command -v jq >/dev/null 2>&1; then
  jq -n \
    --arg schema "a3s-code/zvec-runtime-package/v1" \
    --arg repository "https://github.com/zvec-ai/zvec-rust" \
    --arg license "Apache-2.0" --arg version "$VERSION" --arg target "$TARGET" \
    --arg archive "$ARCHIVE" --arg sha256 "$expected_lower" --arg library "$LIBRARY" \
    --arg import_library "$IMPORT_LIBRARY" \
    '{schema:$schema,repository:$repository,license:$license,version:$version,target:$target,archive:$archive,archive_sha256:$sha256,library:$library} + (if $import_library == "" then {} else {import_library:$import_library} end)' \
    > "$OUTPUT_DIR/zvec-runtime.json"
else
  printf '{"schema":"a3s-code/zvec-runtime-package/v1","repository":"https://github.com/zvec-ai/zvec-rust","license":"Apache-2.0","version":"%s","target":"%s","archive":"%s","archive_sha256":"%s","library":"%s"%s}\n' \
    "$VERSION" "$TARGET" "$ARCHIVE" "$expected_lower" "$LIBRARY" \
    "${IMPORT_LIBRARY:+,\"import_library\":\"$IMPORT_LIBRARY\"}" > "$OUTPUT_DIR/zvec-runtime.json"
fi

echo "Staged zvec-rust v${VERSION} for ${TARGET} in ${OUTPUT_DIR}"
