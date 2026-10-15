use super::memory::{distinctive_usage_token, record_memory_usage};
use super::render_env_block;

#[test]
fn env_block_contains_grounding_facts() {
    let block = render_env_block(std::path::Path::new("/tmp/demo-ws"));
    assert!(block.starts_with("<env>"), "block: {block}");
    assert!(block.trim_end().ends_with("</env>"));
    assert!(block.contains("Working directory: /tmp/demo-ws"));
    assert!(block.contains("Platform:"));
    assert!(block.contains(std::env::consts::OS));
    assert!(block.contains("Today's date:"));
}

#[test]
fn env_block_date_is_iso_yyyy_mm_dd() {
    let block = render_env_block(std::path::Path::new("/tmp"));
    let line = block
        .lines()
        .find(|l| l.starts_with("Today's date:"))
        .expect("date line present");
    let date = line.trim_start_matches("Today's date:").trim();
    assert_eq!(date.len(), 10, "date not YYYY-MM-DD: {date}");
    assert_eq!(date.matches('-').count(), 2, "date not YYYY-MM-DD: {date}");
    assert!(date.chars().all(|c| c.is_ascii_digit() || c == '-'));
}

#[test]
fn distinctive_usage_token_picks_the_longest_plain_word() {
    assert_eq!(
        distinctive_usage_token("Prefer the Zephyrquotient metric when ranking"),
        Some("zephyrquotient".to_string())
    );
    // Words with punctuation inside are not purely alphanumeric.
    assert_eq!(distinctive_usage_token("pin at 1.97.1 only"), None);
}

#[tokio::test]
async fn recalled_memory_usage_counts_a_distinctive_token_once() {
    let store = a3s_memory::InMemoryStore::new();
    let content = "Prefer the Zephyrquotient metric when ranking providers";
    let item = a3s_memory::MemoryItem::new(content);
    let id = item.id.clone();
    a3s_memory::MemoryStore::store(&store, item).await.unwrap();
    let recalled = vec![(id.clone(), content.to_string(), 1.0)];

    record_memory_usage(
        &store as &dyn a3s_memory::MemoryStore,
        &recalled,
        "Ranking done with the zephyrquotient metric as asked.",
    )
    .await;
    let used = a3s_memory::MemoryStore::retrieve(&store, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        used.metadata.get("usage_count").map(String::as_str),
        Some("1")
    );

    // The same id repeated in one call counts once, and a final text without
    // the token adds nothing further.
    let repeated: Vec<(String, String, f32)> = recalled.clone();
    record_memory_usage(&store, &repeated, "No related vocabulary appeared here.").await;
    let still_once = a3s_memory::MemoryStore::retrieve(&store, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        still_once.metadata.get("usage_count").map(String::as_str),
        Some("1")
    );
}

#[tokio::test]
async fn recalled_memory_usage_skips_unmatched_and_unresolvable_ids() {
    let store = a3s_memory::InMemoryStore::new();

    // A token absent from the final text records nothing.
    let unmatched = a3s_memory::MemoryItem::new("Prefer the Zephyrquotient metric when ranking");
    let unmatched_id = unmatched.id.clone();
    a3s_memory::MemoryStore::store(&store, unmatched)
        .await
        .unwrap();
    record_memory_usage(
        &store as &dyn a3s_memory::MemoryStore,
        &[(
            unmatched_id.clone(),
            "Prefer the Zephyrquotient metric".to_string(),
            0.5,
        )],
        "An unrelated reply about toolchains.",
    )
    .await;
    let untouched = a3s_memory::MemoryStore::retrieve(&store, &unmatched_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!untouched.metadata.contains_key("usage_count"));

    // Content without any qualifying word cannot produce a signal.
    record_memory_usage(
        &store,
        &[(unmatched_id, "ok done".to_string(), 0.5)],
        "ok done",
    )
    .await;

    // Durable node ids and ledger digests have no V1 store item; they are
    // skipped silently instead of failing the recorder.
    record_memory_usage(
        &store as &dyn a3s_memory::MemoryStore,
        &[(
            "durable-node-42".to_string(),
            "A durable node mentioning Zephyrquotient".to_string(),
            0.5,
        )],
        "The reply says zephyrquotient out loud.",
    )
    .await;
    assert_eq!(a3s_memory::MemoryStore::count(&store).await.unwrap(), 1);
}

/// A deterministic bucketing provider: any text naming the sentinel
/// animal embeds to axis A, everything else to axis B. Cosine similarity
/// is then 1.0 within a bucket and 0.0 across — a fully deterministic
/// semantic judge for pins.
struct BucketProvider;

#[async_trait::async_trait]
impl crate::embedding::EmbeddingProvider for BucketProvider {
    fn descriptor(&self) -> crate::embedding::EmbeddingProviderDescriptor {
        crate::embedding::EmbeddingProviderDescriptor {
            provider: "bucket".to_string(),
            model: "bucket-2d".to_string(),
            revision: None,
            dimension: 2,
            normalization: crate::embedding::EmbeddingNormalization::None,
        }
    }

    async fn embed(
        &self,
        request: crate::embedding::EmbeddingBatchRequest,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<crate::embedding::EmbeddingBatchResponse, crate::embedding::EmbeddingProviderError>
    {
        let vectors = request
            .inputs()
            .iter()
            .map(|input| {
                let axis = if input.text().contains("quokka") {
                    0
                } else {
                    1
                };
                let values = if axis == 0 {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                };
                crate::embedding::EmbeddingVector::new(input.id(), values)
            })
            .collect();
        Ok(crate::embedding::EmbeddingBatchResponse::new(
            self.descriptor(),
            vectors,
        ))
    }
}

#[tokio::test]
async fn semantic_usage_judge_counts_a_paraphrase_the_lexical_arm_missed() {
    let store = a3s_memory::InMemoryStore::new();
    // No word of eight-plus characters exists here, so the lexical arm
    // cannot decide; the reply paraphrases without reusing any long token.
    let content = "The quokka colony sleeps at noon";
    let item = a3s_memory::MemoryItem::new(content);
    let id = item.id.clone();
    a3s_memory::MemoryStore::store(&store, item).await.unwrap();
    let judge = crate::memory::SemanticUsageJudge::new(
        std::sync::Arc::new(BucketProvider),
        crate::memory::SEMANTIC_USAGE_DEFAULT_THRESHOLD,
    )
    .unwrap();

    super::memory::record_memory_usage_with_semantics(
        &store as &dyn a3s_memory::MemoryStore,
        &[(id.clone(), content.to_string(), 1.0)],
        "The little quokka group rested through the midday heat.",
        Some(&judge),
    )
    .await;
    let used = a3s_memory::MemoryStore::retrieve(&store, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        used.metadata.get("usage_count").map(String::as_str),
        Some("1"),
        "a same-bucket paraphrase counts as use"
    );
}

#[tokio::test]
async fn semantic_usage_judge_skips_different_bucket_and_errors() {
    let store = a3s_memory::InMemoryStore::new();
    let off_topic = "toolchain pins rust at the workspace root";
    let item = a3s_memory::MemoryItem::new(off_topic);
    let off_id = item.id.clone();
    a3s_memory::MemoryStore::store(&store, item).await.unwrap();
    let judge = crate::memory::SemanticUsageJudge::new(
        std::sync::Arc::new(BucketProvider),
        crate::memory::SEMANTIC_USAGE_DEFAULT_THRESHOLD,
    )
    .unwrap();

    super::memory::record_memory_usage_with_semantics(
        &store as &dyn a3s_memory::MemoryStore,
        &[(off_id.clone(), off_topic.to_string(), 0.5)],
        "The little quokka group rested through the midday heat.",
        Some(&judge),
    )
    .await;
    let untouched = a3s_memory::MemoryStore::retrieve(&store, &off_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !untouched.metadata.contains_key("usage_count"),
        "a different-bucket memory is not used"
    );

    // A failing provider decides nothing: telemetry degrades to the
    // lexical arm, never to a wrong bump.
    struct FailingProvider;
    #[async_trait::async_trait]
    impl crate::embedding::EmbeddingProvider for FailingProvider {
        fn descriptor(&self) -> crate::embedding::EmbeddingProviderDescriptor {
            crate::embedding::EmbeddingProviderDescriptor {
                provider: "failing".to_string(),
                model: "failing".to_string(),
                revision: None,
                dimension: 2,
                normalization: crate::embedding::EmbeddingNormalization::None,
            }
        }
        async fn embed(
            &self,
            _request: crate::embedding::EmbeddingBatchRequest,
            _cancellation: tokio_util::sync::CancellationToken,
        ) -> Result<
            crate::embedding::EmbeddingBatchResponse,
            crate::embedding::EmbeddingProviderError,
        > {
            Err(crate::embedding::EmbeddingProviderError::Unavailable { retry_after: None })
        }
    }
    let failing = crate::memory::SemanticUsageJudge::new(
        std::sync::Arc::new(FailingProvider),
        crate::memory::SEMANTIC_USAGE_DEFAULT_THRESHOLD,
    )
    .unwrap();
    super::memory::record_memory_usage_with_semantics(
        &store as &dyn a3s_memory::MemoryStore,
        &[(off_id.clone(), off_topic.to_string(), 0.5)],
        "nothing matches lexically here",
        Some(&failing),
    )
    .await;
    let still = a3s_memory::MemoryStore::retrieve(&store, &off_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !still.metadata.contains_key("usage_count"),
        "an embedding error must not bump usage"
    );
}

#[test]
fn semantic_usage_threshold_validates_fail_closed() {
    for bad in [f32::NAN, 1.5, -1.5] {
        assert!(
            crate::memory::SemanticUsageJudge::new(std::sync::Arc::new(BucketProvider), bad)
                .is_err(),
            "threshold {bad} must refuse construction"
        );
    }
}
