//! Supply no Cursor-native host context; A3S owns context and tool execution.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

pub(super) fn empty() -> Value {
    json!({
        "rulesInfoComplete":true, "envInfoComplete":true,
        "repositoryInfoComplete":true, "gitRepoInfoComplete":true,
        "gitStatusInfoComplete":true, "mcpInfoComplete":true,
        "mcpFileSystemInfoComplete":true, "agentSkillsInfoComplete":true,
        "customSubagentsInfoComplete":true,
        "webSearchEnabled":false, "webFetchEnabled":false
    })
}

pub(super) fn reply(payload: &[u8]) -> Result<Option<Vec<u8>>> {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return Ok(None);
    };
    let Some(request) = value.get("execServerMessage").and_then(Value::as_object) else {
        return Ok(None);
    };
    if !request.contains_key("requestContextArgs") {
        return Ok(None);
    }
    let invalid = || anyhow!("Cursor sent an invalid context request");
    if !request["requestContextArgs"].is_object()
        || request.keys().any(|key| {
            !matches!(
                key.as_str(),
                "id" | "execId"
                    | "machineId"
                    | "spanContext"
                    | "requestContextArgs"
                    | "acceptHookAdditionalContexts"
            )
        })
    {
        return Err(invalid());
    }
    let id = match request.get("id") {
        None => 0,
        Some(value) => value
            .as_u64()
            .filter(|id| *id <= u32::MAX as u64)
            .ok_or_else(invalid)?,
    };
    let exec_id = match request.get("execId") {
        None => "",
        Some(value) => value.as_str().ok_or_else(invalid)?,
    };
    if exec_id.len() > 1024 {
        return Err(invalid());
    }
    Ok(Some(serde_json::to_vec(&json!({"execClientMessage":{
        "id":id, "execId":exec_id,
        "requestContextResult":{"success":{"requestContext":empty()}}
    }}))?))
}

#[cfg(test)]
mod tests;
