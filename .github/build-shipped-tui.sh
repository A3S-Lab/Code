#!/bin/bash
# Build the debug a3s-code-tui that acp/tests/shipped_tui_e2e.rs launches.
# The pager path-depends on the sibling ACL crate. CI checks that crate out
# and symlinks it to ../acl before calling this script.
# Two working-tree patches match the CLI release pager build and stay uncommitted:
# find-msvc-tools 0.1.14 for the stable toolchain, and portable protoc output.

set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
acl="$repo_root/../acl"
pager="$repo_root/vendor/a3s-code-ui"

if [[ ! -f "$acl/Cargo.toml" ]]; then
  echo "shipped TUI needs the sibling ACL crate at $acl" >&2
  exit 1
fi
if ! grep -q 'name = "a3s-acl"' "$acl/Cargo.toml"; then
  echo "sibling at $acl is not the a3s-acl crate" >&2
  exit 1
fi

expected="$(cd "$pager/crates/codegen/a3s-code-pager" && cd ../../../../../../acl && pwd -P)"
actual="$(cd "$acl" && pwd -P)"
if [[ "$expected" != "$actual" ]]; then
  echo "pager ACL path resolves to $expected, sibling is $actual" >&2
  exit 1
fi

cd "$pager"

python3 - <<'PY'
from pathlib import Path

lock_path = Path("Cargo.lock")
lock = lock_path.read_text()
old = """name = "find-msvc-tools"
version = "0.1.13"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ef25905e51abafe4dcea6c15fec58c57b601cdbd0ee53d22ea1d3016c587d39b\""""
new = """name = "find-msvc-tools"
version = "0.1.14"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aedcfb3409746eddb02b9e19ebda1c3394f759a152e48ee875a0844d1b955484\""""
if old not in lock:
    raise SystemExit("pager lock is missing find-msvc-tools 0.1.13")
lock_path.write_text(lock.replace(old, new, 1))
PY

python3 - <<'PY'
from pathlib import Path

path = Path("crates/build/a3s-proto-build/src/lib.rs")
source = path.read_text()
old = """            let mut command = Command::new(protoc.unwrap_or(Path::new("protoc")));
            command
                .arg("--dependency_out=/dev/stdout")
                .arg("--descriptor_set_out=/dev/null");"""
new = """            let dep_path = std::env::temp_dir().join("a3s-protoc-deps.d");
            let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
            let mut command = Command::new(protoc.unwrap_or(Path::new("protoc")));
            command
                .arg(format!(
                    "--dependency_out={}",
                    dep_path.to_str().context("dependency path not UTF-8")?
                ))
                .arg(format!("--descriptor_set_out={null_device}"));"""
output_old = """            let output =
                String::from_utf8(output.stdout).context("protoc command output not UTF-8")?;

            let mut lines = output.lines();
            let first_line = lines.next().context("protoc command output is empty")?;
            let prefix = "/dev/null:";
            let rem = first_line.strip_prefix(prefix).with_context(|| {
                format!("protoc command output must start with /dev/null: {output:?}")
            })?;"""
output_new = """            let output = fs::read_to_string(&dep_path)
                .context("protoc dependency file is not UTF-8")?;

            let mut lines = output.lines();
            let first_line = lines.next().context("protoc command output is empty")?;
            let prefix = format!("{null_device}:");
            let rem = first_line.strip_prefix(&prefix).with_context(|| {
                format!("protoc dependency output must start with {prefix} {output:?}")
            })?;"""
if old not in source or output_old not in source:
    raise SystemExit("protoc dependency emitter no longer matches the portable patch")
path.write_text(source.replace(old, new, 1).replace(output_old, output_new, 1))
PY

# The end-to-end tests only launch this binary.
export CARGO_PROFILE_DEV_DEBUG=0
cargo build --locked -p a3s-code-pager-bin --bin a3s-code-tui
test -x target/debug/a3s-code-tui
