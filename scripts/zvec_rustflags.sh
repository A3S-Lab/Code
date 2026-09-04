#!/usr/bin/env bash
# Print the linker flag that makes a packaged zvec-rust sidecar discoverable.
#
# Usage:
#   zvec_rustflags.sh <rust-target> [relative-runtime-directory]
#
# The value is intended for RUSTFLAGS.  The relative directory is resolved
# from the final loadable (Node addon, Python extension, or Go executable),
# not from Cargo's target directory.  Windows DLL lookup already searches the
# executable/module directory, so no extra linker flag is required there.

set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 <rust-target> [relative-runtime-directory]" >&2
  exit 64
fi

TARGET="$1"
RUNTIME_DIR="${2:-zvec}"

case "$RUNTIME_DIR" in
  ''|/*|*..*|*\\*)
    echo "relative runtime directory is unsafe: $RUNTIME_DIR" >&2
    exit 1
    ;;
esac

case "$TARGET" in
  *-apple-darwin)
    printf '%s\n' "-C link-arg=-Wl,-rpath,@loader_path/$RUNTIME_DIR"
    ;;
  *-unknown-linux-gnu|*-unknown-linux-musl)
    # Keep the dollar sign literal. Cargo passes it to the ELF linker, which
    # stores $ORIGIN in DT_RUNPATH for resolution beside the final artifact.
    printf '%s\n' "-C link-arg=-Wl,-rpath,\$ORIGIN/$RUNTIME_DIR"
    ;;
  *-pc-windows-msvc)
    # LoadLibrary searches the module directory for a same-named DLL.
    :
    ;;
  *)
    echo "unsupported Rust target for zvec sidecar flags: $TARGET" >&2
    exit 2
    ;;
esac
