//! Acknowledge Cursor's per-request blob store without touching the filesystem.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;
const MAX_BLOBS: usize = 256;

#[derive(Default)]
pub(super) struct Store {
    blobs: HashMap<String, String>,
}

impl Store {
    pub(super) fn reply(&mut self, payload: &[u8]) -> Result<Option<Vec<u8>>> {
        let Ok(value) = serde_json::from_slice::<Value>(payload) else {
            return Ok(None);
        };
        let Some(request) = value.get("kvServerMessage") else {
            return Ok(None);
        };
        let invalid = || anyhow!("Cursor sent an invalid or oversized blob request");
        // Protobuf JSON omits default-valued scalar fields, including id 0.
        let id = match request.get("id") {
            None => 0,
            Some(id) => id
                .as_u64()
                .filter(|id| *id <= u32::MAX as u64)
                .ok_or_else(invalid)?,
        };
        let result = match (request.get("setBlobArgs"), request.get("getBlobArgs")) {
            (Some(args), None) => {
                let key = args
                    .get("blobId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                let data = args
                    .get("blobData")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                let total = self
                    .blobs
                    .iter()
                    .filter(|(stored, _)| stored.as_str() != key)
                    .map(|(key, data)| key.len() + data.len())
                    .sum::<usize>();
                if key.len().saturating_add(data.len()).saturating_add(total) > MAX_BLOB_BYTES
                    || !self.blobs.contains_key(key) && self.blobs.len() >= MAX_BLOBS
                {
                    return Err(invalid());
                }
                self.blobs.insert(key.to_string(), data.to_string());
                json!({"id":id,"setBlobResult":{}})
            }
            (None, Some(args)) => {
                let key = args
                    .get("blobId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                let data = self
                    .blobs
                    .get(key)
                    .ok_or_else(|| anyhow!("Cursor requested an unknown conversation blob"))?;
                json!({"id":id,"getBlobResult":{"blobData":data}})
            }
            _ => return Err(invalid()),
        };
        Ok(Some(serde_json::to_vec(
            &json!({"kvClientMessage":result}),
        )?))
    }
}

#[cfg(test)]
mod tests;
