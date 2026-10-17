//! Per-session files in the launch workspace.
//!
//! A new session is stored at
//! `<workspace>/.a3s/sessions/YYYY-MM-DD/<session-id>/`. The date is the
//! local calendar day of creation. The directory is owner-only. Resume
//! finds that same directory by session id and does not open a second day.
//! `session.json` names the effect log, and the log itself is
//! `<session-id>.jsonl` in that directory.
//!
//! A legacy `<workspace>/.a3s/effect-log/<session-id>.jsonl` that has no
//! session folder stays where it is.
//!
//! A host-set [`crate::state_root`] override replaces `<workspace>/.a3s`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;

mod catalog;
pub use catalog::{
    list_workspace_sessions, resolve_workspace_session, SessionCatalog, SessionCatalogEntry,
};

pub const SESSION_FOLDER_SCHEMA: &str = "a3s.code.session-folder.v1";

pub fn sessions_root(workspace: &Path) -> PathBuf {
    crate::state_root::workspace_state_dir(workspace).join("sessions")
}

/// Create or reuse this session's folder. The day is the local calendar day.
pub fn ensure_session_dir(workspace: &Path, session_id: &str) -> Result<PathBuf> {
    ensure_session_dir_on(workspace, session_id, chrono::Local::now().date_naive())
}

pub(crate) fn ensure_session_dir_on(
    workspace: &Path,
    session_id: &str,
    day: NaiveDate,
) -> Result<PathBuf> {
    let id = folder_component(session_id)?;
    if let Some(existing) = find_session_dir(workspace, session_id)? {
        write_session_json_if_absent(&existing, session_id, &id, workspace)?;
        return Ok(existing);
    }
    let date = day.format("%Y-%m-%d").to_string();
    let root = sessions_root(workspace);
    ensure_owner_dir(&crate::state_root::workspace_state_dir(workspace))?;
    ensure_owner_dir(&root)?;
    let date_dir = root.join(&date);
    ensure_owner_dir(&date_dir)?;
    let dir = date_dir.join(&id);
    if std::fs::symlink_metadata(&dir).is_ok() {
        bail!("session folder path is already present: {}", dir.display());
    }
    ensure_owner_dir(&dir)?;
    write_session_json(&dir, session_id, &id, workspace)?;
    Ok(dir)
}

/// Startup placement.
///
/// A fresh id gets a date folder. An existing folder is reused. A legacy
/// effect log without a folder is left alone.
pub fn prepare_session_home(workspace: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    if let Some(existing) = find_session_dir(workspace, session_id)? {
        write_session_json_if_absent(
            &existing,
            session_id,
            &folder_component(session_id)?,
            workspace,
        )?;
        return Ok(Some(existing));
    }
    if legacy_effect_log_exists(workspace, session_id) {
        return Ok(None);
    }
    Ok(Some(ensure_session_dir(workspace, session_id)?))
}

pub fn find_session_dir(workspace: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    let id = folder_component(session_id)?;
    let root = sessions_root(workspace);
    let meta = match std::fs::symlink_metadata(&root) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if meta.file_type().is_symlink() {
        bail!("refusing to follow a symlink at {}", root.display());
    }
    if !meta.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if NaiveDate::parse_from_str(name, "%Y-%m-%d").is_err() {
            continue;
        }
        let candidate = entry.path().join(&id);
        let Ok(meta) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        matches.push(candidate);
    }
    matches.sort();
    Ok(matches.into_iter().next())
}

pub fn legacy_effect_log_exists(workspace: &Path, session_id: &str) -> bool {
    let Ok(id) = folder_component(session_id) else {
        return false;
    };
    let path = legacy_effect_log_dir(workspace).join(format!("{id}.jsonl"));
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta.file_type().is_file(),
        Err(_) => false,
    }
}

/// Directory `FileLog` opens for this session.
///
/// A prepared session folder wins. Otherwise the legacy flat directory is
/// used, including for sessions that already have a log there.
pub fn effect_log_dir(workspace: &Path, session_id: &str) -> PathBuf {
    if let Ok(Some(dir)) = find_session_dir(workspace, session_id) {
        let marker = dir.join("session.json");
        if std::fs::symlink_metadata(&marker).is_ok_and(|meta| meta.file_type().is_file()) {
            return dir;
        }
    }
    legacy_effect_log_dir(workspace)
}

pub fn legacy_effect_log_dir(workspace: &Path) -> PathBuf {
    crate::state_root::workspace_state_dir(workspace).join("effect-log")
}

fn folder_component(session_id: &str) -> Result<String> {
    let component = crate::fact_control::thread_for_session(session_id);
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains('/')
        || component.contains('\\')
        || component.contains('\0')
    {
        bail!("session id cannot name a session folder");
    }
    Ok(component)
}

fn ensure_owner_dir(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("refusing to follow a symlink at {}", path.display());
        }
        Ok(meta) if meta.is_dir() => {
            // A pre-existing directory may have been created by an older
            // build or another tool at permissive defaults; tighten it
            // instead of silently keeping conversation state group-readable.
            crate::secure_state::restrict_state_dir(path)?;
            Ok(())
        }
        Ok(_) => bail!("{} is not a directory", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(path) {
                Ok(()) => set_mode(path, 0o700)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let meta = std::fs::symlink_metadata(path)?;
                    if meta.file_type().is_symlink() {
                        bail!("refusing to follow a symlink at {}", path.display());
                    }
                    if !meta.is_dir() {
                        bail!("{} is not a directory", path.display());
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("create {}", path.display()));
                }
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn write_session_json_if_absent(
    dir: &Path,
    session_id: &str,
    folder_id: &str,
    workspace: &Path,
) -> Result<()> {
    let marker = dir.join("session.json");
    if std::fs::symlink_metadata(&marker).is_ok_and(|meta| meta.file_type().is_file()) {
        return Ok(());
    }
    if std::fs::symlink_metadata(&marker).is_ok() {
        bail!("refusing to replace {}", marker.display());
    }
    write_session_json(dir, session_id, folder_id, workspace)
}

fn write_session_json(
    dir: &Path,
    session_id: &str,
    folder_id: &str,
    workspace: &Path,
) -> Result<()> {
    let body = serde_json::json!({
        "schema": SESSION_FOLDER_SCHEMA,
        "id": session_id,
        "created_at": chrono::Local::now().to_rfc3339(),
        "workspace": workspace.display().to_string(),
        "effect_log": format!("{folder_id}.jsonl"),
    });
    let bytes = serde_json::to_vec_pretty(&body)?;
    let temporary = dir.join(".session.json.tmp");
    std::fs::write(&temporary, bytes).with_context(|| format!("write {}", temporary.display()))?;
    set_mode(&temporary, 0o600)?;
    let marker = dir.join("session.json");
    std::fs::rename(&temporary, &marker).with_context(|| format!("rename {}", marker.display()))?;
    // A session with no facts yet still has the log the marker names.
    // Resume admission reads that file; a missing log is not an empty session.
    create_empty_effect_log(&dir, folder_id)?;
    Ok(())
}

fn create_empty_effect_log(dir: &Path, folder_id: &str) -> Result<()> {
    let log = dir.join(format!("{folder_id}.jsonl"));
    match std::fs::symlink_metadata(&log) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("refusing to follow a symlink at {}", log.display());
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&log, b"").with_context(|| format!("write {}", log.display()))?;
            set_mode(&log, 0o600)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_effect::LogStore;

    const ID: &str = "11111111-1111-4111-8111-111111111111";

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn a_session_lands_in_the_local_date_folder() {
        let workspace = tempfile::tempdir().unwrap();
        let before = chrono::Local::now().date_naive();
        let dir = ensure_session_dir(workspace.path(), ID).unwrap();
        let after = chrono::Local::now().date_naive();
        let date_name = dir
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap();
        let before = before.format("%Y-%m-%d").to_string();
        let after = after.format("%Y-%m-%d").to_string();
        assert!(date_name == before || date_name == after);
        assert!(dir.starts_with(sessions_root(workspace.path())));
        assert_eq!(dir.file_name().and_then(|name| name.to_str()), Some(ID));
        let marker = std::fs::read_to_string(dir.join("session.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&marker).unwrap();
        assert_eq!(value["schema"], SESSION_FOLDER_SCHEMA);
        assert_eq!(value["id"], ID);
        assert_eq!(value["effect_log"], format!("{ID}.jsonl"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = std::fs::symlink_metadata(&dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let file_mode = std::fs::symlink_metadata(dir.join("session.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
            assert_eq!(file_mode, 0o600);
            let log_mode = std::fs::symlink_metadata(dir.join(format!("{ID}.jsonl")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(log_mode, 0o600);
        }
        assert_eq!(std::fs::read(dir.join(format!("{ID}.jsonl"))).unwrap(), b"");
    }

    #[test]
    fn resume_keeps_the_original_date() {
        let workspace = tempfile::tempdir().unwrap();
        let created = ensure_session_dir_on(workspace.path(), ID, day(2026, 10, 4)).unwrap();
        let again = ensure_session_dir_on(workspace.path(), ID, day(2026, 10, 5)).unwrap();
        assert_eq!(created, again);
        assert!(!sessions_root(workspace.path()).join("2026-10-05").exists());
        let home = prepare_session_home(workspace.path(), ID).unwrap().unwrap();
        assert_eq!(home, created);
    }

    #[test]
    fn the_effect_log_is_stored_in_the_session_folder() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = ensure_session_dir_on(workspace.path(), ID, day(2026, 10, 4)).unwrap();
        let log = a3s_effect::FileLog::open(effect_log_dir(workspace.path(), ID)).unwrap();
        log.append(
            ID,
            &[a3s_effect::NewFact {
                kind: "user.message".into(),
                key: "m".into(),
                payload: serde_json::json!({ "text": "hello" }),
            }],
            None,
        )
        .unwrap();
        assert!(dir.join(format!("{ID}.jsonl")).is_file());
        assert!(!legacy_effect_log_dir(workspace.path())
            .join(format!("{ID}.jsonl"))
            .exists());
        let facts = crate::fact_control::read_workspace_facts(workspace.path(), ID).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].payload["text"], "hello");
    }

    #[test]
    fn a_legacy_effect_log_stays_in_place() {
        let workspace = tempfile::tempdir().unwrap();
        let id = "legacy-session";
        let legacy = legacy_effect_log_dir(workspace.path());
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(format!("{id}.jsonl")), "{}\n").unwrap();
        assert!(prepare_session_home(workspace.path(), id)
            .unwrap()
            .is_none());
        assert!(!sessions_root(workspace.path()).exists());
        assert_eq!(effect_log_dir(workspace.path(), id), legacy);
    }

    #[test]
    fn dotdot_cannot_name_a_session_folder() {
        let workspace = tempfile::tempdir().unwrap();
        let error = ensure_session_dir(workspace.path(), "..").unwrap_err();
        assert!(error.to_string().contains("session folder"));
        assert!(!workspace.path().join("session.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_sessions_root_is_refused() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let a3s = workspace.path().join(".a3s");
        std::fs::create_dir(&a3s).unwrap();
        std::os::unix::fs::symlink(outside.path(), a3s.join("sessions")).unwrap();
        let error = ensure_session_dir(workspace.path(), ID).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}
