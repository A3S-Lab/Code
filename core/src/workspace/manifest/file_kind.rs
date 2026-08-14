//! Conservative text/non-text boundary for workspace indexing.

use crate::language::LanguageCatalog;
use std::io::Read;
use std::path::Path;

const SAMPLE_BYTES: usize = 2 * 1024;

/// Return `true` when a file must stay outside text search and embeddings.
///
/// Known source and text paths avoid an extra probe during manifest scans. A
/// complete UTF-8 read still occurs before chunking, so mislabeled source never
/// reaches the chunker or embedding provider. Known non-text assets are denied
/// by extension even when their container header happens to be ASCII.
pub(super) fn is_binary_file(path: &Path, size: u64) -> bool {
    if is_known_non_text_path(path) {
        return true;
    }
    if is_known_text_path(path) || size == 0 {
        return false;
    }

    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let mut sample = [0u8; SAMPLE_BYTES];
    match file.read(&mut sample) {
        Ok(0) => false,
        Ok(read) => sample_is_non_text(&sample[..read]),
        Err(_) => true,
    }
}

fn sample_is_non_text(sample: &[u8]) -> bool {
    if sample
        .iter()
        .any(|byte| byte.is_ascii_control() && !matches!(*byte, b'\n' | b'\r' | b'\t'))
    {
        return true;
    }

    match std::str::from_utf8(sample) {
        Ok(_) => false,
        // A bounded sample can end in the middle of an otherwise valid UTF-8
        // scalar. `None` means only that trailing scalar is incomplete.
        Err(error) => error.error_len().is_some(),
    }
}

fn is_known_non_text_path(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        // Images and design assets.
        "avif"
            | "bmp"
            | "gif"
            | "heic"
            | "ico"
            | "jpeg"
            | "jpg"
            | "png"
            | "psd"
            | "tif"
            | "tiff"
            | "webp"
            // Office and publication containers.
            | "doc"
            | "docx"
            | "epub"
            | "mobi"
            | "odp"
            | "ods"
            | "odt"
            | "pdf"
            | "ppt"
            | "pptx"
            | "xls"
            | "xlsx"
            // Archives and compressed streams.
            | "7z"
            | "bz2"
            | "gz"
            | "rar"
            | "tar"
            | "tgz"
            | "xz"
            | "zip"
            | "zst"
            // Audio and video.
            | "aac"
            | "avi"
            | "flac"
            | "m4a"
            | "mkv"
            | "mov"
            | "mp3"
            | "mp4"
            | "ogg"
            | "wav"
            | "webm"
            // Databases, columnar data, fonts, models, and native artifacts.
            | "a"
            | "arrow"
            | "avro"
            | "bin"
            | "class"
            | "dat"
            | "db"
            | "dll"
            | "dmp"
            | "duckdb"
            | "dylib"
            | "eot"
            | "exe"
            | "iso"
            | "jar"
            | "lib"
            | "o"
            | "obj"
            | "onnx"
            | "orc"
            | "pak"
            | "parquet"
            | "pt"
            | "pth"
            | "pyc"
            | "pyd"
            | "rlib"
            | "safetensors"
            | "so"
            | "sqlite"
            | "sqlite3"
            | "tflite"
            | "ttf"
            | "wasm"
            | "woff"
            | "woff2"
    )
}

fn is_known_text_path(path: &Path) -> bool {
    if LanguageCatalog::id_for_path(path).is_some() {
        return true;
    }
    if matches!(
        extension(path).as_str(),
        "acl" | "dockerignore" | "env" | "example" | "gitignore" | "lock" | "sample" | "txt"
    ) {
        return true;
    }
    matches!(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "dockerfile" | "makefile" | "justfile" | "license" | "notice"
    )
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}
