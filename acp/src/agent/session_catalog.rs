//! The pager's list query projects Code facts; it never opens an AgentSession.

use std::path::PathBuf;

use agent_client_protocol as acp;
use serde::Deserialize;
use serde_json::json;

use super::workspace_scope::canonical_workspace;
use super::A3sCodeAgent;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListRequest {
    cwd: Option<PathBuf>,
    limit: Option<usize>,
    query: Option<String>,
    headless: Option<HeadlessPolicy>,
    // Compatibility with the pager. This never widens the host's scope.
    allow_relax: Option<bool>,
    #[serde(rename = "_meta")]
    meta: Option<ListMeta>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListMeta {
    #[serde(rename = "x.ai/facetFilters")]
    facets: Option<Facets>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Facets {
    kind: Vec<SessionFacet>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(try_from = "String")]
enum SessionFacet {
    Build,
    Chat,
}

impl SessionFacet {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "build" => Ok(Self::Build),
            "chat" => Ok(Self::Chat),
            _ => Err(format!("unknown session facet: {value}")),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Chat => "chat",
        }
    }
}

impl TryFrom<String> for SessionFacet {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(try_from = "String")]
enum HeadlessPolicy {
    Include,
    Exclude,
    Only,
}

impl HeadlessPolicy {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "include" => Ok(Self::Include),
            "exclude" => Ok(Self::Exclude),
            "only" => Ok(Self::Only),
            _ => Err(format!("unknown headless policy: {value}")),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
            Self::Only => "only",
        }
    }
}

impl TryFrom<String> for HeadlessPolicy {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl A3sCodeAgent {
    pub(super) async fn handle_session_list(
        &self,
        args: &acp::ExtRequest,
    ) -> acp::Result<acp::ExtResponse> {
        if args.params.get().len() > 1024 * 1024 {
            return Err(acp::Error::invalid_params().data("session list request exceeds 1 MiB"));
        }
        let request: ListRequest = serde_json::from_str(args.params.get())
            .map_err(|_| acp::Error::invalid_params().data("invalid session list request"))?;
        let limit = request.limit.unwrap_or(30);
        if !(1..=100).contains(&limit)
            || request.query.as_ref().is_some_and(|q| q.len() > 4096)
            || request
                .meta
                .as_ref()
                .and_then(|m| m.facets.as_ref())
                .is_some_and(|f| f.kind.len() > 2)
        {
            return Err(acp::Error::invalid_params().data("invalid session list bounds"));
        }
        let launch_workspace = self.launch.workspace.clone();
        let permit = self
            .catalog_queries
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                acp::Error::invalid_params().data("session catalog query already in progress")
            })?;
        let response = tokio::task::spawn_blocking(move || {
            // The worker owns admission: cancelling the async waiter cannot
            // admit another scan while this one still uses I/O and memory.
            let _permit = permit;
            let workspace = canonical_workspace(&launch_workspace, request.cwd.as_deref())?;
            let _allow_relax = request.allow_relax;
            let headless = request.headless.unwrap_or(HeadlessPolicy::Include);
            let facets = request.meta.and_then(|m| m.facets);
            let includes_build = facets
                .as_ref()
                .is_none_or(|f| f.kind.contains(&SessionFacet::Build));
            let facet_names: Option<Vec<_>> = facets
                .as_ref()
                .map(|f| f.kind.iter().map(SessionFacet::as_str).collect());
            let mut page = if includes_build {
                a3s_code_core::session_folder::list_workspace_sessions(
                    &workspace,
                    limit,
                    request.query.as_deref(),
                )
                .map_err(|_| {
                    acp::Error::internal_error().data("session catalog could not be read")
                })?
            } else {
                a3s_code_core::session_folder::SessionCatalog {
                    sessions: Vec::new(),
                    truncated: false,
                    incomplete: false,
                    skipped_entries: 0,
                }
            };
            // Existing Code markers/facts do not store an authoritative
            // headless kind. Unknown is never invented from a title or id.
            let unclassified = if includes_build && headless != HeadlessPolicy::Include {
                page.incomplete |= !page.sessions.is_empty() || page.truncated;
                let count = page.sessions.len();
                page.sessions.clear();
                count
            } else {
                0
            };
            let sessions: Vec<_> = page
                .sessions
                .into_iter()
                .map(|entry| {
                    let mut row = json!(entry);
                    row["source"] = json!("local");
                    row["_meta"] = json!({"a3sCode/sessionCatalog": "a3s.code.session-catalog.v1"});
                    row
                })
                .collect();
            Ok::<_, acp::Error>(json!({
                "sessions": sessions,
                "_meta": {
                    "x.ai/listScope": "cwd",
                    "a3sCode/sessionCatalog": {
                        "schema": "a3s.code.session-catalog.v1",
                        "incomplete": page.incomplete,
                        "truncated": page.truncated,
                        "skippedEntries": page.skipped_entries,
                        "unclassifiedPageEntries": unclassified,
                        "headlessPolicy": headless.as_str(),
                        "kindFacets": facet_names,
                    }
                }
            }))
        })
        .await
        .map_err(|_| acp::Error::internal_error().data("session catalog query failed"))??;
        let raw =
            serde_json::value::to_raw_value(&response).map_err(|_| acp::Error::internal_error())?;
        Ok(acp::ExtResponse::new(raw.into()))
    }
}
