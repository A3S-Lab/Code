#![cfg(test)]

use super::*;

struct CacheCoverageFixture {
    workspace: tempfile::TempDir,
    session: AgentSession,
    pinned: PinnedFact,
    checkpoint: LoopCheckpoint,
    log: FileLog,
    thread: String,
    write: ToolPreparedV1,
    written: ToolExecutionResultV1,
    verify: ToolPreparedV1,
    verified: ToolExecutionResultV1,
}

impl CacheCoverageFixture {
    async fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let session = session(workspace.path()).await;
        let pinned = prepare_pinned(&session, "resume byte evidence", false)
            .await
            .unwrap();
        let mut checkpoint = checkpoint(&session);
        checkpoint.convergence.mutations = Default::default();
        checkpoint.capability_binding = pinned
            .parts
            .surface
            .as_ref()
            .unwrap()
            .agent
            .checkpoint_capability_binding()
            .cloned();
        let thread = crate::fact_control::thread_for_session(session.session_id());
        let binding = crate::content_digest::digest_json(
            "a3s.code.tool-admission-binding.v1",
            checkpoint.capability_binding.as_ref().unwrap(),
        )
        .unwrap();
        let mut write = prepared("cached-write-A");
        write.identity.admission = Some(a3s_effect::ToolAdmissionIdentityV1 {
            run_id: checkpoint.run_id.clone(),
            thread_id: thread.clone(),
            capability_binding_digest: binding,
        });
        std::fs::write(workspace.path().join("cached.txt"), b"actual cached bytes").unwrap();
        let effects = crate::porcelain::observe_path_effects(
            workspace.path(),
            crate::porcelain::PathDelta {
                paths: vec!["cached.txt".into()],
                incomplete: false,
            },
        );
        let written = ToolExecutionResultV1 {
            schema: TOOL_RESULT_SCHEMA.into(),
            identity: write.identity.clone(),
            prepared_digest: write.digest().unwrap(),
            outcome: ToolExecutionOutcome::Succeeded,
            exit_code: Some(0),
            output: json!("observed actual file bytes"),
            effects,
        };
        checkpoint
            .convergence
            .mutations
            .observe_tool_receipt(&write.tool_name, &written);
        assert_eq!(
            std::fs::read(workspace.path().join("cached.txt")).unwrap(),
            b"actual cached bytes"
        );
        let report = crate::verification::VerificationReport::new(
            "actual cached file byte check",
            vec![crate::verification::VerificationCheck::required(
                "exact-byte-read",
                "file",
                "the test read the exact bytes written to cached.txt",
            )
            .with_status(crate::verification::VerificationStatus::Passed)],
        )
        .with_effect_digest(checkpoint.convergence.mutations.digest());
        let mut verify = write.clone();
        verify.identity.tool_call_id = "cached-verifier-V".into();
        let verified = ToolExecutionResultV1 {
            schema: TOOL_RESULT_SCHEMA.into(),
            identity: verify.identity.clone(),
            prepared_digest: verify.digest().unwrap(),
            outcome: ToolExecutionOutcome::Succeeded,
            exit_code: Some(0),
            output: json!(json!({
                "kind":"a3s.fact_tool_result", "text":"actual byte check passed",
                "is_error":false, "exit_code":0, "outcome":"succeeded",
                "metadata":{"verification_report":report,"verification_author":"host"}
            })
            .to_string()),
            effects: ToolEffectObservationV1::complete(Vec::new()),
        };
        checkpoint
            .convergence
            .mutations
            .observe_tool_receipt(&verify.tool_name, &verified);
        checkpoint.verification_reports.push(report);
        let log = FileLog::open(crate::session_folder::effect_log_dir(
            workspace.path(),
            session.session_id(),
        ))
        .unwrap();
        Self {
            workspace,
            session,
            pinned,
            checkpoint,
            log,
            thread,
            write,
            written,
            verify,
            verified,
        }
    }

    fn pair(prepared: &ToolPreparedV1, result: &ToolExecutionResultV1) -> Vec<NewFact> {
        vec![
            fact(
                "tool.prepared",
                &format!("prepared:{}", prepared.identity.tool_call_id),
                json!({"prepared":prepared}),
            ),
            fact(
                "tool.result",
                &format!("result:{}", prepared.identity.tool_call_id),
                json!({"receipt":result}),
            ),
        ]
    }

    fn original(&self) -> Vec<NewFact> {
        let mut facts = Self::pair(&self.write, &self.written);
        facts.extend(Self::pair(&self.verify, &self.verified));
        facts
    }

    fn append(&self, facts: &[NewFact]) {
        self.log.append(&self.thread, facts, None).unwrap();
    }

    fn gate(&self) -> crate::harness_loop::CompletionGate {
        let surface = self.pinned.parts.surface.as_ref().unwrap();
        crate::harness_loop::decide_completion(
            &surface.ledger.lock().unwrap(),
            &surface.reports.lock().unwrap(),
            &[],
            false,
        )
    }

    fn seed(&mut self) {
        seed_resumed_mutation_state(&mut self.pinned, &self.checkpoint).unwrap();
        let negative = self
            .pinned
            .parts
            .open()
            .unwrap()
            .read_facts()
            .unwrap()
            .into_iter()
            .filter(|fact| fact.kind == "recovery.unproved")
            .collect::<Vec<_>>();
        if unresolved(&self.pinned).contains(&"tool_effect_cache_evidence_missing".into()) {
            assert_eq!(
                negative.len(),
                1,
                "a coverage gap must commit one negative input"
            );
            assert_eq!(negative[0].payload["source_run_id"], self.checkpoint.run_id);
            assert!(negative[0].cause.is_none());
        } else {
            assert!(
                negative.is_empty(),
                "exact original proof needs no negative input"
            );
        }
    }
}

#[tokio::test]
async fn exact_original_write_and_verifier_receipts_cover_a_passed_cache() {
    let mut fixture = CacheCoverageFixture::new().await;
    fixture.append(&fixture.original());
    fixture.seed();
    assert!(!unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Verified { .. }
        )
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn unrelated_prepared_debt_does_not_invalidate_exact_cache_provenance() {
    let mut fixture = CacheCoverageFixture::new().await;
    let mut facts = fixture.original();
    let mut unrelated = fixture.write.clone();
    unrelated.identity.tool_call_id = "unrelated-prepared-C".into();
    facts.push(fact(
        "tool.prepared",
        "prepared:unrelated-prepared-C",
        json!({"prepared":unrelated}),
    ));
    fixture.append(&facts);
    fixture.seed();
    let debt = unresolved(&fixture.pinned);
    assert!(!debt.contains(&"tool_effect_cache_evidence_missing".into()));
    assert!(debt.contains(&unrelated.identity.key().unwrap()));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn exact_original_unknown_receipt_covers_its_cache_without_forgiving_debt() {
    let mut fixture = CacheCoverageFixture::new().await;
    fixture.written.outcome = ToolExecutionOutcome::Unknown;
    fixture.written.exit_code = None;
    fixture.checkpoint.convergence.mutations = Default::default();
    fixture.checkpoint.verification_reports.clear();
    fixture
        .checkpoint
        .convergence
        .mutations
        .observe_tool_receipt(&fixture.write.tool_name, &fixture.written);
    fixture.append(&CacheCoverageFixture::pair(
        &fixture.write,
        &fixture.written,
    ));
    fixture.seed();
    let debt = unresolved(&fixture.pinned);
    assert!(!debt.contains(&"tool_effect_cache_evidence_missing".into()));
    assert!(debt.contains(&fixture.write.identity.key().unwrap()));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn cached_prepared_debt_waits_for_its_actual_later_original_boundary() {
    let mut fixture = CacheCoverageFixture::new().await;
    let mut facts = fixture.original();
    let mut pending = fixture.write.clone();
    pending.identity.tool_call_id = "cached-pending-C-after-verifier".into();
    let key = pending.identity.key().unwrap();
    fixture
        .checkpoint
        .convergence
        .mutations
        .restore_unresolved_tool_effects(
            std::collections::BTreeSet::from([key.clone()]),
            &std::collections::BTreeSet::new(),
        );
    facts.push(fact(
        "tool.prepared",
        "prepared:cached-pending-C-after-verifier",
        json!({"prepared":pending}),
    ));
    fixture.append(&facts);
    fixture.seed();
    let debt = unresolved(&fixture.pinned);
    assert!(!debt.contains(&"tool_effect_cache_evidence_missing".into()));
    assert!(debt.contains(&key));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn a_future_terminal_does_not_erase_prepared_debt_at_the_exact_cached_prefix() {
    let mut fixture = CacheCoverageFixture::new().await;
    let mut pending = fixture.write.clone();
    pending.identity.tool_call_id = "pending-C-terminal-after-cache".into();
    let key = pending.identity.key().unwrap();
    fixture
        .checkpoint
        .convergence
        .mutations
        .restore_unresolved_tool_effects(
            std::collections::BTreeSet::from([key.clone()]),
            &std::collections::BTreeSet::new(),
        );
    let mut facts = vec![fact(
        "tool.prepared",
        "prepared:pending-C-terminal-after-cache",
        json!({"prepared":pending}),
    )];
    facts.extend(fixture.original());
    std::fs::write(
        fixture.workspace.path().join("cached.txt"),
        b"actual later C bytes",
    )
    .unwrap();
    let mut terminal = fixture.written.clone();
    terminal.identity = pending.identity.clone();
    terminal.prepared_digest = pending.digest().unwrap();
    terminal.effects = crate::porcelain::observe_path_effects(
        fixture.workspace.path(),
        crate::porcelain::PathDelta {
            paths: vec!["cached.txt".into()],
            incomplete: false,
        },
    );
    facts.push(fact(
        "tool.result",
        "result:pending-C-terminal-after-cache",
        json!({"receipt":terminal}),
    ));
    fixture.append(&facts);
    fixture.seed();
    let debt = unresolved(&fixture.pinned);
    assert!(!debt.contains(&"tool_effect_cache_evidence_missing".into()));
    assert!(debt.contains(&key));
    assert!(fixture
        .pinned
        .parts
        .surface
        .as_ref()
        .unwrap()
        .ledger
        .lock()
        .unwrap()
        .content_digest_matches("cached.txt", &sha256::digest(b"actual later C bytes")));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn exact_prepared_only_debt_proves_its_negative_cache_without_a_terminal_receipt() {
    let mut fixture = CacheCoverageFixture::new().await;
    let key = fixture.write.identity.key().unwrap();
    fixture.checkpoint.convergence.mutations = Default::default();
    fixture.checkpoint.verification_reports.clear();
    fixture
        .checkpoint
        .convergence
        .mutations
        .restore_unresolved_tool_effects(
            std::collections::BTreeSet::from([key.clone()]),
            &std::collections::BTreeSet::new(),
        );
    fixture.append(&[fact(
        "tool.prepared",
        "prepared:negative-only-cache",
        json!({"prepared":fixture.write}),
    )]);
    fixture.seed();
    assert_eq!(unresolved(&fixture.pinned), vec![key]);
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn valid_older_cache_is_covered_before_a_later_known_write() {
    let mut fixture = CacheCoverageFixture::new().await;
    let mut facts = fixture.original();
    let mut later = fixture.write.clone();
    later.identity.tool_call_id = "later-known-write".into();
    std::fs::write(
        fixture.workspace.path().join("cached.txt"),
        b"later original bytes",
    )
    .unwrap();
    let mut result = fixture.written.clone();
    result.identity = later.identity.clone();
    result.prepared_digest = later.digest().unwrap();
    result.effects = crate::porcelain::observe_path_effects(
        fixture.workspace.path(),
        crate::porcelain::PathDelta {
            paths: vec!["cached.txt".into()],
            incomplete: false,
        },
    );
    facts.extend(CacheCoverageFixture::pair(&later, &result));
    fixture.append(&facts);
    fixture.seed();
    assert!(!unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()));
    let surface = fixture.pinned.parts.surface.as_ref().unwrap();
    assert!(surface
        .ledger
        .lock()
        .unwrap()
        .content_digest_matches("cached.txt", &sha256::digest(b"later original bytes")));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn prior_run_settled_receipts_keep_their_own_frozen_binding_in_cache_coverage() {
    let mut fixture = CacheCoverageFixture::new().await;
    let prior = fixture.write.identity.admission.as_mut().unwrap();
    prior.run_id = "prior-run".into();
    prior.capability_binding_digest = format!("sha256:{}", "b".repeat(64));
    fixture.written.identity = fixture.write.identity.clone();
    fixture.written.prepared_digest = fixture.write.digest().unwrap();
    fixture.checkpoint.convergence.mutations = Default::default();
    fixture
        .checkpoint
        .convergence
        .mutations
        .observe_tool_receipt(&fixture.write.tool_name, &fixture.written);
    fixture
        .checkpoint
        .convergence
        .mutations
        .observe_tool_receipt(&fixture.verify.tool_name, &fixture.verified);
    let mut facts = CacheCoverageFixture::pair(&fixture.write, &fixture.written);
    facts.push(fact(
        "turn.complete",
        "prior-run-settled",
        json!({"receipt_keys":[fixture.write.identity.key().unwrap()]}),
    ));
    facts.extend(CacheCoverageFixture::pair(
        &fixture.verify,
        &fixture.verified,
    ));
    fixture.append(&facts);
    fixture.seed();
    assert!(!unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Verified { .. }
        )
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn prior_settled_path_does_not_rebind_a_new_runs_distinct_effect_report() {
    let mut fixture = CacheCoverageFixture::new().await;
    std::fs::write(
        fixture.workspace.path().join("prior.txt"),
        b"prior settled bytes",
    )
    .unwrap();
    let mut prior = fixture.write.clone();
    prior.identity.tool_call_id = "prior-settled-write".into();
    let admission = prior.identity.admission.as_mut().unwrap();
    admission.run_id = "prior-run".into();
    admission.capability_binding_digest = format!("sha256:{}", "d".repeat(64));
    let mut result = fixture.written.clone();
    result.identity = prior.identity.clone();
    result.prepared_digest = prior.digest().unwrap();
    result.effects = crate::porcelain::observe_path_effects(
        fixture.workspace.path(),
        crate::porcelain::PathDelta {
            paths: vec!["prior.txt".into()],
            incomplete: false,
        },
    );
    let mut facts = CacheCoverageFixture::pair(&prior, &result);
    facts.push(fact(
        "turn.complete",
        "settled-prior-distinct-path",
        json!({"receipt_keys":[prior.identity.key().unwrap()]}),
    ));
    facts.extend(fixture.original());
    fixture.append(&facts);
    fixture.seed();
    assert!(!unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()));
    let surface = fixture.pinned.parts.surface.as_ref().unwrap();
    assert_eq!(
        surface.ledger.lock().unwrap().paths().collect::<Vec<_>>(),
        ["cached.txt"]
    );
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Verified { .. }
        )
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn incomplete_or_mismatched_original_evidence_cannot_promote_a_passed_cache() {
    for scenario in [
        "unrelated-read",
        "partial-prefix",
        "wrong-run",
        "wrong-thread",
        "wrong-binding",
        "wrong-bytes",
        "missing-preparation",
        "missing-result",
        "earlier-settled",
        "malformed-result",
        "invented-cache-uncertainty",
    ] {
        let mut fixture = CacheCoverageFixture::new().await;
        let mut facts = fixture.original();
        match scenario {
            "unrelated-read" | "earlier-settled" => {
                let mut read = fixture.verify.clone();
                read.identity.tool_call_id = "unrelated-read-B".into();
                let mut result = fixture.verified.clone();
                result.identity = read.identity.clone();
                result.prepared_digest = read.digest().unwrap();
                result.output = json!("unrelated read, no verifier evidence");
                facts = CacheCoverageFixture::pair(&read, &result);
                if scenario == "earlier-settled" {
                    let mut earlier = fixture.write.clone();
                    earlier.identity.tool_call_id = "earlier-settled-C".into();
                    let mut result = fixture.written.clone();
                    result.identity = earlier.identity.clone();
                    result.prepared_digest = earlier.digest().unwrap();
                    result.effects.changes[0].path = "other-earlier.txt".into();
                    let mut prefix = CacheCoverageFixture::pair(&earlier, &result);
                    prefix.push(fact(
                        "turn.complete",
                        "unrelated-settled-marker",
                        json!({"receipt_keys":[earlier.identity.key().unwrap()]}),
                    ));
                    prefix.extend(facts);
                    facts = prefix;
                }
            }
            "partial-prefix" => facts.drain(..2).for_each(drop),
            "wrong-run" | "wrong-thread" | "wrong-binding" => {
                let mut original = fixture.write.clone();
                let admission = original.identity.admission.as_mut().unwrap();
                match scenario {
                    "wrong-run" => admission.run_id = "different-run".into(),
                    "wrong-thread" => admission.thread_id = "different-thread".into(),
                    _ => admission.capability_binding_digest = format!("sha256:{}", "c".repeat(64)),
                }
                let mut result = fixture.written.clone();
                result.identity = original.identity.clone();
                result.prepared_digest = original.digest().unwrap();
                facts
                    .splice(..2, CacheCoverageFixture::pair(&original, &result))
                    .for_each(drop);
            }
            "wrong-bytes" => {
                let mut result = fixture.written.clone();
                result.effects.changes[0].after.content_digest = Some(format!(
                    "sha256:{}",
                    sha256::digest(b"different original bytes")
                ));
                facts[1].payload = json!({"receipt":result});
            }
            "missing-preparation" => {
                facts.remove(0);
            }
            "missing-result" => {
                facts.remove(1);
            }
            "malformed-result" => facts[1].payload["receipt"]["schema"] = json!("unknown.v99"),
            "invented-cache-uncertainty" => {
                fixture
                    .checkpoint
                    .convergence
                    .mutations
                    .restore_unresolved_tool_effects(
                        std::collections::BTreeSet::from([
                            "no-original-source-for-this-claim".into()
                        ]),
                        &std::collections::BTreeSet::new(),
                    );
            }
            _ => unreachable!(),
        }
        fixture.append(&facts);
        fixture.seed();
        assert!(
            unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()),
            "{scenario} must not borrow unrelated original evidence"
        );
        fixture.append(&[fact(
            "turn.complete",
            "cannot-forgive-cache-gap",
            json!({"receipt_keys":[fixture.write.identity.key().unwrap(),fixture.verify.identity.key().unwrap()]}),
        )]);
        crate::fact_control::replay_unsettled_mutations(
            fixture.pinned.parts.surface.as_ref().unwrap(),
        );
        assert!(
            matches!(
                fixture.gate(),
                crate::harness_loop::CompletionGate::Incomplete { .. }
            ),
            "{scenario} must refuse Narrative, Verified and waiver closure after a marker"
        );
        {
            let surface = fixture.pinned.parts.surface.as_ref().unwrap();
            let ledger = surface.ledger.lock().unwrap();
            let reports = surface.reports.lock().unwrap();
            let waiver = crate::harness_loop::CompletionWaiverV1::new(
                ledger.digest(),
                "host waiver cannot supply missing receipt evidence",
            )
            .unwrap();
            assert!(
                matches!(
                    crate::harness_loop::decide_completion(&ledger, &reports, &[waiver], true),
                    crate::harness_loop::CompletionGate::Incomplete { .. }
                ),
                "{scenario} must remain unproved with a host waiver and continuation enabled"
            );
        }
        fixture.pinned.close().await;
    }
}

#[tokio::test]
async fn an_original_write_without_its_verifier_cannot_authorize_cached_passed_reports() {
    let mut fixture = CacheCoverageFixture::new().await;
    // Deliberately retain only the write's cached key: this isolates report
    // provenance from the missing-verifier-key coverage check.
    fixture.checkpoint.convergence.mutations = Default::default();
    fixture
        .checkpoint
        .convergence
        .mutations
        .observe_tool_receipt(&fixture.write.tool_name, &fixture.written);
    fixture.append(&CacheCoverageFixture::pair(
        &fixture.write,
        &fixture.written,
    ));
    fixture.seed();
    assert!(unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()));
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    fixture.pinned.close().await;
}

#[tokio::test]
async fn cached_passed_report_requires_the_original_accepted_bound_report() {
    for scenario in [
        "editor",
        "unbound",
        "malformed",
        "needs-review",
        "background-missing",
    ] {
        let mut fixture = CacheCoverageFixture::new().await;
        if scenario == "background-missing" {
            fixture
                .checkpoint
                .convergence
                .mutations
                .observe_background_receipt(
                    "background:missing-exact-child:exit",
                    &ToolEffectObservationV1::complete(Vec::new()),
                );
        } else {
            let mut output: serde_json::Value =
                serde_json::from_str(fixture.verified.output.as_str().unwrap()).unwrap();
            match scenario {
                "editor" => output["metadata"]["verification_author"] = json!("editor"),
                "unbound" => {
                    output["metadata"]["verification_report"]["effect_digest"] = json!(null)
                }
                "malformed" => output["metadata"]["verification_report"] = json!({"invalid":true}),
                "needs-review" => {
                    output["metadata"]["verification_report"]["status"] = json!("needs_review");
                    output["metadata"]["verification_report"]["checks"][0]["status"] =
                        json!("needs_review");
                }
                _ => unreachable!(),
            }
            fixture.verified.output = json!(output.to_string());
        }
        fixture.append(&fixture.original());
        fixture.seed();
        assert!(
            unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()),
            "{scenario} cannot become original positive report evidence"
        );
        assert!(matches!(
            fixture.gate(),
            crate::harness_loop::CompletionGate::Incomplete { .. }
        ));
        fixture.pinned.close().await;
    }
}

#[tokio::test]
async fn reports_only_and_unreadable_original_logs_remain_unproved() {
    for unreadable in [false, true] {
        let mut fixture = CacheCoverageFixture::new().await;
        fixture.append(&fixture.original());
        if unreadable {
            let path = crate::session_folder::effect_log_dir(
                fixture.workspace.path(),
                fixture.session.session_id(),
            )
            .join(format!("{}.jsonl", fixture.thread));
            std::fs::write(path, b"unreadable original fact line\n").unwrap();
        } else {
            fixture.checkpoint.convergence.mutations = Default::default();
        }
        if unreadable {
            let error =
                seed_resumed_mutation_state(&mut fixture.pinned, &fixture.checkpoint).unwrap_err();
            assert!(error
                .to_string()
                .contains("tool_effect_recovery_unproved_append_failed"));
        } else {
            fixture.seed();
        }
        assert!(unresolved(&fixture.pinned).contains(&"tool_effect_cache_evidence_missing".into()));
        assert!(matches!(
            fixture.gate(),
            crate::harness_loop::CompletionGate::Incomplete { .. }
        ));
        fixture.pinned.close().await;
    }
}

mod recovery_identity;
mod recovery_lifecycle;

#[tokio::test]
async fn originally_settled_legacy_history_stays_readable_without_positive_cache_claims() {
    let mut fixture = CacheCoverageFixture::new().await;
    fixture.checkpoint.convergence.mutations = Default::default();
    fixture.checkpoint.verification_reports.clear();
    fixture.append(&[
        fact(
            "model.turn",
            "model:1:0",
            json!({"kind":"tool","call":{"id":"legacy-write","name":"write","args":{"file_path":"old.txt"},"needs_confirmation":false}}),
        ),
        fact(
            "tool.result",
            "tool-result:1:0:legacy-write",
            json!({"ok":true,"toolCallId":"legacy-write","output":"legacy result"}),
        ),
        fact("turn.complete", "original-legacy-settlement", json!({})),
    ]);
    fixture.seed();
    assert!(unresolved(&fixture.pinned).is_empty());
    assert!(matches!(
        fixture.gate(),
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Narrative
        )
    ));
    fixture.pinned.close().await;
}
