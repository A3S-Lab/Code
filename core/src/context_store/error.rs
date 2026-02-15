//! Error types for context store

use thiserror::Error;

pub type Result<T> = std::result::Result<T, A3SError>;

#[derive(Debug, Error)]
pub enum A3SError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Embedding error: {0}")]
    Embedding(String),
    #[error("Ingest error: {0}")]
    Ingest(String),
    #[error("Retrieval error: {0}")]
    Retrieval(String),
    #[error("Digest generation error: {0}")]
    DigestGeneration(String),
    #[error("Pathway error: {0}")]
    Pathway(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let e = A3SError::Storage("disk full".to_string());
        assert_eq!(e.to_string(), "Storage error: disk full");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let e: A3SError = io_err.into();
        assert!(e.to_string().contains("not found"));
    }

    #[test]
    fn test_all_error_variants() {
        let errors = vec![
            A3SError::Storage("s".into()),
            A3SError::Embedding("e".into()),
            A3SError::Ingest("i".into()),
            A3SError::Retrieval("r".into()),
            A3SError::DigestGeneration("d".into()),
            A3SError::Pathway("p".into()),
        ];
        for e in errors {
            assert!(!e.to_string().is_empty());
        }
    }
}
