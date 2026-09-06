//! Bounded file reads for persistence and context boundaries.

use std::io::{self, Read};
use std::path::Path;

fn read_limit(max_bytes: usize) -> u64 {
    u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}

fn too_large(max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("file exceeds configured limit of {max_bytes} bytes"),
    )
}

/// Read a regular file while preventing a concurrent growth race from
/// bypassing the caller's byte limit.
pub(crate) fn read_file_bounded(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let declared_len = file.metadata()?.len();
    if declared_len > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(too_large(max_bytes));
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(declared_len)
            .unwrap_or(usize::MAX)
            .min(64 * 1024),
    );
    file.take(read_limit(max_bytes)).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(too_large(max_bytes));
    }
    Ok(bytes)
}

/// Async counterpart of [`read_file_bounded`].
pub(crate) async fn read_file_bounded_async(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path).await?;
    let declared_len = file.metadata().await?.len();
    if declared_len > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(too_large(max_bytes));
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(declared_len)
            .unwrap_or(usize::MAX)
            .min(64 * 1024),
    );
    let mut reader = file.take(read_limit(max_bytes));
    reader.read_to_end(&mut bytes).await?;
    if bytes.len() > max_bytes {
        return Err(too_large(max_bytes));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_read_rejects_oversized_sparse_file_before_allocation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(5).unwrap();

        let error = read_file_bounded(file.path(), 4).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn async_read_rejects_oversized_sparse_file_before_allocation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(5).unwrap();

        let error = read_file_bounded_async(file.path(), 4).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
