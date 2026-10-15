//! Scope each member of the built-in read tool before aggregating permission.

use super::{canonical_tool_name, PermissionDecision};
use crate::tools::builtin::{MAX_BATCH_READ_FILES, MAX_BATCH_READ_PATH_BYTES};
use serde_json::{json, Value};

pub(super) fn paths<'a>(
    tool_name: &str,
    args: &'a Value,
) -> Result<Option<Vec<&'a str>>, PermissionDecision> {
    if !canonical_tool_name(tool_name).eq_ignore_ascii_case("read") {
        return Ok(None);
    }
    let Some(files) = args.get("files") else {
        return Ok(None);
    };
    if args.get("file_path").is_some() {
        return Err(PermissionDecision::Deny);
    }
    let files = files
        .as_array()
        .filter(|files| !files.is_empty() && files.len() <= MAX_BATCH_READ_FILES)
        .ok_or(PermissionDecision::Deny)?;
    let mut paths = Vec::with_capacity(files.len());
    for entry in files {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty() && path.len() <= MAX_BATCH_READ_PATH_BYTES)
            .ok_or(PermissionDecision::Deny)?;
        paths.push(path);
    }
    Ok(Some(paths))
}

pub(super) fn check(
    tool_name: &str,
    args: &Value,
    mut check_member: impl FnMut(&Value) -> PermissionDecision,
) -> Option<PermissionDecision> {
    let paths = match paths(tool_name, args) {
        Ok(None) => return None,
        Ok(Some(paths)) => paths,
        Err(denial) => return Some(denial),
    };
    let mut decision = PermissionDecision::Allow;
    for path in paths {
        match check_member(&json!({"file_path": path})) {
            PermissionDecision::Deny => return Some(PermissionDecision::Deny),
            PermissionDecision::Ask => decision = PermissionDecision::Ask,
            PermissionDecision::Allow => {}
        }
    }
    Some(decision)
}
