use crate::workspace::WorkspacePath;
use percent_encoding::percent_decode_str;
use reqwest::Url;
use std::path::{Component, Path, PathBuf};

const MAX_FILENAME_BYTES: usize = 240;

#[derive(Debug)]
pub(super) struct Destination {
    pub workspace_path: WorkspacePath,
    pub absolute_path: PathBuf,
    pub parent: PathBuf,
    pub existed: bool,
}

pub(super) fn infer_filename(content_disposition: Option<&str>, url: &Url) -> String {
    let candidate = content_disposition
        .and_then(filename_from_content_disposition)
        .or_else(|| filename_from_query(url))
        .or_else(|| filename_from_url_path(url))
        .unwrap_or_else(|| "download.bin".to_string());
    sanitize_filename(&candidate)
}

fn filename_from_content_disposition(header: &str) -> Option<String> {
    let mut plain = None;
    let mut encoded = None;
    for segment in split_header_parameters(header).into_iter().skip(1) {
        let Some((name, value)) = segment.split_once('=') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = unquote_parameter(value.trim());
        match name.as_str() {
            "filename*" => encoded = decode_extended_filename(&value),
            "filename" if plain.is_none() => plain = Some(value),
            _ => {}
        }
    }
    encoded.or(plain).filter(|value| !value.trim().is_empty())
}

fn split_header_parameters(header: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in header.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            current.push(character);
            escaped = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
            current.push(character);
            continue;
        }
        if character == ';' && !quoted {
            segments.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(character);
        }
    }
    segments.push(current.trim().to_string());
    segments
}

fn unquote_parameter(value: &str) -> String {
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    let mut result = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            result.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            result.push(character);
        }
    }
    if escaped {
        result.push('\\');
    }
    result
}

fn decode_extended_filename(value: &str) -> Option<String> {
    let (charset, remainder) = value.split_once('\'')?;
    let (_, encoded) = remainder.split_once('\'')?;
    if !charset.is_empty()
        && !charset.eq_ignore_ascii_case("utf-8")
        && !charset.eq_ignore_ascii_case("us-ascii")
    {
        return None;
    }
    percent_decode_str(encoded)
        .decode_utf8()
        .ok()
        .map(|value| value.into_owned())
}

fn filename_from_query(url: &Url) -> Option<String> {
    url.query_pairs().find_map(|(key, value)| {
        matches!(key.to_ascii_lowercase().as_str(), "filename" | "file")
            .then(|| value.into_owned())
            .filter(|value| !value.trim().is_empty())
    })
}

fn filename_from_url_path(url: &Url) -> Option<String> {
    let segment = url
        .path_segments()?
        .rev()
        .find(|segment| !segment.is_empty())?;
    percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(|value| value.into_owned())
}

pub(super) fn sanitize_filename(candidate: &str) -> String {
    let mut sanitized = String::with_capacity(candidate.len());
    for character in candidate.chars() {
        if character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
        {
            sanitized.push('_');
        } else {
            sanitized.push(character);
        }
    }

    let trimmed = sanitized.trim().trim_end_matches([' ', '.']).trim();
    let mut sanitized = if trimmed.is_empty() || matches!(trimmed, "." | "..") {
        "download.bin".to_string()
    } else {
        trimmed.to_string()
    };

    let base = sanitized
        .split_once('.')
        .map(|(base, _)| base)
        .unwrap_or(&sanitized)
        .to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || base
            .strip_prefix("COM")
            .or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'));
    if reserved {
        sanitized.insert(0, '_');
    }

    truncate_filename(&sanitized, MAX_FILENAME_BYTES)
}

fn truncate_filename(filename: &str, max_bytes: usize) -> String {
    if filename.len() <= max_bytes {
        return filename.to_string();
    }

    let extension_start = filename
        .rfind('.')
        .filter(|index| *index > 0 && filename.len().saturating_sub(*index) < max_bytes / 2);
    let result = if let Some(index) = extension_start {
        let suffix = &filename[index..];
        let stem_budget = max_bytes.saturating_sub(suffix.len());
        format!(
            "{}{}",
            truncate_utf8(&filename[..index], stem_budget),
            suffix
        )
    } else {
        truncate_utf8(filename, max_bytes).to_string()
    };
    let result = result.trim_end_matches([' ', '.']);
    if result.is_empty() {
        "download.bin".to_string()
    } else {
        result.to_string()
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub(super) fn prepare_destination(
    root: &Path,
    workspace_path: WorkspacePath,
    overwrite: bool,
) -> Result<Destination, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("Failed to resolve local workspace root: {error}"))?;
    if !canonical_root.is_dir() {
        return Err("Local workspace root is not a directory".to_string());
    }

    let relative = Path::new(workspace_path.as_str());
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("file_path must identify a file inside the workspace".to_string());
    }
    let file_name = relative
        .file_name()
        .ok_or_else(|| "file_path must identify a file".to_string())?;
    let relative_parent = relative.parent().unwrap_or_else(|| Path::new(""));

    let mut parent = canonical_root.clone();
    for component in relative_parent.components() {
        let Component::Normal(component) = component else {
            return Err("file_path contains an unsupported path component".to_string());
        };
        parent.push(component);
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("file_path crosses a symbolic link".to_string())
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err("A file_path parent component is not a directory".to_string())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent)
                    .map_err(|error| format!("Failed to create download directory: {error}"))?;
            }
            Err(error) => return Err(format!("Failed to inspect download directory: {error}")),
        }
    }

    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("Failed to resolve download directory: {error}"))?;
    if !canonical_parent.is_dir() || !canonical_parent.starts_with(&canonical_root) {
        return Err("file_path resolves outside the workspace".to_string());
    }

    let absolute_path = canonical_parent.join(file_name);
    let existed = match std::fs::symlink_metadata(&absolute_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err("Download destination cannot be a symbolic link".to_string())
        }
        Ok(metadata) if metadata.is_dir() => {
            return Err("Download destination is a directory".to_string())
        }
        Ok(_) if !overwrite => {
            return Err(
                "Download destination already exists; set overwrite=true to replace it".to_string(),
            )
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("Failed to inspect download destination: {error}")),
    };

    Ok(Destination {
        workspace_path,
        absolute_path,
        parent: canonical_parent,
        existed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_prefers_extended_filename_and_sanitizes_paths() {
        let url = Url::parse("https://example.com/fallback.bin").unwrap();
        assert_eq!(
            infer_filename(
                Some(
                    "attachment; filename=plain.bin; filename*=UTF-8''..%2Freport%20%E4%B8%AD.bin"
                ),
                &url,
            ),
            ".._report 中.bin"
        );
    }

    #[test]
    fn filename_fallbacks_cover_query_path_and_windows_reserved_names() {
        let query = Url::parse("https://example.com/path/archive?filename=CON.txt").unwrap();
        assert_eq!(infer_filename(None, &query), "_CON.txt");
        let path = Url::parse("https://example.com/files/report%20final.pdf").unwrap();
        assert_eq!(infer_filename(None, &path), "report final.pdf");
    }

    #[test]
    fn filename_truncation_is_utf8_safe_and_bounded() {
        let name = format!("{}.tar.gz", "数".repeat(200));
        let sanitized = sanitize_filename(&name);
        assert!(sanitized.len() <= MAX_FILENAME_BYTES);
        assert!(sanitized.ends_with(".gz"));
        assert!(std::str::from_utf8(sanitized.as_bytes()).is_ok());
    }

    #[test]
    fn destination_preflight_rejects_collisions_and_traversal() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("existing.bin"), b"original").unwrap();

        let collision = prepare_destination(
            root.path(),
            WorkspacePath::from_normalized("existing.bin"),
            false,
        )
        .unwrap_err();
        assert!(collision.contains("already exists"));

        let traversal = prepare_destination(
            root.path(),
            WorkspacePath::from_normalized("../outside.bin"),
            false,
        )
        .unwrap_err();
        assert!(traversal.contains("inside the workspace"));
    }
}
