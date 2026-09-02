use super::super::vec_shadow::ShadowVectorIndex;
use super::super::WorkspaceVecShadowPhase;
use a3s_memory::vector::{VectorIndex, VectorIndexDescriptor, VectorRecord, VectorSearchRequest};

#[tokio::test]
async fn vec_shadow_matches_memory_ranking_and_partition_filters() {
    let index = ShadowVectorIndex::new(VectorIndexDescriptor::new(3)).unwrap();
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
    let index = ShadowVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap();
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
    let index = ShadowVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap();
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
    let index = ShadowVectorIndex::new(VectorIndexDescriptor::new(dimension)).unwrap();
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
