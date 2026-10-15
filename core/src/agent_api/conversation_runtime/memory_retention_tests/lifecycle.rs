#![cfg(test)]

use super::*;
use support::*;

mod support;

// Correct-contract regression: collect BOTH real adapters before asserting the
// main callback requirement. Current T7-GOV-01 is expected to make this red;
// an extractor-only record is not accepted as full main-call accounting.
#[tokio::test]
async fn blocking_send_awaits_original_extraction_and_accounts_usage_in_both_stores() {
    let mut observations = Vec::new();
    for kind in STORES {
        let capture = TraceCapture::default();
        let _trace = capture.install();
        let fixture = Fixture::new(
            kind,
            MainReply::Success,
            true,
            false,
            false,
            StoreOutcome::Ack,
            FixturePolicy {
                disabled: false,
                deny: false,
            },
        )
        .await;
        let call = start(&fixture.session, Route::Blocking);
        fixture.client.extraction.wait().await;
        assert_pending_ticket(&fixture.session).await;
        assert!(
            !call.is_finished(),
            "blocking send awaits the original extractor"
        );
        fixture.assert_calls(1, 1, 0);
        fixture.client.extraction.release.add_permits(1);
        let result = join(call).await;
        result.assert_success();
        fixture.drain().await;
        fixture.guard.assert_checks(1, 1);
        fixture.assert_calls(1, 1, 1);
        let items = fixture.canonical().await;
        assert_eq!(items.len(), 1);
        let trace = capture.last();
        field(&trace, "disposition", "stored");
        assert_eq!(canonical_ids(&trace), vec![items[0].id.clone()]);
        observations.push((kind, fixture.guard.records()));
        fixture.session.close().await;
    }
    assert_eq!(observations.len(), STORES.len());
    let expected = vec![
        (SESSION_ID.to_owned(), record_tuple(&main_usage())),
        (SESSION_ID.to_owned(), record_tuple(&extraction_usage())),
    ];
    for (kind, records) in &observations {
        eprintln!("original callback observations {kind:?}: {records:?}");
    }
    assert!(observations.iter().all(|(_, records)| records == &expected),
        "every successful main and extraction call must record its exact usage through the original guard; observations={observations:?}; expected={expected:?}");
}

#[tokio::test]
async fn actual_session_extraction_budget_deny_has_no_provider_retry_in_both_stores() {
    for kind in STORES {
        for route in ROUTES {
            let capture = TraceCapture::default();
            let _trace = capture.install();
            let fixture = Fixture::new(
                kind,
                MainReply::Success,
                false,
                false,
                false,
                StoreOutcome::Ack,
                FixturePolicy {
                    disabled: false,
                    deny: true,
                },
            )
            .await;
            let result = join(start(&fixture.session, route)).await;
            result.assert_success();
            fixture.drain().await;
            fixture.assert_calls(1, 0, 0);
            fixture.guard.assert_checks(1, 1);
            let records = fixture.guard.records();
            assert!(records.iter().all(|(id, usage)| id == SESSION_ID && *usage == record_tuple(&main_usage())),
                "Deny must add no extractor callback; main accounting remains separately blocked: {records:?}");
            eprintln!(
                "all-stage deny observation {kind:?}/{route:?}: checks={:?}, records={records:?}",
                fixture.guard.checks.lock().unwrap()
            );
            assert!(fixture.canonical().await.is_empty());
            let trace = capture.last();
            field(&trace, "disposition", "refused");
            field(&trace, "refusal", "budget_denied");
            field(&trace, "entered_store_calls", "0");
            field(&trace, "unproved_write", "false");
            assert!(canonical_ids(&trace).is_empty());
            fixture.session.close().await;
        }
    }
}

#[tokio::test]
async fn actual_session_original_cancel_before_store_refuses_in_both_stores() {
    for kind in STORES {
        for route in ROUTES {
            let capture = TraceCapture::default();
            let _trace = capture.install();
            let fixture = Fixture::new(
                kind,
                MainReply::Success,
                false,
                true,
                false,
                StoreOutcome::Ack,
                FixturePolicy {
                    disabled: false,
                    deny: false,
                },
            )
            .await;
            let original = fixture.original_cancel();
            let call = start(&fixture.session, route);
            fixture.boundary.pre_store.wait().await;
            assert!(fixture.client.extraction_returned.load(Ordering::SeqCst));
            assert_pending_ticket(&fixture.session).await;
            fixture.assert_calls(1, 1, 0);
            assert!(!original.is_cancelled());
            original.cancel();
            fixture.assert_original_cancelled();
            fixture.boundary.pre_store.release.add_permits(1);
            join(call).await.assert_success();
            fixture.drain().await;
            fixture.assert_calls(1, 1, 0);
            fixture.guard.assert_checks(1, 1);
            assert_eq!(fixture.store.store.entered.load(Ordering::SeqCst), 0);
            assert!(fixture.canonical().await.is_empty());
            let trace = capture.last();
            field(&trace, "disposition", "refused");
            field(&trace, "refusal", "cancelled");
            field(&trace, "accepted_items", "1");
            field(&trace, "entered_store_calls", "0");
            field(&trace, "unproved_write", "false");
            assert!(canonical_ids(&trace).is_empty());
            fixture.session.close().await;
        }
    }
}

#[tokio::test]
async fn actual_session_entered_store_results_remain_truthful_in_both_stores() {
    for kind in STORES {
        for route in ROUTES {
            for outcome in [
                StoreOutcome::Ack,
                StoreOutcome::ErrBefore,
                StoreOutcome::CommitThenErr,
            ] {
                let capture = TraceCapture::default();
                let _trace = capture.install();
                let fixture = Fixture::new(
                    kind,
                    MainReply::Success,
                    false,
                    false,
                    true,
                    outcome,
                    FixturePolicy {
                        disabled: false,
                        deny: false,
                    },
                )
                .await;
                let original = fixture.original_cancel();
                let call = start(&fixture.session, route);
                fixture.boundary.entered_store.wait().await;
                assert_pending_ticket(&fixture.session).await;
                fixture.assert_calls(1, 1, 1);
                assert_eq!(fixture.store.store.entered.load(Ordering::SeqCst), 0);
                original.cancel();
                fixture.assert_original_cancelled();
                fixture.boundary.entered_store.release.add_permits(1);
                join(call).await.assert_success();
                fixture.drain().await;
                fixture.assert_calls(1, 1, 1);
                fixture.guard.assert_checks(1, 1);
                let items = fixture.canonical().await;
                let trace = capture.last();
                field(&trace, "accepted_items", "1");
                field(&trace, "entered_store_calls", "1");
                field(&trace, "abandoned", "false");
                match outcome {
                    StoreOutcome::Ack => {
                        assert_eq!(items.len(), 1);
                        assert_eq!(fixture.store.store.entered.load(Ordering::SeqCst), 1);
                        let committed = fixture.boundary.committed.lock().unwrap();
                        assert_eq!(committed.len(), 1);
                        assert_eq!(items[0].id, committed[0].id);
                        assert_eq!(canonical_ids(&trace), vec![items[0].id.clone()]);
                        field(&trace, "disposition", "stored");
                        field(&trace, "unproved_write", "false");
                        assert!(
                            !trace.contains_key("refusal"),
                            "None must be absent: {trace:?}"
                        );
                    }
                    StoreOutcome::ErrBefore => {
                        assert!(items.is_empty());
                        assert!(fixture.boundary.committed.lock().unwrap().is_empty());
                        assert_eq!(fixture.store.store.entered.load(Ordering::SeqCst), 0);
                        field(&trace, "disposition", "unproved");
                        field(&trace, "refusal", "store_failed");
                        field(&trace, "unproved_write", "true");
                        assert!(canonical_ids(&trace).is_empty());
                    }
                    StoreOutcome::CommitThenErr => {
                        assert_eq!(items.len(), 1);
                        assert_eq!(fixture.store.store.entered.load(Ordering::SeqCst), 1);
                        let committed = fixture.boundary.committed.lock().unwrap();
                        assert_eq!(committed.len(), 1);
                        assert_eq!(items[0].id, committed[0].id);
                        field(&trace, "disposition", "unproved");
                        field(&trace, "refusal", "store_failed");
                        field(&trace, "unproved_write", "true");
                        assert!(
                            canonical_ids(&trace).is_empty(),
                            "readback cannot forge a returned Ack"
                        );
                    }
                }
                fixture.session.close().await;
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn actual_session_close_timeout_uses_original_five_second_drain_in_both_stores() {
    // Real file setup/cleanup uses the real clock; only the original close
    // drain boundary is measured with the explicitly paused clock.
    tokio::time::resume();
    for kind in STORES {
        for route in ROUTES {
            let capture = TraceCapture::default();
            let _trace = capture.install();
            let fixture = Fixture::new(
                kind,
                MainReply::Success,
                true,
                false,
                false,
                StoreOutcome::Ack,
                FixturePolicy {
                    disabled: false,
                    deny: false,
                },
            )
            .await;
            let original = fixture.original_cancel();
            let mut call = Some(start(&fixture.session, route));
            fixture.client.extraction.wait().await;
            assert_pending_ticket(&fixture.session).await;
            fixture.assert_calls(1, 1, 0);
            assert!(!fixture.client.extraction_returned.load(Ordering::SeqCst));
            assert_eq!(fixture.client.extraction.release.available_permits(), 0);
            let original_close = &fixture.session.close_handle;
            assert!(original_close.command_queue.is_none());
            assert!(original_close.memory_maintenance.is_none());
            assert!(Arc::ptr_eq(
                &fixture.session.active_run_control,
                &original_close.active_run_control
            ));
            assert!(Arc::ptr_eq(
                &fixture.session.run_store,
                &original_close.run_store
            ));
            // Streaming publishes and joins its actual public guardian before
            // the retained memory tail finishes. Other routes still retain the
            // original execution while this extraction is blocked.
            let streamed = if matches!(route, Route::Stream) {
                let actual = join(call.take().unwrap()).await;
                actual.assert_success();
                assert!(fixture.session.active_run_control.lock().await.is_none());
                assert!(fixture.session.current_run_id.lock().await.is_none());
                Some(actual)
            } else {
                let control_slot = fixture.session.active_run_control.lock().await;
                let control = Arc::clone(control_slot.as_ref().unwrap());
                let control_snapshot = control.snapshot().await;
                assert_eq!(control_snapshot.session_id, SESSION_ID);
                assert_eq!(control_snapshot.run_id, control.snapshot_run_id());
                assert_eq!(control_snapshot.queued_controls, 0);
                assert_eq!(control_snapshot.last_sequence, 0);
                assert!(!control_snapshot.interrupt_requested);
                let run = fixture
                    .session
                    .run_store
                    .snapshot(&control_snapshot.run_id)
                    .await
                    .unwrap();
                assert_eq!(run.session_id, SESSION_ID);
                assert_eq!(run.id, control_snapshot.run_id);
                assert!(Arc::ptr_eq(control_slot.as_ref().unwrap(), &control));
                None
            };
            assert!(!original.is_cancelled());
            tokio::time::pause();
            let close = fixture.session.close();
            tokio::pin!(close);
            // Queue/maintenance are absent. The first actual close poll reaches
            // its original memory drain and registers this same waker; no
            // retained/cleared stream slot is used as a timer-boundary proxy.
            let wake = Arc::new(CloseWake::default());
            let registered_at = tokio::time::Instant::now();
            assert!(matches!(poll_close(close.as_mut(), &wake), Poll::Pending));
            assert!(fixture.session.is_closed());
            assert_eq!(wake.count(), 0);
            assert!(!original.is_cancelled());
            tokio::time::advance(Duration::from_secs(4)).await;
            assert!(matches!(poll_close(close.as_mut(), &wake), Poll::Pending));
            assert_eq!(wake.count(), 0);
            assert!(
                !original.is_cancelled(),
                "original five-second drain has not expired"
            );
            fixture.assert_calls(1, 1, 0);
            assert!(!fixture.client.extraction_returned.load(Ordering::SeqCst));
            tokio::time::advance(Duration::from_secs(1)).await;
            // advance does not guarantee timer-driver delivery. Wait for this
            // same close's actual wake, allowing only Tokio's upward 1ms tick
            // rounding, never a new production drain deadline or yield loop.
            let woke_at = wake.wait().await;
            let elapsed = woke_at.duration_since(registered_at);
            assert!(elapsed >= Duration::from_secs(5), "{elapsed:?}");
            assert!(
                elapsed <= Duration::from_secs(5) + Duration::from_millis(1),
                "{elapsed:?}"
            );
            eprintln!("original drain wake {kind:?}/{route:?}: {elapsed:?}");
            let close_finished = matches!(poll_close(close.as_mut(), &wake), Poll::Ready(()));
            assert!(
                original.is_cancelled(),
                "actual timer wake must reach original parent cancellation"
            );
            fixture.assert_original_cancelled();
            tokio::time::resume();
            let result = if let Some(actual) = streamed {
                if !close_finished {
                    (&mut close).await;
                }
                actual
            } else if close_finished {
                join(call.take().unwrap()).await
            } else {
                let ((), result) = tokio::join!(&mut close, join(call.take().unwrap()));
                result
            };
            result.assert_success();
            fixture.drain().await;
            fixture.assert_calls(1, 1, 0);
            fixture.guard.assert_checks(1, 1);
            assert!(fixture
                .client
                .extraction_dropped_pending
                .load(Ordering::SeqCst));
            assert!(!fixture.client.extraction_returned.load(Ordering::SeqCst));
            assert!(fixture.canonical().await.is_empty());
            let trace = capture.last();
            field(&trace, "disposition", "refused");
            field(&trace, "refusal", "cancelled");
            field(&trace, "entered_store_calls", "0");
            field(&trace, "unproved_write", "false");
            assert!(canonical_ids(&trace).is_empty());
            assert!(capture
                .events()
                .iter()
                .any(|event| event.get("message").map(String::as_str)
                    == Some(
                        "Completed-turn memory extraction did not drain before close timeout"
                    )));
        }
    }
}

#[tokio::test]
async fn actual_session_failed_empty_disabled_turns_admit_no_extraction_in_both_stores() {
    for kind in STORES {
        for route in ROUTES {
            for (main, disabled) in [
                (MainReply::Error, false),
                (MainReply::Empty, false),
                (MainReply::Success, true),
            ] {
                let capture = TraceCapture::default();
                let _trace = capture.install();
                let fixture = Fixture::new(
                    kind,
                    main,
                    false,
                    false,
                    false,
                    StoreOutcome::Ack,
                    FixturePolicy {
                        disabled,
                        deny: false,
                    },
                )
                .await;
                let result = join(start(&fixture.session, route)).await;
                fixture.drain().await;
                fixture.assert_calls(1, 0, 0);
                fixture.guard.assert_checks(1, 0);
                assert!(fixture
                    .guard
                    .records()
                    .iter()
                    .all(|(id, usage)| id == SESSION_ID && *usage == record_tuple(&main_usage())));
                assert!(fixture.canonical().await.is_empty());
                match main {
                    MainReply::Success => {
                        result.assert_success();
                        let trace = capture.last();
                        field(&trace, "disposition", "not_admitted");
                        field(&trace, "refusal", "disabled");
                    }
                    MainReply::Empty => {
                        result.assert_successful_publication("");
                        if let Some(value) = &result.result {
                            assert_eq!(value.as_ref().unwrap().text, "");
                        } else {
                            assert!(!result
                                .events
                                .iter()
                                .any(|event| matches!(event, AgentEvent::Error { .. })));
                            assert_eq!(result.events.iter().filter(|event| matches!(event, AgentEvent::End { text, .. } if text.is_empty())).count(), 1);
                        }
                        let trace = capture.last();
                        field(&trace, "disposition", "not_admitted");
                        field(&trace, "refusal", "empty_turn");
                    }
                    MainReply::Error => {
                        if let Some(value) = &result.result {
                            assert!(value.is_err());
                        } else {
                            assert!(result
                                .events
                                .iter()
                                .any(|event| matches!(event, AgentEvent::Error { .. })));
                        }
                        assert!(capture.events().iter().all(|event| event.get("disposition").is_none()),
                            "failed main turn has no eligible mapped result/ticket, not a fabricated completed turn");
                    }
                }
                fixture.session.close().await;
            }
        }
    }
}

#[tokio::test]
async fn supplemental_original_facade_parts_match_and_aux_keep_guard_usage_in_both_stores() {
    use crate::harness_evidence::{ModelCallObservation, ModelInputKindV1};
    for kind in STORES {
        for auxiliary in [false, true] {
            let fixture = Fixture::ordinary(kind).await;
            let original = fixture.original_cancel();
            let cancellation = original.child_token();
            let agent = crate::agent_api::agent_loop_runtime::build_agent_loop(&fixture.session);
            let (bound_tx, mut bound_rx) = mpsc::channel(8);
            let invocation = agent.invocation_context(
                "lifecycle-explicit-scope",
                Some(SESSION_ID),
                Some(bound_tx.clone()),
                cancellation.clone(),
            );
            assert_eq!(invocation.run_id(), "lifecycle-explicit-scope");
            assert_eq!(invocation.session_id(), fixture.session.session_id);
            let original_guard: Arc<dyn crate::budget::BudgetGuard> = fixture.guard.clone();
            assert!(Arc::ptr_eq(
                invocation.governance().budget_guard().unwrap(),
                &original_guard
            ));
            let agent = invocation.bind_agent_loop(&agent);
            let messages = [Message::user("Explicit facade identity probe.")];
            // Seed only evidence, not a provider call. Matched helpers must
            // continue the original sequence; auxiliary helpers start their
            // own sequence on the requested distinct channel. Snapshot wire
            // types expose no run/session ID or generated auxiliary UUID.
            let _seed = invocation
                .capture_model_evidence(ModelCallObservation::new(
                    ModelInputKindV1::Completion,
                    &messages,
                    None,
                    &[],
                    None,
                    crate::llm::estimate_prompt_tokens(&messages, None, &[]),
                ))
                .unwrap()
                .unwrap();
            let (other_tx, mut other_rx) = mpsc::channel(8);
            let event_tx = Some(if auxiliary { other_tx } else { bound_tx });
            let facade =
                agent.scoped_llm_client_for_parts(Some(SESSION_ID), &event_tx, &cancellation);
            let result = facade.complete(&messages, None, &[]).await.unwrap();
            assert_eq!(usage_tuple(&result.usage), usage_tuple(&extraction_usage()));
            let receiver = if auxiliary {
                &mut other_rx
            } else {
                &mut bound_rx
            };
            let mut events = Vec::new();
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
            assert_eq!(events.len(), 4);
            let capability = match &events[0] {
                AgentEvent::RunCapabilityBound { snapshot, .. } => snapshot,
                other => panic!("{other:?}"),
            };
            let presentation = match &events[1] {
                AgentEvent::ModelPresentationBound { snapshot } => snapshot,
                other => panic!("{other:?}"),
            };
            let input = match &events[2] {
                AgentEvent::ModelInputBound { snapshot } => snapshot,
                other => panic!("{other:?}"),
            };
            let usage = match &events[3] {
                AgentEvent::ModelUsageBound { snapshot } => snapshot,
                other => panic!("{other:?}"),
            };
            input.validate_against(capability).unwrap();
            presentation.validate_against(input).unwrap();
            usage.validate_against(input).unwrap();
            assert!(capability.policy.budget_guard_bound);
            assert_eq!(input.call_sequence, if auxiliary { 1 } else { 2 });
            assert_eq!(usage.call_sequence, input.call_sequence);
            assert_eq!(
                (
                    usage.reported_prompt_tokens,
                    usage.reported_completion_tokens,
                    usage.reported_total_tokens
                ),
                usage_tuple(&extraction_usage())
            );
            assert_eq!(
                fixture.guard.records(),
                vec![(SESSION_ID.to_owned(), record_tuple(&extraction_usage()))]
            );
            fixture.guard.assert_checks(1, 0);
            if auxiliary {
                assert!(bound_rx.try_recv().is_err());
            } else {
                assert!(other_rx.try_recv().is_err());
            }
            fixture.assert_calls(0, 1, 0);
            assert_no_ticket(&fixture.session).await;
            assert!(fixture.canonical().await.is_empty());
            // Same Session cancellation lineage, no new ungoverned token.
            original.cancel();
            assert!(invocation.cancellation().is_cancelled());
            assert!(facade.complete(&messages, None, &[]).await.is_err());
            fixture.assert_calls(0, 1, 0);
            assert_eq!(fixture.guard.records().len(), 1);
            fixture.session.close().await;
        }
    }
}
