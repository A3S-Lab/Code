use super::vec_shadow_store::{VecShadowFailure, VecShadowResult};
use super::vector_contract::VectorRecord;
use a3s_vec::Doc;
use sha2::{Digest, Sha256};

pub(super) const RECORD_ID_FIELD: &str = "record_id";
pub(super) const PARTITION_FIELD: &str = "partition";
pub(super) const PARTITION_KEY_FIELD: &str = "partition_key";
pub(super) const EMBEDDING_FIELD: &str = "embedding";
const MAX_FILTER_BYTES: usize = 64 * 1024;

pub(super) fn prepare_documents(
    partition: &str,
    records: Vec<VectorRecord>,
    dimension: usize,
) -> VecShadowResult<(Vec<String>, Vec<Doc>)> {
    let partition_key = partition_key(partition);
    let mut keys = Vec::with_capacity(records.len());
    let mut docs = Vec::with_capacity(records.len());
    for mut record in records {
        if !record.labels.is_empty() {
            return Err(VecShadowFailure::UnsupportedLabels);
        }
        normalize_unit(&mut record.embedding, dimension)?;
        let key = tie_key(partition, &record.id);
        let mut doc = Doc::with_pk(&key)?;
        doc.add_string(RECORD_ID_FIELD, &record.id)?;
        doc.add_string(PARTITION_FIELD, partition)?;
        doc.add_string(PARTITION_KEY_FIELD, &partition_key)?;
        doc.add_vector_f32(EMBEDDING_FIELD, &record.embedding)?;
        keys.push(key);
        docs.push(doc);
    }
    Ok((keys, docs))
}

pub(super) fn partition_filter<'a>(
    partitions: impl Iterator<Item = &'a str>,
) -> VecShadowResult<String> {
    let keys = partitions.map(partition_key).collect::<Vec<_>>();
    let filter = if keys.is_empty() {
        format!("{PARTITION_KEY_FIELD} == 'no-selected-partition'")
    } else {
        format!(
            "{PARTITION_KEY_FIELD} in [{}]",
            keys.iter()
                .map(|key| format!("'{key}'"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    if filter.len() > MAX_FILTER_BYTES {
        Err(VecShadowFailure::FilterBudget)
    } else {
        Ok(filter)
    }
}

fn partition_key(partition: &str) -> String {
    format!("{:x}", Sha256::digest(partition.as_bytes()))
}

fn tie_key(partition: &str, id: &str) -> String {
    let mut key = String::new();
    append_hex(&mut key, partition.as_bytes());
    key.push('!');
    append_hex(&mut key, id.as_bytes());
    key
}

fn append_hex(target: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        target.push(char::from(HEX[usize::from(byte >> 4)]));
        target.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}

pub(super) fn normalize_unit(vector: &mut [f32], dimension: usize) -> VecShadowResult<()> {
    if vector.len() != dimension || vector.iter().any(|value| !value.is_finite()) {
        return Err(VecShadowFailure::InvalidContract);
    }
    let norm = vector
        .iter()
        .fold(0.0f64, |sum, value| {
            let value = f64::from(*value);
            sum + value * value
        })
        .sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return Err(VecShadowFailure::InvalidContract);
    }
    for value in vector {
        *value = (f64::from(*value) / norm) as f32;
    }
    Ok(())
}
