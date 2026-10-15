//! Derived consumers of one workspace source snapshot.
//!
//! A search hit, a symbol result, a model context item, and an evidence fact
//! each carry the admitting snapshot digest. Presentation as current uses
//! that snapshot: a result bound to an older snapshot is not returned as
//! current against a newer live snapshot. An index rebuild that the snapshot
//! still considers current stays. This module does not scan, watch, or own
//! an index. The manifest stays the scan authority.

#[cfg(feature = "research")]
use std::collections::BTreeSet;

use crate::code_intelligence::{CodeQueryResult, SymbolInformation};
use crate::content_digest::{digest_bytes, validate_digest};
use crate::context::{ContextItem, ContextResult};
#[cfg(feature = "research")]
use crate::research::ResearchEvidenceFactV1;
use crate::workspace::{
    LexicalSearchResult, WorkspaceSourceSnapshotError, WorkspaceSourceSnapshotV1,
};

const SYMBOL_DIGEST_DOMAIN: &str = "a3s.code.source-consumer.symbol.v1";
const CONTEXT_DIGEST_DOMAIN: &str = "a3s.code.source-consumer.context.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(not(feature = "research"), allow(dead_code))]
pub(crate) enum SourceConsumerKind {
    SearchHit,
    SymbolResult,
    ModelContextItem,
    EvidenceFact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceBoundConsumer {
    kind: SourceConsumerKind,
    item_digest: String,
    snapshot_digest: String,
}

#[cfg_attr(not(feature = "research"), allow(dead_code))]
impl SourceBoundConsumer {
    pub(crate) fn kind(&self) -> SourceConsumerKind {
        self.kind
    }

    pub(crate) fn snapshot_digest(&self) -> &str {
        &self.snapshot_digest
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum SourceConsumerError {
    Snapshot(WorkspaceSourceSnapshotError),
    RevisionMismatch,
    Stale,
    MissingSnapshot,
    Digest,
}

#[derive(Clone, Debug)]
#[cfg(feature = "research")]
pub(crate) struct DerivedSourceConsumers {
    snapshot: WorkspaceSourceSnapshotV1,
    search: Vec<SourceBoundConsumer>,
    symbols: Vec<SourceBoundConsumer>,
    context: Vec<SourceBoundConsumer>,
    evidence: Vec<SourceBoundConsumer>,
}

#[cfg(feature = "research")]
impl DerivedSourceConsumers {
    pub(crate) fn snapshot_digest(&self) -> &str {
        &self.snapshot.snapshot_digest
    }

    /// Consumers that may be presented as current. A stale or invalid live
    /// snapshot yields an empty set, never a stale result labeled current.
    pub(crate) fn present_current(
        &self,
        live: &WorkspaceSourceSnapshotV1,
    ) -> PresentedSourceConsumers {
        if live.validate().is_err() || self.snapshot.is_stale_against(live) {
            return PresentedSourceConsumers {
                consumers: Vec::new(),
            };
        }
        let mut consumers = Vec::new();
        consumers.extend(self.search.iter().cloned());
        consumers.extend(self.symbols.iter().cloned());
        consumers.extend(self.context.iter().cloned());
        consumers.extend(self.evidence.iter().cloned());
        PresentedSourceConsumers { consumers }
    }
}

#[derive(Clone, Debug)]
#[cfg(feature = "research")]
pub(crate) struct PresentedSourceConsumers {
    consumers: Vec<SourceBoundConsumer>,
}

#[cfg(feature = "research")]
impl PresentedSourceConsumers {
    pub(crate) fn is_empty(&self) -> bool {
        self.consumers.is_empty()
    }

    pub(crate) fn kinds(&self) -> BTreeSet<SourceConsumerKind> {
        self.consumers
            .iter()
            .map(SourceBoundConsumer::kind)
            .collect()
    }

    pub(crate) fn snapshot_digests(&self) -> BTreeSet<&str> {
        self.consumers
            .iter()
            .map(SourceBoundConsumer::snapshot_digest)
            .collect()
    }

    pub(crate) fn contains_item(&self, digest: &str) -> bool {
        self.consumers
            .iter()
            .any(|consumer| consumer.item_digest == digest)
    }
}

#[cfg(feature = "research")]
pub(crate) fn bind_derived_consumers(
    snapshot: &WorkspaceSourceSnapshotV1,
    search: &LexicalSearchResult,
    symbols: &CodeQueryResult<SymbolInformation>,
    context: &ContextItem,
    fact: &ResearchEvidenceFactV1,
) -> Result<DerivedSourceConsumers, SourceConsumerError> {
    let search = trace_search_hits(snapshot, search)?;
    let symbols = trace_symbol_results(snapshot, symbols)?;
    let context = vec![trace_model_context_item(snapshot, context)?];
    let evidence = vec![trace_evidence_fact(snapshot, fact)?];
    Ok(DerivedSourceConsumers {
        snapshot: snapshot.clone(),
        search,
        symbols,
        context,
        evidence,
    })
}

pub(crate) fn trace_search_hits(
    snapshot: &WorkspaceSourceSnapshotV1,
    result: &LexicalSearchResult,
) -> Result<Vec<SourceBoundConsumer>, SourceConsumerError> {
    snapshot.validate().map_err(SourceConsumerError::Snapshot)?;
    if result.catalog_revision != snapshot.catalog_revision
        || result.source_revision != snapshot.manifest_version
    {
        return Err(SourceConsumerError::RevisionMismatch);
    }
    let mut bound = Vec::with_capacity(result.hits.len());
    for hit in &result.hits {
        if hit.chunk.source_revision != snapshot.manifest_version {
            return Err(SourceConsumerError::RevisionMismatch);
        }
        validate_digest(hit.chunk.content_digest.as_ref())
            .map_err(|_| SourceConsumerError::Digest)?;
        bound.push(SourceBoundConsumer {
            kind: SourceConsumerKind::SearchHit,
            item_digest: hit.chunk.content_digest.to_string(),
            snapshot_digest: snapshot.snapshot_digest.clone(),
        });
    }
    Ok(bound)
}

pub(crate) fn trace_symbol_results(
    snapshot: &WorkspaceSourceSnapshotV1,
    query: &CodeQueryResult<SymbolInformation>,
) -> Result<Vec<SourceBoundConsumer>, SourceConsumerError> {
    snapshot.validate().map_err(SourceConsumerError::Snapshot)?;
    if query
        .document
        .as_ref()
        .is_some_and(|document| document.stale)
    {
        return Err(SourceConsumerError::Stale);
    }
    if query.workspace_revision != snapshot.manifest_version {
        return Err(SourceConsumerError::RevisionMismatch);
    }
    let mut bound = Vec::with_capacity(query.items.len());
    for symbol in &query.items {
        bound.push(SourceBoundConsumer {
            kind: SourceConsumerKind::SymbolResult,
            item_digest: symbol_item_digest(symbol),
            snapshot_digest: snapshot.snapshot_digest.clone(),
        });
    }
    Ok(bound)
}

#[cfg_attr(not(feature = "research"), allow(dead_code))]
pub(crate) fn trace_model_context_item(
    snapshot: &WorkspaceSourceSnapshotV1,
    item: &ContextItem,
) -> Result<SourceBoundConsumer, SourceConsumerError> {
    snapshot.validate().map_err(SourceConsumerError::Snapshot)?;
    let Some(digest) = item.source_snapshot_digest() else {
        return Err(SourceConsumerError::MissingSnapshot);
    };
    if digest != snapshot.snapshot_digest {
        return Err(SourceConsumerError::Digest);
    }
    validate_digest(digest).map_err(|_| SourceConsumerError::Digest)?;
    Ok(SourceBoundConsumer {
        kind: SourceConsumerKind::ModelContextItem,
        item_digest: digest_bytes(
            CONTEXT_DIGEST_DOMAIN,
            format!("{}\0{}", item.id, item.content).as_bytes(),
        ),
        snapshot_digest: snapshot.snapshot_digest.clone(),
    })
}

#[cfg(feature = "research")]
pub(crate) fn trace_evidence_fact(
    snapshot: &WorkspaceSourceSnapshotV1,
    fact: &ResearchEvidenceFactV1,
) -> Result<SourceBoundConsumer, SourceConsumerError> {
    snapshot.validate().map_err(SourceConsumerError::Snapshot)?;
    fact.validate().map_err(|_| SourceConsumerError::Digest)?;
    let Some(digest) = fact.source_snapshot_digest() else {
        return Err(SourceConsumerError::MissingSnapshot);
    };
    if digest != snapshot.snapshot_digest {
        return Err(SourceConsumerError::Digest);
    }
    Ok(SourceBoundConsumer {
        kind: SourceConsumerKind::EvidenceFact,
        item_digest: fact.fact_digest.clone(),
        snapshot_digest: snapshot.snapshot_digest.clone(),
    })
}

/// Keep unstamped context. Drop a stamped item whose digest is not the live
/// snapshot. A stamped item is never presented as current against a different
/// snapshot when the caller only has the live digest.
pub(crate) fn retain_current_context_results(
    results: &[ContextResult],
    live: &WorkspaceSourceSnapshotV1,
) -> Vec<ContextResult> {
    let live_ok = live.validate().is_ok();
    results
        .iter()
        .map(|result| {
            let mut kept = ContextResult::new(result.provider.clone());
            kept.truncated = result.truncated;
            for item in &result.items {
                let stamped = item.source_snapshot_digest();
                let current = match stamped {
                    None => true,
                    Some(digest) => live_ok && digest == live.snapshot_digest,
                };
                if current {
                    kept.add_item(item.clone());
                }
            }
            kept
        })
        .collect()
}

fn symbol_item_digest(symbol: &SymbolInformation) -> String {
    let identity = format!(
        "{}\0{:?}\0{}\0{}:{}-{}:{}",
        symbol.name,
        symbol.kind,
        symbol.location.path.as_str(),
        symbol.location.range.start.line,
        symbol.location.range.start.character,
        symbol.location.range.end.line,
        symbol.location.range.end.character,
    );
    digest_bytes(SYMBOL_DIGEST_DOMAIN, identity.as_bytes())
}

#[cfg(all(test, feature = "research"))]
mod tests {
    use super::*;
    use crate::code_intelligence::{
        CodeLocation, CodePosition, CodeRange, CodeSymbolKind, DocumentRevision, DocumentSnapshot,
    };
    use crate::context::ContextType;
    use crate::research::ResearchEvidenceFactKindV1;
    use crate::workspace::retrieval::{ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog};
    use crate::workspace::{
        derived_search_is_current, workspace_content_digest, LocalWorkspaceFile,
        LocalWorkspaceFileStatus, LocalWorkspaceManifestSnapshot, WorkspacePath,
        WorkspaceSourceSnapshotV1, BUILTIN_ELIGIBILITY_REVISION,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn catalog() -> Arc<WorkspaceChunkCatalog> {
        WorkspaceChunkCatalog::new(
            ChunkingConfig::default(),
            ChunkCatalogLimits {
                max_files: 64,
                max_chunks: 512,
                max_text_bytes: 1024 * 1024,
                max_index_bytes: 8 * 1024 * 1024,
            },
        )
        .unwrap()
    }

    fn manifest_for(version: u64, path: &str, size: u64) -> LocalWorkspaceManifestSnapshot {
        LocalWorkspaceManifestSnapshot {
            version,
            root: std::path::PathBuf::from("/tmp/ws"),
            files: vec![LocalWorkspaceFile {
                path: path.to_owned(),
                size,
                modified_ms: Some(7),
                language: Some("rust".to_owned()),
                status: LocalWorkspaceFileStatus::Tracked,
                binary: false,
                generated: false,
            }],
            scanned_at_ms: 10_000 + version,
        }
    }

    fn snapshot_for(
        manifest: &LocalWorkspaceManifestSnapshot,
        catalog_revision: u64,
        index_generation: u64,
    ) -> WorkspaceSourceSnapshotV1 {
        WorkspaceSourceSnapshotV1::from_manifest(
            manifest,
            workspace_content_digest(manifest),
            BUILTIN_ELIGIBILITY_REVISION,
            manifest.version,
            catalog_revision,
            index_generation,
        )
        .unwrap()
    }

    #[test]
    fn derived_consumers_trace_to_one_snapshot_and_edits_are_not_current() {
        let catalog = catalog();
        let path = WorkspacePath::from_normalized("src/main.rs");
        let source = "fn current_marker() {}\n";
        catalog
            .replace_file(&path, Some("rust"), 1, source)
            .unwrap();
        let admitted = catalog.snapshot().unwrap();
        let manifest = manifest_for(1, "src/main.rs", source.len() as u64);
        let snapshot = snapshot_for(&manifest, admitted.revision(), admitted.revision());
        catalog.admit_source_snapshot(snapshot.clone()).unwrap();
        let admitted_again = catalog.admitted_source_snapshot().unwrap().unwrap();
        assert_eq!(admitted_again.snapshot_digest, snapshot.snapshot_digest);

        let request = crate::workspace::LexicalSearchRequest::new("current_marker");
        let search = admitted.lexical_search(&request).unwrap();
        assert!(
            !search.hits.is_empty(),
            "search hit must exist so the consumer trace is not vacuous"
        );

        let symbol = SymbolInformation {
            name: "current_marker".to_owned(),
            kind: CodeSymbolKind::Function,
            location: CodeLocation {
                path: path.clone(),
                range: CodeRange::new(CodePosition::new(0, 3), CodePosition::new(0, 17)),
            },
            container_name: None,
        };
        let symbols = CodeQueryResult {
            items: vec![symbol],
            truncated: false,
            workspace_revision: snapshot.manifest_version,
            document: None,
        };
        let chunk_text = search.hits[0].chunk.text.to_string();
        let context = ContextItem::new(
            "src/main.rs#current_marker",
            ContextType::Resource,
            chunk_text,
        )
        .with_source_snapshot(snapshot.snapshot_digest.clone());
        let mut metadata = BTreeMap::new();
        metadata.insert("locator".to_owned(), "src/main.rs:1".to_owned());
        let fact = ResearchEvidenceFactV1::new(
            "run-1",
            1,
            ResearchEvidenceFactKindV1::SourceSpan,
            "src-main",
            Some(search.hits[0].chunk.content_digest.to_string()),
            search.hits[0].chunk.content_digest.to_string(),
            metadata,
            snapshot.observed_at_ms,
        )
        .unwrap()
        .with_source_snapshot_digest(&snapshot.snapshot_digest)
        .unwrap();

        let derived =
            bind_derived_consumers(&snapshot, &search, &symbols, &context, &fact).unwrap();
        assert_eq!(derived.snapshot_digest(), snapshot.snapshot_digest);
        let current = derived.present_current(&snapshot);
        assert_eq!(
            current.kinds(),
            BTreeSet::from([
                SourceConsumerKind::SearchHit,
                SourceConsumerKind::SymbolResult,
                SourceConsumerKind::ModelContextItem,
                SourceConsumerKind::EvidenceFact,
            ])
        );
        assert_eq!(
            current.snapshot_digests(),
            BTreeSet::from([snapshot.snapshot_digest.as_str()])
        );
        assert!(current.contains_item(search.hits[0].chunk.content_digest.as_ref()));
        assert!(current.contains_item(&fact.fact_digest));
        assert!(derived_search_is_current(
            &snapshot,
            search.catalog_revision,
            search.source_revision,
            admitted.revision(),
            admitted.source_revision(),
            Some(&snapshot),
        ));

        let rebuilt = snapshot_for(
            &manifest,
            snapshot.catalog_revision,
            snapshot.index_generation + 1,
        );
        let rebuilt_current = derived.present_current(&rebuilt);
        assert_eq!(rebuilt_current.kinds(), current.kinds());
        assert!(!rebuilt.snapshot_digest.is_empty());

        catalog
            .replace_file(&path, Some("rust"), 2, "fn current_marker() { edited }\n")
            .unwrap();
        let edited = catalog.snapshot().unwrap();
        let edited_manifest = manifest_for(
            2,
            "src/main.rs",
            "fn current_marker() { edited }\n".len() as u64,
        );
        let live = snapshot_for(&edited_manifest, edited.revision(), edited.revision());
        assert!(snapshot.is_stale_against(&live));
        let presented = derived.present_current(&live);
        assert!(presented.is_empty());
        assert!(presented.snapshot_digests().is_empty());
        assert!(!derived_search_is_current(
            &snapshot,
            search.catalog_revision,
            search.source_revision,
            edited.revision(),
            edited.source_revision(),
            Some(&live),
        ));

        let mut results = vec![ContextResult::new("workspace")];
        results[0].add_item(context.clone());
        results[0].add_item(ContextItem::new("unstamped", ContextType::Resource, "keep"));
        let retained = retain_current_context_results(&results, &live);
        assert_eq!(retained[0].items.len(), 1);
        assert_eq!(retained[0].items[0].id, "unstamped");
        let retained_same = retain_current_context_results(&results, &snapshot);
        assert_eq!(retained_same[0].items.len(), 2);
    }

    #[test]
    fn stale_symbol_document_and_foreign_fact_do_not_trace() {
        let catalog = catalog();
        let path = WorkspacePath::from_normalized("src/main.rs");
        catalog
            .replace_file(&path, Some("rust"), 4, "fn current_marker() {}\n")
            .unwrap();
        let admitted = catalog.snapshot().unwrap();
        let manifest = manifest_for(4, "src/main.rs", 24);
        let snapshot = snapshot_for(&manifest, admitted.revision(), admitted.revision());
        let symbols = CodeQueryResult {
            items: vec![SymbolInformation {
                name: "current_marker".to_owned(),
                kind: CodeSymbolKind::Function,
                location: CodeLocation {
                    path,
                    range: CodeRange::new(CodePosition::new(0, 0), CodePosition::new(0, 1)),
                },
                container_name: None,
            }],
            truncated: false,
            workspace_revision: snapshot.manifest_version,
            document: Some(DocumentSnapshot {
                revision: DocumentRevision::new(4),
                content_hash:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_owned(),
                stale: true,
            }),
        };
        assert!(matches!(
            trace_symbol_results(&snapshot, &symbols),
            Err(SourceConsumerError::Stale)
        ));

        let fact = ResearchEvidenceFactV1::new(
            "run-1",
            1,
            ResearchEvidenceFactKindV1::Source,
            "source-1",
            None,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
            BTreeMap::new(),
            1,
        )
        .unwrap();
        let foreign = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let rebound = fact.clone().with_source_snapshot_digest(foreign).unwrap();
        assert!(matches!(
            trace_evidence_fact(&snapshot, &rebound),
            Err(SourceConsumerError::Digest)
        ));
        assert!(fact
            .with_source_snapshot_digest(foreign)
            .unwrap()
            .with_source_snapshot_digest(&snapshot.snapshot_digest)
            .is_err());
    }
}
