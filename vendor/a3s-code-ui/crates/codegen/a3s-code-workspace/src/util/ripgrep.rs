// Resolution (bundled binary, RG_BIN_PATH, Bazel runfiles, PATH) lives in the a3s-tools crate
// This module only preserves the `crate::util::ripgrep` path
pub use a3s_code_tools::util::rg_path;
