//! S-CP-01: alternating cognitive generations do not replace a bound run.
//!
//! Host CAR qualification stays external. This soak proves the Code kernel:
//! a second generation conflicts, and the rejected digest is not retained.

use super::{cognitive_binding, InMemoryRunStore, RunCognitiveBindingError};

const CYCLES: usize = 20;
const NEXT_GENERATION_DIGEST: &str =
    "sha256:bb0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb21";

fn next_generation() -> crate::cognitive_context::CognitivePackageBindingV1 {
    crate::cognitive_context::package_binding_for_generation(
        "0.2.0",
        8,
        NEXT_GENERATION_DIGEST,
        "sha256:2def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a91",
    )
}

#[tokio::test]
#[ignore = "S-CP-01 soak: 20 alternating cognitive binds retain one generation"]
async fn soak_alternating_cognitive_binds_keep_one_generation() {
    let store = InMemoryRunStore::new();
    let generation_g = cognitive_binding();
    let generation_next = next_generation();
    assert_ne!(
        generation_g.generation_digest, generation_next.generation_digest,
        "the two generations must be distinct"
    );

    for cycle in 0..CYCLES {
        let run = store
            .create_run("cognitive-soak", &format!("cycle-{cycle}"))
            .await;
        let (kept, rejected) = if cycle % 2 == 0 {
            (&generation_g, &generation_next)
        } else {
            (&generation_next, &generation_g)
        };
        store
            .bind_cognitive_package(&run.id, kept.clone())
            .await
            .expect("the first generation binds");
        let error = store
            .bind_cognitive_package(&run.id, rejected.clone())
            .await
            .expect_err("a different generation must not replace the binding");
        assert!(
            matches!(error, RunCognitiveBindingError::Conflict),
            "cycle {cycle} did not fail closed: {error:?}"
        );
        let snapshot = store.snapshot(&run.id).await.expect("run remains");
        assert_eq!(snapshot.cognitive_package_binding.as_ref(), Some(kept));
        let retained = serde_json::to_string(&snapshot).expect("snapshot encodes");
        assert!(
            !retained.contains(&rejected.generation_digest),
            "cycle {cycle} retained the rejected generation"
        );
        assert!(
            !retained.contains(&rejected.capability_snapshot_digest),
            "cycle {cycle} retained the rejected capability snapshot"
        );
    }
}
