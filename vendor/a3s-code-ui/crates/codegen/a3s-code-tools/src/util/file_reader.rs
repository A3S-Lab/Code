//! Raw file acquisition shared by local backends and direct readers.

use std::{io, path::Path};

use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Clone, Copy, Debug)]
pub enum FileReadMode {
    Whole,
    BoundedComplete { max_bytes: usize },
    Range { offset: u64, length: u64 },
}

#[derive(Clone, Copy, Debug)]
pub struct FileReadOptions {
    pub mode: FileReadMode,
    pub require_regular_file: bool,
}

impl Default for FileReadOptions {
    fn default() -> Self {
        FileReadOptions {
            mode: FileReadMode::Whole,
            require_regular_file: false,
        }
    }
}

/// Acquire complete bytes or a byte range without decoding or applying policy.
///
/// # Errors
/// Returns I/O errors, `InvalidInput` for nonregular sources or overflowing limits,
/// and `FileTooLarge` when a bounded complete read exceeds its limit.
pub async fn read_file(path: &Path, options: FileReadOptions) -> io::Result<Vec<u8>> {
    if options.require_regular_file && !tokio::fs::metadata(path).await?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source is not a regular file",
        ));
    }
    if matches!(options.mode, FileReadMode::Whole) && !options.require_regular_file {
        return tokio::fs::read(path).await;
    }
    let limit = match options.mode {
        FileReadMode::BoundedComplete { max_bytes } => {
            let probe = max_bytes.checked_add(1).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "file read limit overflow")
            })?;
            Some(u64::try_from(probe).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "file read limit overflow")
            })?)
        }
        FileReadMode::Range { length, .. } => Some(length),
        FileReadMode::Whole => None,
    };
    let mut file = tokio::fs::File::open(path).await?;
    if options.require_regular_file && !file.metadata().await?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source is not a regular file",
        ));
    }
    if let FileReadMode::Range { offset, .. } = options.mode
        && offset > 0
    {
        file.seek(io::SeekFrom::Start(offset)).await?;
    }
    let mut bytes = Vec::new();
    match limit {
        Some(limit) => file.take(limit).read_to_end(&mut bytes).await?,
        None => file.read_to_end(&mut bytes).await?,
    };
    if let FileReadMode::BoundedComplete { max_bytes } = options.mode
        && bytes.len() > max_bytes
    {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "file exceeds byte limit",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "file_reader_tests.rs"]
mod tests;
