#![cfg(test)]

use super::*;

#[test]
fn agent_scope_stays_in_that_agents_folder() {
    assert!(hit_in_scope(
        "agents/legal-expert/合同.md",
        "agent",
        "legal-expert"
    ));
    assert!(!hit_in_scope(
        "agents/other/合同.md",
        "agent",
        "legal-expert"
    ));
    assert!(!hit_in_scope("local/合同.md", "agent", "legal-expert"));
    assert!(hit_in_scope("local/合同.md", "local", "legal-expert"));
    assert!(!hit_in_scope(
        "wiki/agents/legal-expert/合同.md",
        "local",
        "legal-expert"
    ));
    assert!(!agent_folder_ok("../legal"));
}

#[tokio::test]
async fn published_search_uses_the_same_generation_as_search_wiki() {
    let root = tempfile::tempdir().expect("wiki root");
    let wiki = root.path();
    std::fs::write(
        wiki.join("规范.md"),
        "---\ntitle: \"规范\"\nparser_generation: \"builtin:1\"\n---\n句子 0 在这一段\n",
    )
    .unwrap();
    let index = Arc::new(WikiIndex::open(wiki).expect("wiki index"));
    publish_index(&index);
    index.hold_publication(true);
    index.reconcile_now(None).await.expect("admit");
    std::fs::write(
        wiki.join("tail.md"),
        "---\ntitle: \"tail\"\nparser_generation: \"builtin:1\"\n---\n句子 32 在这一段\n",
    )
    .unwrap();
    let held = search_published(wiki, "句子 32 在这一段");
    let direct = index.search("句子 32 在这一段").expect("direct");
    assert_eq!(held.phase, WikiSearchPhase::NotReady);
    assert_eq!(direct.phase, WikiSearchPhase::NotReady);
    assert!(held.hits.is_empty());
    assert_eq!(held.source_revision, direct.source_revision);
    index.hold_publication(false);
    index.reconcile_now(None).await.expect("publish");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let shared = search_published(wiki, "句子 0 在这一段");
        let direct = index.search("句子 0 在这一段").expect("direct");
        assert_eq!(shared.phase, direct.phase);
        assert_eq!(shared.source_revision, direct.source_revision);
        if shared.phase == WikiSearchPhase::Published
            && shared
                .hits
                .iter()
                .any(|hit| hit.passage.contains("句子 0 在这一段"))
        {
            assert_eq!(
                shared
                    .hits
                    .iter()
                    .map(|hit| hit.passage.clone())
                    .collect::<Vec<_>>(),
                direct
                    .hits
                    .iter()
                    .map(|hit| hit.passage.clone())
                    .collect::<Vec<_>>()
            );
            let tail = search_published(wiki, "句子 32 在这一段");
            let tail_direct = index.search("句子 32 在这一段").expect("tail");
            assert_eq!(tail.phase, tail_direct.phase);
            assert_eq!(tail.source_revision, tail_direct.source_revision);
            assert_eq!(
                tail.hits
                    .iter()
                    .map(|hit| hit.passage.clone())
                    .collect::<Vec<_>>(),
                tail_direct
                    .hits
                    .iter()
                    .map(|hit| hit.passage.clone())
                    .collect::<Vec<_>>()
            );
            std::fs::write(
                wiki.join("stale.md"),
                "---\ntitle: \"stale\"\nparser_generation: \"builtin:0\"\n---\n旧代次句子不该命中\n",
            )
            .unwrap();
            index.reconcile_now(None).await.expect("stale");
            let stale_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
            loop {
                let stale_direct = index.search("旧代次句子不该命中").expect("stale direct");
                if stale_direct.phase == WikiSearchPhase::Published
                    && stale_direct
                        .hits
                        .iter()
                        .any(|hit| hit.passage.contains("旧代次句子不该命中"))
                {
                    let stale = search_published(wiki, "旧代次句子不该命中");
                    assert!(stale
                        .hits
                        .iter()
                        .all(|hit| !hit.passage.contains("旧代次句子不该命中")));
                    break;
                }
                if tokio::time::Instant::now() > stale_deadline {
                    panic!("stale parser generation was not indexed");
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("published generation did not match search_wiki");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[test]
fn team_passages_join_the_same_search_result() {
    let hit = HostHit {
        name: "报销".into(),
        place: "财务".into(),
        body: "差旅标准".into(),
    };
    let local = ToolOutput::success("note\npage\nlocal");
    let merged = merge(Some(local), Ok(vec![hit]));
    assert!(merged.success);
    assert!(merged.content.contains("local"));
    assert!(merged.content.contains("报销\n财务\n差旅标准"));
    let failed = merge(
        Some(ToolOutput::success("note\npage\nlocal")),
        Err("down".into()),
    );
    assert!(!failed.success);
    assert!(failed.content.contains("local"));
    assert!(failed
        .content
        .contains("organization encyclopedia search failed: down"));
    let only_failed = merge(None, Err("down".into()));
    assert!(!only_failed.success);
    assert!(only_failed
        .content
        .contains("organization encyclopedia search failed: down"));
}
