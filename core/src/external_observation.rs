//! Typed external observations are run inputs. Clearing one requires a newer
//! observation of the same subject or a LOOP-1 waiver, not a sentence.
//!
//! Concurrent writes to a path another session has dirtied fail closed.

use crate::harness_loop::CompletionWaiverV1;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const EXTERNAL_OBSERVATION_SCHEMA: &str = "a3s.code.external-observation.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredAction {
    None,
    WorkspaceChange,
    Address,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalObservationV1 {
    pub schema: String,
    pub kind: String,
    pub subject: String,
    pub digest: String,
    pub payload: String,
    pub source_revision: Option<String>,
    pub required_action: RequiredAction,
}

impl ExternalObservationV1 {
    pub fn new(
        kind: impl Into<String>,
        subject: impl Into<String>,
        digest: impl Into<String>,
        payload: impl Into<String>,
        required_action: RequiredAction,
    ) -> Option<Self> {
        let digest = digest.into();
        let payload = payload.into();
        if digest.trim().is_empty() || payload.len() > 8_192 {
            return None;
        }
        Some(Self {
            schema: EXTERNAL_OBSERVATION_SCHEMA.to_string(),
            kind: kind.into(),
            subject: subject.into(),
            digest,
            payload,
            source_revision: None,
            required_action,
        })
    }

    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.source_revision = Some(revision.into());
        self
    }

    pub fn model_input_fragment(&self) -> String {
        format!(
            "[external observation schema={} kind={} subject={} digest={} action={:?}]",
            self.schema, self.kind, self.subject, self.digest, self.required_action
        )
    }
}

pub fn still_open(
    observations: &[ExternalObservationV1],
    waivers: &[CompletionWaiverV1],
    effect_digest: &str,
) -> Vec<ExternalObservationV1> {
    let mut newest: HashMap<String, &ExternalObservationV1> = HashMap::new();
    for observation in observations {
        newest.insert(observation.subject.clone(), observation);
    }
    newest
        .into_values()
        .filter(|observation| observation.required_action != RequiredAction::None)
        .filter(|observation| {
            !waivers.iter().any(|waiver| {
                waiver.effect_digest == observation.digest
                    || (!effect_digest.is_empty() && waiver.effect_digest == effect_digest)
            })
        })
        .cloned()
        .collect()
}

pub fn blocks_success(open: &[ExternalObservationV1]) -> bool {
    open.iter()
        .any(|observation| observation.required_action == RequiredAction::WorkspaceChange)
}

fn dirty() -> &'static Mutex<HashMap<PathBuf, String>> {
    static DIRTY: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    DIRTY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Refuse a missing session instead of sharing an `"anonymous"` owner.
pub fn claim_bound_write(
    session_id: Option<&str>,
    workspace: &Path,
    relative: &str,
) -> Result<(), String> {
    let Some(session_id) = session_id.filter(|id| !id.trim().is_empty()) else {
        return Err("write requires a session id".to_string());
    };
    claim_write(session_id, workspace, relative)
}

/// One dirty-path identity for `guest.txt`, `./guest.txt`, and
/// `subdir/../guest.txt`. Joining the raw spelling lets a second session
/// hide a claim by respelling the same file.
fn claim_identity(workspace: &Path, relative: &str) -> Result<PathBuf, String> {
    let root = canonical_root(workspace);
    let raw = Path::new(relative);
    let under_root = if raw.is_absolute() {
        raw.strip_prefix(&root)
            .or_else(|_| raw.strip_prefix(workspace))
            .map(Path::to_path_buf)
            .map_err(|_| "write path must stay inside the workspace".to_string())?
    } else {
        raw.to_path_buf()
    };
    let mut parts = Vec::new();
    for component in under_root.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err("write path must stay inside the workspace".to_string());
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("write path must stay inside the workspace".to_string());
            }
        }
    }
    if parts.is_empty() {
        return Err("write path must name a file".to_string());
    }
    Ok(parts.into_iter().fold(root, |path, part| path.join(part)))
}

fn canonical_root(workspace: &Path) -> PathBuf {
    workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
}

pub fn claim_write(session_id: &str, workspace: &Path, relative: &str) -> Result<(), String> {
    let path = claim_identity(workspace, relative)?;
    let mut guard = dirty().lock().expect("dirty paths");
    if let Some(owner) = guard.get(&path) {
        if owner != session_id {
            return Err(format!(
                "concurrent write to {} is already owned by session {owner}",
                path.display()
            ));
        }
        return Ok(());
    }
    guard.insert(path, session_id.to_string());
    Ok(())
}

/// Refuse a workspace-wide overwrite when another session already owns a path
/// here. Checkout and stash do not name every file they will touch.
pub fn foreign_workspace_claim(session_id: Option<&str>, workspace: &Path) -> Result<(), String> {
    let Some(session_id) = session_id.filter(|id| !id.trim().is_empty()) else {
        return Err("write requires a session id".to_string());
    };
    refuse_foreign_workspace_owner(Some(session_id), workspace)
}

/// Refuse when a different session already owns a dirty path in this workspace.
///
/// A missing session is not an owner, so any existing claim is foreign. An
/// empty claim map is not a refusal: callers that still allow an unbound
/// command (bash) must not treat "no session" as "no owner anywhere".
pub fn refuse_foreign_workspace_owner(
    session_id: Option<&str>,
    workspace: &Path,
) -> Result<(), String> {
    let session_id = session_id.unwrap_or("").trim();
    let workspace = canonical_root(workspace);
    let guard = dirty().lock().expect("dirty paths");
    if let Some((path, owner)) = guard.iter().find(|(path, owner)| {
        !owner.is_empty() && owner.as_str() != session_id && path.starts_with(&workspace)
    }) {
        return Err(format!(
            "concurrent write to {} is already owned by session {owner}",
            path.display()
        ));
    }
    Ok(())
}

pub fn session_owns_write(session_id: &str, workspace: &Path, relative: &str) -> bool {
    let Ok(path) = claim_identity(workspace, relative) else {
        return false;
    };
    dirty()
        .lock()
        .expect("dirty paths")
        .get(&path)
        .is_some_and(|owner| owner == session_id)
}

/// Drop a claim this session holds, and no other session's claim.
/// Move a finished child's claim to the parent, or claim an unowned dirty
/// path for the parent. A path another live session already owns is left
/// alone.
pub fn adopt_write_claim(
    from_session: &str,
    to_session: &str,
    workspace: &Path,
    relative: &str,
) -> Result<(), String> {
    let to_session = to_session.trim();
    if to_session.is_empty() {
        return Err("write requires a session id".to_string());
    }
    let path = claim_identity(workspace, relative)?;
    let mut guard = dirty().lock().expect("dirty paths");
    match guard.get(&path).map(String::as_str) {
        Some(owner) if owner == to_session || owner == from_session => {
            guard.insert(path, to_session.to_string());
            Ok(())
        }
        Some(owner) => Err(format!(
            "concurrent write to {} is already owned by session {owner}",
            path.display()
        )),
        None => {
            guard.insert(path, to_session.to_string());
            Ok(())
        }
    }
}

pub fn release_write_claim(session_id: &str, workspace: &Path, relative: &str) {
    let Ok(path) = claim_identity(workspace, relative) else {
        return;
    };
    let mut guard = dirty().lock().expect("dirty paths");
    if guard.get(&path).is_some_and(|owner| owner == session_id) {
        guard.remove(&path);
    }
}

pub fn release_session(session_id: &str) {
    dirty()
        .lock()
        .expect("dirty paths")
        .retain(|_, owner| owner != session_id);
}

pub fn observation_from_value(value: &Value) -> Option<ExternalObservationV1> {
    serde_json::from_value(value.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_on_the_model_input_fragment() {
        let observation = ExternalObservationV1::new(
            "ci",
            "build",
            "obs-1",
            "job failed",
            RequiredAction::WorkspaceChange,
        )
        .unwrap();
        let fragment = observation.model_input_fragment();
        assert!(fragment.contains("obs-1"));
        assert!(fragment.contains(EXTERNAL_OBSERVATION_SCHEMA));
    }

    #[test]
    fn final_answer_does_not_clear_a_required_workspace_change() {
        let observation = ExternalObservationV1::new(
            "ci",
            "build",
            "obs-1",
            "job failed",
            RequiredAction::WorkspaceChange,
        )
        .unwrap();
        let open = still_open(&[observation], &[], "");
        assert!(blocks_success(&open));
        let waiver = CompletionWaiverV1::new("obs-1", "host accepted").unwrap();
        let cleared = still_open(&open, &[waiver], "");
        assert!(!blocks_success(&cleared));
    }

    #[test]
    fn second_session_cannot_silently_win_a_dirty_path() {
        let root = tempfile::tempdir().unwrap();
        claim_write("one", root.path(), "src/lib.rs").unwrap();
        let error = claim_write("two", root.path(), "src/lib.rs").unwrap_err();
        assert!(error.contains("already owned"));
        let missing = claim_bound_write(None, root.path(), "src/other.rs").unwrap_err();
        assert!(missing.contains("session id"));
        let blocked = foreign_workspace_claim(Some("two"), root.path()).unwrap_err();
        assert!(blocked.contains("already owned"));
        claim_write("two", root.path(), "src/other.rs").unwrap();
        claim_write("one", root.path(), "src/lib.rs").unwrap();
        release_session("one");
        release_session("two");
    }

    #[test]
    fn adopt_moves_a_child_claim_to_the_parent_and_does_not_steal() {
        let root = tempfile::tempdir().unwrap();
        claim_write("task-run-child", root.path(), "guest.txt").unwrap();
        claim_write("other-live", root.path(), "held.txt").unwrap();

        adopt_write_claim("task-run-child", "task-parent", root.path(), "guest.txt").unwrap();
        let stolen = adopt_write_claim("task-run-child", "task-parent", root.path(), "held.txt");

        assert!(session_owns_write("task-parent", root.path(), "guest.txt"));
        assert!(!session_owns_write(
            "task-run-child",
            root.path(),
            "guest.txt"
        ));
        assert!(stolen.unwrap_err().contains("already owned"));
        assert!(session_owns_write("other-live", root.path(), "held.txt"));
        release_session("task-parent");
        release_session("task-run-child");
        release_session("other-live");
    }

    #[test]
    fn newer_observation_of_the_same_subject_replaces_the_open_action() {
        let first = ExternalObservationV1::new(
            "ci",
            "build",
            "obs-1",
            "failed",
            RequiredAction::WorkspaceChange,
        )
        .unwrap();
        let second =
            ExternalObservationV1::new("ci", "build", "obs-2", "green", RequiredAction::None)
                .unwrap();
        let open = still_open(&[first, second], &[], "");
        assert!(!blocks_success(&open));
    }
}
