use super::*;
use crate::computer::{
    local::{LocalFs, MockFs},
    types::AsyncFileSystem,
};

#[tokio::test]
async fn whole_and_range_preserve_large_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large");
    let bytes = vec![0x80; 8 * 1024 * 1024 + 1];
    tokio::fs::write(&path, &bytes).await.unwrap();
    assert_eq!(bytes, LocalFs.read_file(&path).await.unwrap());
    for (offset, expected) in [
        (bytes.len() as u64 - 2, vec![0x80; 2]),
        (bytes.len() as u64 + 10, vec![]),
    ] {
        assert_eq!(
            expected,
            read_file(
                &path,
                FileReadOptions {
                    mode: FileReadMode::Range { offset, length: 40 },
                    require_regular_file: false,
                }
            )
            .await
            .unwrap()
        );
    }
}

#[tokio::test]
async fn bounded_reads_are_complete_or_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    tokio::fs::write(&path, b"abcd").await.unwrap();
    assert_eq!(
        b"abcd",
        LocalFs
            .read_file_bounded(&path, 4)
            .await
            .unwrap()
            .as_slice()
    );
    for (source, limit, kind) in [
        (path.as_path(), 3, io::ErrorKind::FileTooLarge),
        (dir.path(), 4, io::ErrorKind::InvalidInput),
        (path.as_path(), usize::MAX, io::ErrorKind::InvalidInput),
    ] {
        assert_eq!(
            Some(kind),
            LocalFs
                .read_file_bounded(source, limit)
                .await
                .unwrap_err()
                .io_error_kind()
        );
    }
    let mock = MockFs::new();
    mock.set_file(&path, b"abcd").await;
    assert_eq!(
        b"abcd",
        mock.read_file_bounded(&path, 4).await.unwrap().as_slice()
    );
    assert_eq!(
        Some(io::ErrorKind::FileTooLarge),
        mock.read_file_bounded(&path, 3)
            .await
            .unwrap_err()
            .io_error_kind()
    );
}

#[tokio::test]
async fn regular_requirement_is_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let error = read_file(
        dir.path(),
        FileReadOptions {
            require_regular_file: true,
            ..FileReadOptions::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(io::ErrorKind::InvalidInput, error.kind());
}

#[tokio::test]
async fn unsupported_backend_never_uses_unbounded_read() {
    struct Unsupported;
    #[async_trait::async_trait]
    impl AsyncFileSystem for Unsupported {
        async fn read_file(
            &self,
            _: &Path,
        ) -> Result<Vec<u8>, crate::computer::types::ComputerError> {
            panic!("unbounded fallback");
        }
        async fn write_file(
            &self,
            _: &Path,
            _: &[u8],
        ) -> Result<(), crate::computer::types::ComputerError> {
            Ok(())
        }
        async fn delete_file(&self, _: &Path) -> Result<(), crate::computer::types::ComputerError> {
            Ok(())
        }
    }
    assert!(!Unsupported.supports_bounded_read());
    assert_eq!(
        Some(io::ErrorKind::Unsupported),
        Unsupported
            .read_file_bounded(Path::new("anywhere"), 4)
            .await
            .unwrap_err()
            .io_error_kind()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bounded_read_follows_ordinary_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let link = dir.path().join("link");
    tokio::fs::write(&source, b"abcd").await.unwrap();
    std::os::unix::fs::symlink(source, &link).unwrap();
    assert_eq!(
        b"abcd",
        LocalFs
            .read_file_bounded(&link, 4)
            .await
            .unwrap()
            .as_slice()
    );
}
