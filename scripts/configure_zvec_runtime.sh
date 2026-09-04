#!/usr/bin/env bash
# Export the verified zvec runtime directory for CI build and test steps.
#
# The package script intentionally leaves the native library in a staging
# directory.  This helper bridges that staging layout to each platform's
# loader without changing the relocatable rpath used by release artifacts.
#
# Usage:
#   configure_zvec_runtime.sh <rust-target> <runtime-directory>

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <rust-target> <runtime-directory>" >&2
  exit 64
fi

TARGET="$1"
RUNTIME_DIR="$2"
[[ -d "$RUNTIME_DIR" ]] || {
  echo "zvec runtime directory does not exist: $RUNTIME_DIR" >&2
  exit 1
}
[[ -n "${GITHUB_ENV:-}" && -n "${GITHUB_PATH:-}" ]] || {
  echo "GITHUB_ENV and GITHUB_PATH are required" >&2
  exit 1
}

case "$TARGET" in
  *-pc-windows-msvc)
    # GitHub's Windows bash exposes drive paths as /c/...; Cargo and the
    # MSVC linker require a native Windows path for ZVEC_LIB_DIR.  Adding the
    # same native directory to PATH also lets test binaries discover the DLL.
    command -v cygpath >/dev/null 2>&1 || {
      echo "cygpath is required to configure a Windows zvec runtime" >&2
      exit 1
    }
    NATIVE_DIR="$(cygpath -aw "$RUNTIME_DIR")"
    printf 'ZVEC_LIB_DIR=%s\n' "$NATIVE_DIR" >> "$GITHUB_ENV"
    printf '%s\n' "$NATIVE_DIR" >> "$GITHUB_PATH"
    ;;
  *-apple-darwin)
    printf 'ZVEC_LIB_DIR=%s\n' "$RUNTIME_DIR" >> "$GITHUB_ENV"
    printf '%s\n' "$RUNTIME_DIR" >> "$GITHUB_PATH"
    printf 'DYLD_LIBRARY_PATH=%s%s\n' "$RUNTIME_DIR" \
      "${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" >> "$GITHUB_ENV"
    ;;
  *-unknown-linux-gnu|*-unknown-linux-musl)
    printf 'ZVEC_LIB_DIR=%s\n' "$RUNTIME_DIR" >> "$GITHUB_ENV"
    printf '%s\n' "$RUNTIME_DIR" >> "$GITHUB_PATH"
    printf 'LD_LIBRARY_PATH=%s%s\n' "$RUNTIME_DIR" \
      "${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" >> "$GITHUB_ENV"
    ;;
  *)
    echo "unsupported Rust target for zvec runtime: $TARGET" >&2
    exit 2
    ;;
esac
