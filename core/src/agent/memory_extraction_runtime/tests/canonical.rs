#![cfg(test)]

mod support;

use a3s_memory::repository::{
    EvidenceKind, MemoryNamespace, MemoryOperation, MemoryQuery, MemoryRepository, MemoryStatus,
};
use a3s_memory::{MemoryItem, MemoryType};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::sync::atomic::Ordering;
use support::*;

// These fixtures exercise the original private extractor and selected writer.
// They do not qualify actual AgentSession lifecycle or model semantics (T007+).

#[tokio::test]
async fn same_bytes_host_and_model_authority_matrix_on_both_stores() {
    for kind in STORES {
        for (host, text, count, disposition, rejected) in [
            (true, None, 1, None, "0"),
            (
                false,
                Some(output(vec![proposal(CONTENT)])),
                1,
                Some("stored"),
                "0",
            ),
            (false, Some(output(Vec::new())), 0, Some("no_items"), "0"),
            (
                false,
                Some("I will remember that workflow.".into()),
                0,
                Some("refused"),
                "0",
            ),
            (
                false,
                Some(output(vec![proposal(CONTENT), json!({"content":42})])),
                1,
                Some("partial"),
                "1",
            ),
        ] {
            let fixture = Fixture::new(kind).await;
            let workspace = workspace("T006 ");
            let memory = fixture.memory(None);
            if host {
                // Exactly the same request bytes are intentionally selected by
                // a trusted typed operation, with no extractor/model call.
                let item = MemoryItem::new(RAW)
                    .with_type(MemoryType::Semantic)
                    .with_importance(0.91)
                    .with_tag("host-selected")
                    .with_metadata("authority", "explicit_host");
                let canonical = memory.remember_item(item.clone()).await.unwrap();
                assert_item(&canonical, &item);
                let rows = fixture.read_back().await;
                assert_eq!(rows.len(), count);
                assert_item(&rows[0], &canonical);
                assert_eq!(canonical.metadata.len(), 1);
                assert!(!canonical.metadata.contains_key("schema"));
                assert!(!canonical.tags.iter().any(|tag| tag == "extracted"));
            } else {
                let provider = client(text.unwrap());
                let agent = agent(memory, provider.clone(), workspace.path());
                let traces = TraceCapture::default();
                let _guard = traces.install();
                extract(&agent, RAW).await;
                assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
                assert!(provider.messages.lock().unwrap()[0][0].text().contains(RAW));
                let event = traces.last();
                field(&event, "disposition", disposition.unwrap());
                field(&event, "rejected_items", rejected);
                field(&event, "entered_store_calls", &count.to_string());
                assert_eq!(fixture.inputs().len(), count);
                assert_eq!(fixture.archives().len(), 0);
                let rows = fixture.read_back().await;
                assert_eq!(rows.len(), count);
                assert_eq!(
                    ids(&event),
                    rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>()
                );
                for row in &rows {
                    assert_distilled(row, CONTENT, workspace.path(), SESSION);
                    assert_ne!(row.content, RAW);
                    assert_item(row, &fixture.results()[0]);
                    assert_eq!(row.timestamp, fixture.inputs()[0].timestamp);
                }
            }
        }
    }
}

#[tokio::test]
async fn host_result_returns_actual_canonical_item_and_error_on_both_stores() {
    for kind in STORES {
        for reject in [false, true] {
            let fixture = Fixture::new(kind).await;
            let memory = fixture.memory(None);
            if reject {
                fixture.store.reject_primary.store(true, Ordering::SeqCst);
                let result = memory.remember_item(MemoryItem::new(RAW)).await;
                // An actual typed Result::Err, not an extractor item/string.
                assert!(result.is_err());
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "controlled primary failure"
                );
                assert_eq!(fixture.inputs().len(), 1);
                assert!(fixture.results().is_empty());
                assert!(fixture.read_back().await.is_empty());
                continue;
            }
            let original = fixture.seed(seeded("canonical-host", RAW)).await;
            let mut incoming = MemoryItem::new(RAW)
                .with_importance(0.9)
                .with_tag("host-selected")
                .with_metadata("authority", "explicit_host");
            incoming.id = "incoming-host".into();
            incoming.timestamp = original.timestamp - chrono::Duration::seconds(1);
            let before = Utc::now();
            let canonical = memory.remember_item(incoming.clone()).await.unwrap();
            let after = Utc::now();
            assert_ne!(canonical.id, incoming.id);
            assert_eq!(canonical.id, original.id);
            assert_eq!(canonical.timestamp, original.timestamp);
            assert_eq!(canonical.content, RAW);
            assert_eq!(canonical.importance, 0.9);
            assert_eq!(canonical.memory_type, MemoryType::Procedural);
            assert_eq!(canonical.tags, vec!["seed", "host-selected"]);
            let mut expected = original.metadata.clone();
            expected.insert("authority".into(), "explicit_host".into());
            expected.insert("duplicate_count".into(), "1".into());
            expected.insert("duplicate_ids".into(), incoming.id.clone());
            let last = canonical.metadata.get("last_duplicate_at").unwrap();
            let at = DateTime::parse_from_rfc3339(last)
                .unwrap()
                .with_timezone(&Utc);
            assert!(before <= at && at <= after);
            expected.insert("last_duplicate_at".into(), last.clone());
            assert_eq!(canonical.metadata, expected);
            assert_item(&fixture.results()[0], &canonical);
            let rows = fixture.read_back().await;
            assert_eq!(rows.len(), 1);
            assert_item(&rows[0], &canonical);
        }
    }
}

#[tokio::test]
async fn supersede_preserves_original_and_exact_replacement_on_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(kind).await;
        let old = fixture
            .seed(seeded(
                "superseded-original",
                "Use direct store writes when changing FileMemoryStore behavior.",
            ))
            .await;
        let mut replacement = proposal(CONTENT);
        replacement["supersedes"] = json!([old.id]);
        let provider = client(output(vec![replacement]));
        let workspace = workspace("T006 ");
        let memory = fixture.memory(None);
        let agent = agent(memory.clone(), provider.clone(), workspace.path());
        let traces = TraceCapture::default();
        let _guard = traces.install();
        extract(&agent, "Remember the workflow for FileMemoryStore changes; replace direct store writes with focused memory store tests.").await;
        let event = traces.last();
        field(&event, "disposition", "stored");
        field(&event, "entered_store_calls", "2");
        field(&event, "archive_acknowledged", "1");
        assert!(provider.messages.lock().unwrap()[0][0]
            .text()
            .contains(&old.id));
        let canonical = fixture.results().remove(0);
        assert_eq!(ids(&event), vec![canonical.id.clone()]);
        assert_ne!(canonical.id, old.id);
        assert_eq!(canonical.content, CONTENT);
        assert_eq!(canonical.timestamp, fixture.inputs()[0].timestamp);
        assert_eq!(canonical.importance, 0.85);
        assert_eq!(canonical.memory_type, MemoryType::Procedural);
        assert_eq!(
            canonical.tags,
            [
                "llm",
                "extracted",
                "workflow",
                "consolidated",
                "memory",
                "tests"
            ]
        );
        let mut metadata = expected_metadata(workspace.path(), SESSION);
        metadata.insert("supersedes".into(), old.id.clone());
        assert_eq!(canonical.metadata, metadata);
        let rows = fixture.read_back().await;
        assert_eq!(rows.len(), 2);
        assert_item(
            rows.iter().find(|row| row.id == canonical.id).unwrap(),
            &canonical,
        );
        let archived = rows.iter().find(|row| row.id == old.id).unwrap();
        let mut expected_old = old.clone();
        expected_old
            .metadata
            .insert("a3s.memory.status".into(), "superseded".into());
        expected_old
            .metadata
            .insert("superseded_by".into(), canonical.id.clone());
        expected_old
            .metadata
            .insert("protected".into(), "true".into());
        expected_old.tags.push("superseded".into());
        assert_item(archived, &expected_old);
        assert_item(&fixture.archives()[0], &expected_old);
        let recall = memory
            .recall_similar("FileMemoryStore behavior", 10)
            .await
            .unwrap();
        assert_eq!(
            recall.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![canonical.id.as_str()]
        );
        assert!(!rows.iter().any(|row| row.content == RAW));
    }
}

#[tokio::test]
async fn primary_and_archive_faults_keep_exact_partial_state_on_both_stores() {
    for kind in STORES {
        for archive_failure in [false, true] {
            let fixture = Fixture::new(kind).await;
            let old = fixture
                .seed(seeded(
                    "fault-original",
                    "Use direct store writes when changing FileMemoryStore behavior.",
                ))
                .await;
            fixture
                .store
                .reject_primary
                .store(!archive_failure, Ordering::SeqCst);
            fixture
                .store
                .reject_archive
                .store(archive_failure, Ordering::SeqCst);
            let mut replacement = proposal(CONTENT);
            replacement["supersedes"] = json!([old.id]);
            let provider = client(output(vec![replacement]));
            let workspace = workspace("T006 ");
            let agent = agent(fixture.memory(None), provider, workspace.path());
            let traces = TraceCapture::default();
            let _guard = traces.install();
            extract(&agent, RAW).await;
            let event = traces.last();
            let rows = fixture.read_back().await;
            assert_item(rows.iter().find(|row| row.id == old.id).unwrap(), &old);
            field(&event, "unproved_write", "true");
            field(&event, "archive_acknowledged", "0");
            assert_eq!(fixture.inputs().len(), 1);
            if archive_failure {
                field(&event, "disposition", "partial");
                field(&event, "entered_store_calls", "2");
                field(&event, "refusal", "relation_update_failed");
                assert_eq!(rows.len(), 2);
                let canonical = fixture.results().remove(0);
                assert_eq!(ids(&event), vec![canonical.id.clone()]);
                assert_item(
                    rows.iter().find(|row| row.id == canonical.id).unwrap(),
                    &canonical,
                );
                assert_eq!(canonical.content, CONTENT);
                assert_eq!(canonical.metadata.get("supersedes"), Some(&old.id));
                assert_eq!(fixture.archives().len(), 1);
                assert_eq!(
                    fixture.archives()[0].metadata.get("superseded_by"),
                    Some(&canonical.id)
                );
            } else {
                field(&event, "disposition", "unproved");
                field(&event, "entered_store_calls", "1");
                field(&event, "refusal", "store_failed");
                assert!(ids(&event).is_empty());
                assert_eq!(rows.len(), 1);
                assert!(fixture.archives().is_empty());
                assert!(fixture.results().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn conflict_filters_unprovided_ids_and_preserves_both_items_on_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(kind).await;
        let old = fixture
            .seed(seeded(
                "provided-conflict",
                "FileMemoryStore release checks use direct store writes.",
            ))
            .await;
        let unrelated = fixture
            .seed(seeded(
                "unprovided-existing",
                "Aurora catalog assigns sapphire ordinal seventeen.",
            ))
            .await;
        let mut candidate = proposal(CONTENT);
        candidate["conflicts_with"] = json!([old.id, unrelated.id, "absent-id", old.id]);
        candidate["supersedes"] = json!([unrelated.id, "absent-id"]);
        let provider = client(output(vec![candidate]));
        let workspace = workspace("T006 ");
        let agent = agent(fixture.memory(None), provider.clone(), workspace.path());
        let traces = TraceCapture::default();
        let _guard = traces.install();
        extract(&agent, RAW).await;
        let prompt = provider.messages.lock().unwrap()[0][0].text();
        assert!(prompt.contains(&old.id));
        assert!(!prompt.contains(&unrelated.id));
        let canonical = fixture.results().remove(0);
        assert_ne!(canonical.id, old.id);
        assert_eq!(canonical.content, CONTENT);
        assert_eq!(canonical.timestamp, fixture.inputs()[0].timestamp);
        assert_eq!(canonical.importance, 0.85);
        assert_eq!(
            canonical.tags,
            [
                "llm",
                "extracted",
                "workflow",
                "conflict",
                "memory",
                "tests"
            ]
        );
        let mut metadata = expected_metadata(workspace.path(), SESSION);
        metadata.insert("conflicts_with".into(), old.id.clone());
        assert_eq!(canonical.metadata, metadata);
        let event = traces.last();
        field(&event, "disposition", "stored");
        assert_eq!(ids(&event), vec![canonical.id.clone()]);
        assert!(fixture.archives().is_empty());
        let rows = fixture.read_back().await;
        assert_eq!(rows.len(), 3);
        assert_item(rows.iter().find(|row| row.id == old.id).unwrap(), &old);
        assert_item(
            rows.iter().find(|row| row.id == unrelated.id).unwrap(),
            &unrelated,
        );
        assert_item(
            rows.iter().find(|row| row.id == canonical.id).unwrap(),
            &canonical,
        );
        assert!(!rows
            .iter()
            .any(|row| row.content == RAW || row.id == "absent-id"));
    }
}

#[tokio::test]
async fn normalized_replays_keep_canonical_identity_time_and_metadata_on_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(kind).await;
        let original = fixture.seed(seeded("canonical-replay", CONTENT)).await;
        let workspace = workspace("T006 ");
        let memory = fixture.memory(None);
        let mut observation_ids = Vec::new();
        for (index, content) in [
            CONTENT,
            "  RUN   FOCUSED memory store tests after changing FileMemoryStore behavior.  ",
        ]
        .into_iter()
        .enumerate()
        {
            let provider = client(output(vec![proposal(content)]));
            let agent = agent(memory.clone(), provider, workspace.path());
            let traces = TraceCapture::default();
            let _guard = traces.install();
            let before = Utc::now();
            extract(&agent, RAW).await;
            let after = Utc::now();
            let canonical = fixture.results()[index].clone();
            assert_eq!(canonical.id, original.id);
            assert_eq!(canonical.timestamp, original.timestamp);
            assert_eq!(canonical.content, content.trim());
            assert_eq!(canonical.importance, 0.85);
            assert_eq!(canonical.memory_type, MemoryType::Procedural);
            let mut tags = expected_tags();
            tags.push("seed".into());
            assert_eq!(canonical.tags, tags);
            let mut metadata = expected_metadata(workspace.path(), SESSION);
            metadata.insert("host_note".into(), "retained".into());
            metadata.insert("duplicate_count".into(), (index + 1).to_string());
            let observed = canonical
                .metadata
                .get("last_observation_id")
                .unwrap()
                .clone();
            assert!(uuid::Uuid::parse_str(&observed).is_ok());
            assert_ne!(observed, canonical.id);
            assert!(!observation_ids.contains(&observed));
            observation_ids.push(observed.clone());
            metadata.insert("last_observation_id".into(), observed);
            metadata.insert("duplicate_ids".into(), observation_ids.join(","));
            let last = canonical.metadata.get("last_duplicate_at").unwrap();
            let at = DateTime::parse_from_rfc3339(last)
                .unwrap()
                .with_timezone(&Utc);
            assert!(before <= at && at <= after);
            metadata.insert("last_duplicate_at".into(), last.clone());
            assert_eq!(canonical.metadata, metadata);
            let event = traces.last();
            field(&event, "disposition", "stored");
            assert_eq!(ids(&event), vec![original.id.clone()]);
            assert_eq!(fixture.inputs()[index].id, original.id);
            assert!(fixture.archives().is_empty());
            let rows = fixture.read_back().await;
            assert_eq!(rows.len(), 1);
            assert_item(&rows[0], &canonical);
        }
    }
}

#[tokio::test]
async fn punctuation_distinct_items_do_not_merge_without_relation_on_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(kind).await;
        let punctuated = "Run focused memory store tests after changing FileMemoryStore behavior!";
        let provider = client(output(vec![proposal(CONTENT), proposal(punctuated)]));
        let workspace = workspace("T006 ");
        let agent = agent(fixture.memory(None), provider, workspace.path());
        let traces = TraceCapture::default();
        let _guard = traces.install();
        extract(&agent, RAW).await;
        let event = traces.last();
        field(&event, "disposition", "stored");
        field(&event, "accepted_items", "2");
        let canonical = fixture.results();
        assert_eq!(canonical.len(), 2);
        assert_ne!(canonical[0].id, canonical[1].id);
        assert_eq!(
            ids(&event),
            canonical
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>()
        );
        let rows = fixture.read_back().await;
        assert_eq!(rows.len(), 2);
        for (index, content) in [CONTENT, punctuated].into_iter().enumerate() {
            assert_distilled(&canonical[index], content, workspace.path(), SESSION);
            assert_eq!(
                canonical[index].timestamp,
                fixture.inputs()[index].timestamp
            );
            assert_item(
                rows.iter()
                    .find(|row| row.id == canonical[index].id)
                    .unwrap(),
                &canonical[index],
            );
        }
        assert!(fixture.archives().is_empty());
    }
}

#[tokio::test]
async fn actual_workspace_session_and_host_selected_store_override_model_fields_on_both_stores() {
    for kind in STORES {
        let chosen = Fixture::new(kind).await;
        let unchosen = Fixture::new(kind).await;
        let workspace = workspace("T006 Case Ω ");
        let mut candidate = proposal(CONTENT);
        candidate["scope"] = json!("repo");
        candidate["workspace"] = json!("/model/claimed/workspace");
        candidate["session_id"] = json!("model-session");
        candidate["store"] = json!("model-store");
        candidate["namespace"] = json!({"tenant_id":"model-tenant", "principal_id":"model-principal", "scope_id":"model-scope"});
        candidate["status"] = json!("active");
        let provider = client(output(vec![candidate]));
        let agent = agent(chosen.memory(None), provider, workspace.path());
        let traces = TraceCapture::default();
        let _guard = traces.install();
        extract(&agent, RAW).await;
        let rows = chosen.read_back().await;
        assert_eq!(rows.len(), 1);
        assert_distilled(&rows[0], CONTENT, workspace.path(), SESSION);
        assert_item(&rows[0], &chosen.results()[0]);
        assert_eq!(ids(&traces.last()), vec![rows[0].id.clone()]);
        assert_eq!(chosen.inputs().len(), 1);
        assert!(unchosen.inputs().is_empty());
        assert!(unchosen.read_back().await.is_empty());
        // V1 provenance metadata is not a tenant-isolation mechanism.
    }
}

#[tokio::test]
async fn v2_shadow_candidate_and_failure_preserve_v1_truth_on_both_stores() {
    for kind in STORES {
        for reject in [false, true] {
            let fixture = Fixture::new(kind).await;
            let repository = ShadowRepository::new(reject);
            let namespace =
                MemoryNamespace::try_new("host-tenant", "host-principal", "host-scope").unwrap();
            let foreign =
                MemoryNamespace::try_new("model-tenant", "model-principal", "model-scope").unwrap();
            let workspace = workspace("T006 ");
            let mut candidate = proposal(CONTENT);
            candidate["namespace"] = json!({"tenant_id":"model-tenant", "principal_id":"model-principal", "scope_id":"model-scope"});
            candidate["status"] = json!("active");
            let provider = client(output(vec![candidate]));
            let memory = fixture.memory(Some(repository.binding(namespace.clone())));
            let agent = agent(memory, provider, workspace.path());
            let traces = TraceCapture::default();
            let _guard = traces.install();
            extract(&agent, RAW).await;
            let event = traces.last();
            field(&event, "disposition", "stored");
            field(&event, "unproved_write", "false");
            assert!(!event.contains_key("refusal"));
            let rows = fixture.read_back().await;
            assert_eq!(rows.len(), 1);
            assert_distilled(&rows[0], CONTENT, workspace.path(), SESSION);
            assert_item(&rows[0], &fixture.results()[0]);
            assert_eq!(ids(&event), vec![rows[0].id.clone()]);
            let attempts = repository.attempts.lock().unwrap().clone();
            assert_eq!(attempts.len(), 1);
            assert_eq!(attempts[0].namespace, namespace);
            assert_eq!(
                attempts[0].occurred_at.timestamp_millis(),
                HOST_MILLIS as i64
            );
            assert_eq!(attempts[0].operations.len(), 1);
            let MemoryOperation::Create { node: draft } = &attempts[0].operations[0] else {
                panic!("extraction attempted an Active mutation");
            };
            assert_eq!(draft.namespace, namespace);
            assert_eq!(draft.status, MemoryStatus::Candidate);
            assert_eq!(draft.content, CONTENT);
            assert_eq!(draft.confidence, 0.94);
            assert_eq!(draft.importance, 0.85);
            assert_eq!(
                draft.labels.get("a3s.origin").map(String::as_str),
                Some("code.llm_extraction")
            );
            assert_eq!(
                draft
                    .labels
                    .get("a3s.extraction.source")
                    .map(String::as_str),
                Some("workflow")
            );
            assert_eq!(
                draft.labels.get("a3s.extraction.scope").map(String::as_str),
                Some("workspace")
            );
            assert_eq!(
                draft
                    .labels
                    .get("a3s.extraction.reason")
                    .map(String::as_str),
                Some(REASON)
            );
            assert_eq!(
                draft
                    .labels
                    .get("a3s.extraction.schema")
                    .map(String::as_str),
                Some("a3s.memory.durable.v1")
            );
            assert_eq!(
                serde_json::from_str::<Vec<String>>(
                    draft.labels.get("a3s.extraction.tags").unwrap()
                )
                .unwrap(),
                expected_tags()
            );
            assert_eq!(draft.evidence.len(), 1);
            assert_eq!(draft.evidence[0].kind, EvidenceKind::SessionTurn);
            assert_eq!(
                draft.evidence[0].uri,
                "a3s://session/T006%2DSession/turn/T006%2Dturn%2D0"
            );
            assert_eq!(
                draft.evidence[0].occurred_at.timestamp_millis(),
                HOST_MILLIS as i64
            );
            assert!(draft.evidence[0].digest.starts_with("sha256:"));
            assert_eq!(draft.evidence[0].digest.len(), 71);
            let candidates = repository
                .query(MemoryQuery::new(namespace.clone()).with_statuses([MemoryStatus::Candidate]))
                .await
                .unwrap();
            if reject {
                assert!(candidates.hits.is_empty());
            } else {
                assert_eq!(candidates.hits.len(), 1);
                let actual = repository
                    .get(&namespace, &draft.id)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(actual, candidates.hits[0].node);
                assert_eq!(actual.id, draft.id);
                assert_eq!(actual.content, CONTENT);
                assert_eq!(actual.status, MemoryStatus::Candidate);
                assert_eq!(actual.namespace, namespace);
                assert_eq!(actual.evidence, draft.evidence);
                assert_eq!(actual.labels, draft.labels);
                assert_eq!(actual.created_at, attempts[0].occurred_at);
            }
            assert!(repository
                .query(MemoryQuery::new(namespace))
                .await
                .unwrap()
                .hits
                .is_empty());
            assert!(repository
                .query(
                    MemoryQuery::new(foreign)
                        .with_statuses([MemoryStatus::Candidate, MemoryStatus::Active])
                )
                .await
                .unwrap()
                .hits
                .is_empty());
        }
    }
}
