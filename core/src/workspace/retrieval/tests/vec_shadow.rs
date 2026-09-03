use super::super::vec_shadow::ShadowVectorIndex;
use super::super::vec_shadow_store::VecShadowStore;
use super::super::vector_contract::{
    VectorIndexDescriptor, VectorIndexError, VectorMutationConsistency, VectorRecord,
    VectorRevision, VectorSearchRequest, WorkspaceVectorIndex,
};
use super::super::WorkspaceVecShadowPhase;
use super::super::WorkspaceVectorEngine;

#[tokio::test]
async fn vec_shadow_matches_memory_ranking_and_partition_filters() {
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(3),
        WorkspaceVectorEngine::A3sMemory,
    )
    .unwrap();
    let tied = vec![1.0, 2.0, 3.0];
    index
        .replace_partition("src/a.rs", vec![VectorRecord::new("z", tied.clone())])
        .await
        .unwrap();
    index
        .replace_partition("src/aa.rs", vec![VectorRecord::new("a", tied.clone())])
        .await
        .unwrap();
    index
        .replace_partition(
            "src/weird'] || partition_key == ['escape.rs",
            vec![VectorRecord::new("quoted", vec![3.0, 2.0, 1.0])],
        )
        .await
        .unwrap();

    let all = index
        .search(VectorSearchRequest::new(tied.clone(), 3))
        .await
        .unwrap();
    assert_eq!(
        all.hits
            .iter()
            .map(|hit| (hit.partition.as_str(), hit.id.as_str()))
            .collect::<Vec<_>>(),
        [
            ("src/a.rs", "z"),
            ("src/aa.rs", "a"),
            ("src/weird'] || partition_key == ['escape.rs", "quoted"),
        ]
    );

    let filtered = index
        .search(
            VectorSearchRequest::new(tied, 2)
                .with_partition("src/weird'] || partition_key == ['escape.rs"),
        )
        .await
        .unwrap();
    assert_eq!(filtered.hits.len(), 1);
    assert_eq!(filtered.hits[0].id, "quoted");

    let shadow = index.shadow_status();
    assert_eq!(shadow.phase, WorkspaceVecShadowPhase::Ready);
    assert_eq!(shadow.record_count, 3);
    assert_eq!(shadow.compared_queries, 2);
    assert_eq!(shadow.matching_queries, 2);
    assert_eq!(shadow.mismatched_queries, 0);
    assert_eq!(shadow.failed_queries, 0);

    index.close().await;
    assert_eq!(index.shadow_status().phase, WorkspaceVecShadowPhase::Closed);
}

#[tokio::test]
async fn vec_shadow_tracks_replacement_removal_and_clear() {
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(2),
        WorkspaceVectorEngine::A3sMemory,
    )
    .unwrap();
    index
        .replace_partition(
            "src/lib.rs",
            vec![
                VectorRecord::new("old-a", vec![1.0, 0.0]),
                VectorRecord::new("old-b", vec![0.0, 1.0]),
            ],
        )
        .await
        .unwrap();
    index
        .replace_partition("src/lib.rs", vec![VectorRecord::new("new", vec![1.0, 1.0])])
        .await
        .unwrap();
    assert_eq!(index.shadow_status().record_count, 1);

    index.remove_partition("src/lib.rs").await.unwrap();
    assert_eq!(index.shadow_status().record_count, 0);
    index
        .replace_partition(
            "src/main.rs",
            vec![VectorRecord::new("main", vec![1.0, 1.0])],
        )
        .await
        .unwrap();
    index.clear().await.unwrap();

    let shadow = index.shadow_status();
    assert_eq!(shadow.phase, WorkspaceVecShadowPhase::Ready);
    assert_eq!(shadow.record_count, 0);
    assert!(shadow.successful_mutations >= 5);
    assert_eq!(shadow.failed_mutations, 0);
}

#[tokio::test]
async fn unsupported_shadow_labels_never_change_memory_results() {
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(2),
        WorkspaceVectorEngine::A3sMemory,
    )
    .unwrap();
    let record = VectorRecord::new("labelled", vec![1.0, 0.0]).with_label("scope", "workspace");
    index
        .replace_partition("src/lib.rs", vec![record])
        .await
        .unwrap();

    let result = index
        .search(VectorSearchRequest::new(vec![1.0, 0.0], 1).with_label("scope", "workspace"))
        .await
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].id, "labelled");

    let shadow = index.shadow_status();
    assert_eq!(shadow.phase, WorkspaceVecShadowPhase::Degraded);
    assert_eq!(shadow.failed_mutations, 1);
    assert_eq!(shadow.failed_queries, 1);
    assert_eq!(shadow.compared_queries, 0);
}

#[tokio::test]
async fn vec_shadow_matches_high_dimension_scores_bit_for_bit() {
    let dimension = 384;
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(dimension),
        WorkspaceVectorEngine::A3sMemory,
    )
    .unwrap();
    let first = (0..dimension)
        .map(|offset| ((offset % 17) as f32 - 8.0) / 9.0)
        .collect::<Vec<_>>();
    let second = (0..dimension)
        .map(|offset| ((offset % 13) as f32 - 6.0) / 7.0)
        .collect::<Vec<_>>();
    let query = (0..dimension)
        .map(|offset| ((offset % 11) as f32 - 5.0) / 6.0)
        .collect::<Vec<_>>();

    index
        .replace_partition(
            "src/high_dimension.rs",
            vec![
                VectorRecord::new("first", first),
                VectorRecord::new("second", second),
            ],
        )
        .await
        .unwrap();
    let result = index
        .search(VectorSearchRequest::new(query, 2))
        .await
        .unwrap();

    assert_eq!(result.hits.len(), 2);
    let shadow = index.shadow_status();
    assert_eq!(shadow.compared_queries, 1);
    assert_eq!(shadow.matching_queries, 1);
    assert_eq!(shadow.mismatched_queries, 0);
}

#[tokio::test]
async fn vec_primary_serves_and_keeps_memory_shadow_in_sync() {
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(2),
        WorkspaceVectorEngine::A3sVec,
    )
    .unwrap();
    assert_eq!(index.active_engine(), WorkspaceVectorEngine::A3sVec);

    index
        .replace_partition(
            "src/lib.rs",
            vec![
                VectorRecord::new("near", vec![1.0, 0.0]),
                VectorRecord::new("far", vec![0.0, 1.0]),
            ],
        )
        .await
        .unwrap();

    let result = index
        .search(VectorSearchRequest::new(vec![1.0, 0.0], 2))
        .await
        .unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.id.as_str())
            .collect::<Vec<_>>(),
        ["near", "far"]
    );
    let diagnostics = index.shadow_status();
    assert_eq!(diagnostics.phase, WorkspaceVecShadowPhase::Ready);
    assert_eq!(diagnostics.record_count, 2);
    assert_eq!(diagnostics.compared_queries, 1);
    assert_eq!(diagnostics.matching_queries, 1);
    assert_eq!(diagnostics.mismatched_queries, 0);

    index.clear().await.unwrap();
    index.close().await;
    assert_eq!(index.shadow_status().phase, WorkspaceVecShadowPhase::Closed);
}

#[tokio::test]
async fn vec_primary_close_releases_memory_shadow_without_a_prior_clear() {
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(2),
        WorkspaceVectorEngine::A3sVec,
    )
    .unwrap();
    index
        .replace_partition(
            "src/retained.rs",
            vec![VectorRecord::new("retained", vec![1.0, 0.0])],
        )
        .await
        .unwrap();

    index.close().await;
    let closed = index.shadow_status();
    assert_eq!(closed.phase, WorkspaceVecShadowPhase::Closed);
    assert_eq!(closed.record_count, 0);
    assert_eq!(closed.accounted_bytes, 0);
}

#[tokio::test]
async fn vec_primary_supports_revision_cas_and_rejects_stale_writers() {
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(2),
        WorkspaceVectorEngine::A3sVec,
    )
    .unwrap();
    assert_eq!(
        index.mutation_consistency(),
        VectorMutationConsistency::IndexRevisionCas
    );

    let first = index
        .replace_partition_if_revision(
            "src/lib.rs",
            VectorRevision::new(0),
            vec![VectorRecord::new("record", vec![1.0, 0.0])],
        )
        .await
        .expect("the initial revision-CAS write must succeed");
    assert_eq!(first.revision, VectorRevision::new(1));

    let error = index
        .replace_partition_if_revision(
            "src/lib.rs",
            VectorRevision::new(0),
            vec![VectorRecord::new("stale", vec![0.0, 1.0])],
        )
        .await
        .expect_err("a stale Vec writer must be rejected");
    assert_eq!(
        error,
        VectorIndexError::RevisionConflict {
            expected: VectorRevision::new(0),
            actual: VectorRevision::new(1),
        }
    );
    assert_eq!(index.status().record_count, 1);
    let result = index
        .search(VectorSearchRequest::new(vec![1.0, 0.0], 1))
        .await
        .expect("the committed record remains searchable");
    assert_eq!(result.hits[0].id, "record");
    let removed = index
        .remove_partition_if_revision("src/lib.rs", VectorRevision::new(1))
        .await
        .expect("the current revision may remove the partition");
    assert_eq!(removed.revision, VectorRevision::new(2));
    let stale_remove = index
        .remove_partition_if_revision("src/lib.rs", VectorRevision::new(1))
        .await
        .expect_err("a stale removal must be rejected even for an absent partition");
    assert_eq!(
        stale_remove,
        VectorIndexError::RevisionConflict {
            expected: VectorRevision::new(1),
            actual: VectorRevision::new(2),
        }
    );
    let diagnostics = index.shadow_status();
    assert_eq!(diagnostics.phase, WorkspaceVecShadowPhase::Ready);
    assert_eq!(diagnostics.failed_mutations, 0);
}

#[tokio::test]
async fn vec_primary_revision_cas_serializes_concurrent_writers() {
    use std::sync::Arc;

    let index = Arc::new(
        ShadowVectorIndex::new_with_engine(
            VectorIndexDescriptor::new(2),
            WorkspaceVectorEngine::A3sVec,
        )
        .unwrap(),
    );
    let first = index
        .replace_partition_if_revision(
            "src/concurrent.rs",
            VectorRevision::new(0),
            vec![VectorRecord::new("initial", vec![1.0, 0.0])],
        )
        .await
        .unwrap();
    assert_eq!(first.revision, VectorRevision::new(1));

    let left = Arc::clone(&index);
    let right = Arc::clone(&index);
    let (left, right) = tokio::join!(
        async move {
            left.replace_partition_if_revision(
                "src/concurrent.rs",
                VectorRevision::new(1),
                vec![VectorRecord::new("left", vec![1.0, 0.0])],
            )
            .await
        },
        async move {
            right
                .replace_partition_if_revision(
                    "src/concurrent.rs",
                    VectorRevision::new(1),
                    vec![VectorRecord::new("right", vec![0.0, 1.0])],
                )
                .await
        }
    );
    assert!(left.is_ok() ^ right.is_ok());
    let conflict = left
        .as_ref()
        .err()
        .or_else(|| right.as_ref().err())
        .expect("one concurrent writer must observe a revision conflict");
    assert!(matches!(
        conflict,
        VectorIndexError::RevisionConflict { .. }
    ));

    let result = index
        .search(VectorSearchRequest::new(vec![1.0, 0.0], 1))
        .await
        .unwrap();
    assert!(matches!(result.hits[0].id.as_str(), "left" | "right"));
    assert_eq!(result.status.revision, VectorRevision::new(2));
    index.close().await;
}

#[tokio::test]
async fn vec_primary_rejects_invalid_contracts_before_mutating_the_collection() {
    let index = ShadowVectorIndex::new_with_engine(
        VectorIndexDescriptor::new(2),
        WorkspaceVectorEngine::A3sVec,
    )
    .unwrap();

    let dimension_error = index
        .replace_partition("src/lib.rs", vec![VectorRecord::new("bad", vec![1.0])])
        .await
        .expect_err("dimension mismatches must fail before Vec admission");
    assert!(matches!(
        dimension_error,
        VectorIndexError::DimensionMismatch { .. }
    ));
    assert_eq!(index.status().record_count, 0);

    let query_error = index
        .search(VectorSearchRequest::new(vec![0.0, 0.0], 1))
        .await
        .expect_err("zero query vectors must fail before a Vec query");
    assert!(matches!(query_error, VectorIndexError::ZeroVector { .. }));
    assert_eq!(index.shadow_status().failed_mutations, 1);
    assert_eq!(index.shadow_status().failed_queries, 1);
}

#[tokio::test]
async fn vec_partition_replacement_rolls_back_after_resource_rejection() {
    let descriptor = VectorIndexDescriptor::new(2).with_max_bytes(1_024);
    let (store, initial) = VecShadowStore::create(&descriptor).unwrap();
    assert_eq!(initial.record_count, 0);
    let first = store
        .replace_partition(
            "src/lib.rs".to_owned(),
            vec![VectorRecord::new("old", vec![1.0, 0.0])],
        )
        .await
        .unwrap();
    assert_eq!(first.record_count, 1);

    let oversized = store
        .replace_partition(
            "src/lib.rs".to_owned(),
            vec![VectorRecord::new("x".repeat(4_096), vec![0.0, 1.0])],
        )
        .await
        .expect_err("the Vec byte policy must reject an oversized replacement");
    assert_eq!(oversized.code(), "vec_resource_exhausted");

    let result = store
        .search(VectorSearchRequest::new(vec![1.0, 0.0], 1))
        .await
        .unwrap();
    assert_eq!(result.snapshot.record_count, 1);
    assert_eq!(result.hits[0].id, "old");
    store.close().await.unwrap();
}
