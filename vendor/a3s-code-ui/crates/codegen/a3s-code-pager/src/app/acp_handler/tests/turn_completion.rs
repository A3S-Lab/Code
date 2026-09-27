#![cfg_attr(rustfmt, rustfmt::skip)]
    use super::*;

    #[test]
    fn driver_prompt_complete_without_prompt_id_arms_reconcile_not_finish() {
        // Driver still owns the turn via PromptResponse: prompt_complete must NOT finish immediately
        // Missing wire promptId (legacy shells) arms lost-PR reconcile on current_prompt_id so grace teardown can run if the RPC never arrives
        // Turn state stays TurnRunning
        let mut app = make_app_with_agent("sess-drive");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-local".into());
            agent.turn_started_at = Some(std::time::Instant::now());
            assert!(!agent.attached_as_viewer);
        }

        let affected = handle_ext_notification(&prompt_complete_ext("sess-drive"), &mut app);
        assert!(
            affected,
            "arming reconcile must schedule ticks for background-tab recovery"
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            matches!(agent.session.state, AgentState::TurnRunning),
            "driver's running turn must NOT be finished by prompt_complete"
        );
        assert_eq!(
            agent.session.current_prompt_id.as_deref(),
            Some("pid-local"),
            "driver's current_prompt_id must be untouched at arm time"
        );
        assert!(agent.turn_started_at.is_some());
        assert_eq!(
            agent
                .pending_turn_end_reconcile
                .as_ref()
                .map(|p| p.prompt_id.as_str()),
            Some("pid-local"),
        );
    }

    #[test]
    fn driver_prompt_complete_with_matching_prompt_id_arms_reconcile() {
        // Lost-response recovery: the driver receives the turn-end broadcast for the exact turn it is awaiting
        // It must ARM the deferred reconcile without finishing the turn immediately
        // The RPC response normally lands ms later and carries richer context; finishing here would double-finish every turn
        let mut app = make_app_with_agent("sess-drive");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-stuck".into());
            agent.session.cancel_turn(&mut agent.scrollback); // CancelTurn → TurnCancelling
            assert!(!agent.attached_as_viewer);
        }

        let affected = handle_ext_notification(
            &prompt_complete_ext_with_prompt_id("sess-drive", "pid-stuck", "cancelled"),
            &mut app,
        );
        assert!(
            affected,
            "arming must report a state change — the event loop only calls \
             schedule_tick on changed ACP batches, and the reconcile sweep \
             runs on the animation tick (a dormant background tab would \
             otherwise never get swept)"
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            agent.session.state.is_cancelling(),
            "turn state must be untouched at arm time (RPC may still arrive)"
        );
        let pending = agent
            .pending_turn_end_reconcile
            .as_ref()
            .expect("reconcile must be armed for the driver's awaited turn");
        assert_eq!(pending.prompt_id, "pid-stuck");
        assert_eq!(pending.stop_reason.as_deref(), Some("cancelled"));
    }

    #[test]
    fn driver_prompt_complete_with_mismatched_prompt_id_does_not_arm() {
        // A broadcast for some OTHER prompt must not arm a reconcile against the turn this client is actually driving
        // Other prompts here: a stale one, or a queued prompt that resolved server-side
        let mut app = make_app_with_agent("sess-drive");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-current".into());
        }

        let _ = handle_ext_notification(
            &prompt_complete_ext_with_prompt_id("sess-drive", "pid-other", "end_turn"),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.pending_turn_end_reconcile.is_none());
        assert!(matches!(agent.session.state, AgentState::TurnRunning));
    }

    #[test]
    fn driver_prompt_complete_without_prompt_id_arms_on_current() {
        // Older shells omit `promptId`; arm reconcile on current_prompt_id when not mid-tool (see arm_driver_turn_end_reconcile)
        // The turn is not finished here
        let mut app = make_app_with_agent("sess-drive");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-current".into());
        }

        let _ = handle_ext_notification(&prompt_complete_ext("sess-drive"), &mut app);

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent
                .pending_turn_end_reconcile
                .as_ref()
                .map(|p| p.prompt_id.as_str()),
            Some("pid-current"),
        );
        assert!(matches!(agent.session.state, AgentState::TurnRunning));
    }

    #[test]
    fn driver_prompt_complete_pushes_no_marker() {
        // The driver emits its own marker via PromptResponse; prompt_complete must not double-push one for it (or push any block at all)
        let mut app = make_app_with_agent("sess-drive");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-local".into());
            agent.turn_started_at = Some(std::time::Instant::now());
            assert!(!agent.attached_as_viewer);
        }

        let len_before = app.agents.get(&AgentId(0)).unwrap().scrollback.len();
        let _ = handle_ext_notification(&prompt_complete_ext("sess-drive"), &mut app);
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent.scrollback.len(),
            len_before,
            "the driver must not get any new block from prompt_complete"
        );
    }

    #[test]
    fn live_turn_completed_finalizes_viewer_turn_and_duplicate_is_noop() {
        // The durable `TurnCompleted` is the viewer's non-interactive exit from TurnRunning on the replayed rail
        // It parallels the fire-and-forget `prompt_complete`
        // A viewer adopting the driver's live turn must drop back to Idle with a marker when it arrives
        let mut app = make_app_with_agent("sess-view");
        app.agents.get_mut(&AgentId(0)).unwrap().attached_as_viewer = true;
        let _ = handle(
            make_agent_chunk_message_with_prompt("sess-view", "chunk", "pid-driver", false),
            &mut app,
        );
        assert!(matches!(
            app.agents.get(&AgentId(0)).unwrap().session.state,
            AgentState::TurnRunning
        ));

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif("sess-view", "pid-driver", "end_turn", false),
            &mut app,
        );
        assert!(affected, "finalizing the active viewer turn should redraw");
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            agent.session.state.is_idle(),
            "a live TurnCompleted must drop a viewer back to Idle"
        );
        assert!(agent.session.current_prompt_id.is_none());
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCompleted { .. })
        ));

        // A duplicate/stale terminal for the now-finished turn is a no-op.
        let len_before = app.agents.get(&AgentId(0)).unwrap().scrollback.len();
        let affected = handle_ext_notification(
            &a3s_turn_completed_notif("sess-view", "pid-driver", "end_turn", false),
            &mut app,
        );
        assert!(!affected, "a duplicate TurnCompleted must be a no-op");
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().scrollback.len(),
            len_before,
            "a duplicate TurnCompleted must not push a second marker"
        );
    }

    #[test]
    fn unknown_error_kind_from_wire_is_never_sniff_reclassified() {
        // A NEWER shell's kind the pager doesn't know arrives through the real ingress
        // The result quotes a truncation phrase and carries no status
        // A present kind blocks the sniff reclassification, so it renders generic copy, not truncation
        let mut app = make_app_with_agent("sess-view");
        app.agents.get_mut(&AgentId(0)).unwrap().attached_as_viewer = true;
        let _ = handle(
            make_agent_chunk_message_with_prompt("sess-view", "chunk", "pid-driver", false),
            &mut app,
        );

        let _ = handle_ext_notification(
            &a3s_turn_completed_failed_with_error_kind(
                "sess-view",
                "pid-driver",
                "a future failure quoting: response truncated by max_tokens",
                "a_future_kind",
                false,
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        match last_session_event(&agent.scrollback) {
            Some(SessionEvent::TurnFailed { error, .. }) => {
                assert!(
                    error.starts_with("Request failed"),
                    "unknown kind must keep generic copy, got {error:?}"
                );
                assert!(!error.contains("Response truncated"));
            }
            other => panic!("expected TurnFailed, got {other:?}"),
        }
    }

    #[test]
    fn live_turn_completed_error_kind_renders_truncation_copy() {
        let mut app = make_app_with_agent("sess-view");
        app.agents.get_mut(&AgentId(0)).unwrap().attached_as_viewer = true;
        let _ = handle(
            make_agent_chunk_message_with_prompt("sess-view", "chunk", "pid-driver", false),
            &mut app,
        );

        let _ = handle_ext_notification(
            &a3s_turn_completed_failed_with_error_kind(
                "sess-view",
                "pid-driver",
                "turn ended early",
                "max_tokens_truncation",
                false,
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        match last_session_event(&agent.scrollback) {
            Some(SessionEvent::TurnFailed { error, .. }) => {
                assert_eq!(error, "Response truncated: turn ended early")
            }
            other => panic!("expected TurnFailed, got {other:?}"),
        }
    }

    #[test]
    fn live_turn_completed_driver_arms_reconcile() {
        // For the driver the `PromptResponse` RPC owns the lifecycle
        // A live TurnCompleted for the turn it is driving arms the lost-RPC reconcile WITHOUT finishing the turn
        // This mirrors the `prompt_complete` driver path
        let mut app = make_app_with_agent("sess-drive");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-local".into());
            assert!(!agent.attached_as_viewer);
        }

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-drive", "pid-local", "cancelled", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            matches!(agent.session.state, AgentState::TurnRunning),
            "the driver's turn must NOT be finished by a live TurnCompleted"
        );
        let pending = agent
            .pending_turn_end_reconcile
            .as_ref()
            .expect("the driver's awaited turn must arm a reconcile");
        assert_eq!(pending.prompt_id, "pid-local");
        assert_eq!(pending.stop_reason.as_deref(), Some("cancelled"));
    }

    #[test]
    fn silent_wake_turn_completed_is_markerless() {
        let mut app = make_app_with_agent("sess-wake");
        seed_two_bg_tasks(&mut app, "sess-wake");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let affected = handle_ext_notification(
            &a3s_wake_turn_completed_notif(
                "sess-wake",
                "task-completed-bg1",
                Some(1_700_000_000_000 + 5_000),
            ),
            &mut app,
        );
        assert!(affected, "the wake back-to-idle point still redraws");

        {
            let agent = app.agents.get(&AgentId(0)).unwrap();
            assert!(
                agent.session.state.is_idle(),
                "a wake turn is never adopted — the pager stays idle around it"
            );
            assert_eq!(
                agent.scrollback.len(),
                len_before,
                "a silent wake turn pushes no marker"
            );
            assert_eq!(
                agent.watchers().commands,
                2,
                "the running commands stay on the status-row watchers cue"
            );
        }
        assert!(
            app.deferred_notification.is_none(),
            "a silent wake must not queue TurnComplete"
        );
    }

    #[test]
    fn chatty_wake_turn_completed_pushes_one_marker() {
        use crate::app::agent_view::test_fixtures::count_turn_markers;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        assert_eq!(count_turn_markers(app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry"))), 0);

        let affected = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", None),
            &mut app,
        );
        assert!(affected);

        {
            let agent = app.agents.get(&AgentId(0)).unwrap();
            assert_eq!(
                count_turn_markers(agent),
                1,
                "a chatty wake closes with exactly one marker"
            );
            assert!(matches!(
                last_session_event(&agent.scrollback),
                Some(SessionEvent::TurnCompleted { .. })
            ));
        }
        let (event, ticks) = app
            .deferred_notification
            .as_ref()
            .expect("chatty wake EndTurn must queue TurnComplete");
        assert_eq!(event.kind, crate::notifications::NotificationEventKind::TurnComplete);
        assert_eq!(*ticks, 3);
    }

    #[test]
    fn duplicate_wake_terminal_pushes_no_second_marker() {
        // `finish_wake_turn` snapshots the output epoch, so a duplicate sees no new output.
        use crate::app::agent_view::test_fixtures::count_turn_markers;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        let _ = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", None),
            &mut app,
        );
        assert_eq!(count_turn_markers(app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry"))), 1);

        let _ = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", None),
            &mut app,
        );
        assert_eq!(
            count_turn_markers(app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry"))),
            1,
            "a duplicate wake terminal must not push a second marker"
        );
    }

    #[test]
    fn wake_turn_stop_affordance_offered_then_cleared_at_terminal() {
        // The pane stays Idle around a wake turn, so the stop control is keyed on `running_wake_turn`
        // That flag is set by the first live wake delta and cleared by the wake terminal
        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            matches!(agent.wake_display_state(), Some(AgentState::TurnRunning)),
            "a streaming wake turn must offer the running chrome (and [stop])"
        );

        // A delta arriving mid-cancel must not reset the cancelling phase.
        if let Some(wake) = app
            .agents
            .get_mut(&AgentId(0))
            .unwrap()
            .running_wake_turn
            .as_mut()
        {
            wake.cancel_sent = true;
        }
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 6_000),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            matches!(agent.wake_display_state(), Some(AgentState::TurnCancelling)),
            "a later delta must not clobber the cancelling phase"
        );

        let _ = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", None),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            agent.running_wake_turn.is_none() && agent.wake_display_state().is_none(),
            "the wake terminal must retire the stop affordance"
        );

        // Deltas and the terminal arrive on separate channels: a late delta for the finished wake must not revive the stop control
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 7_000),
            &mut app,
        );
        assert!(
            app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).running_wake_turn.is_none(),
            "a late delta after the terminal must not revive the stop affordance"
        );

        // A second wake finishing must not forget the first: bg1's late delta stays dead after bg2's terminal lands too
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg2", 8_000),
            &mut app,
        );
        let _ = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg2", None),
            &mut app,
        );
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 9_000),
            &mut app,
        );
        assert!(
            app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).running_wake_turn.is_none(),
            "an earlier finished wake stays finished after later terminals"
        );
    }

    #[test]
    fn wake_terminal_drains_parked_follow_up() {
        use crate::app::actions::Effect;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .session
            .enqueue_prompt("follow-up after wake".into());

        let _ = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", None),
            &mut app,
        );
        assert!(
            app.pending_effects
                .iter()
                .any(|e| matches!(e, Effect::SendPrompt { text, .. } if text == "follow-up after wake")),
            "wake terminal must drain the parked follow-up; effects = {:?}",
            app.pending_effects
        );
        assert!(
            app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).session.pending_prompts.is_empty(),
            "the parked row must leave the local queue"
        );
    }

    #[test]
    fn wake_terminal_does_not_drain_while_reconnect_pending() {
        use crate::app::actions::Effect;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .session
            .enqueue_prompt("follow-up after wake".into());
        app.reconnect_pending = true;

        let _ = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", None),
            &mut app,
        );
        assert!(
            !app.pending_effects
                .iter()
                .any(|e| matches!(e, Effect::SendPrompt { .. })),
            "reconnect must hold the parked follow-up; effects = {:?}",
            app.pending_effects
        );
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).session.pending_prompts.len(),
            1,
            "the parked row must stay queued until reconnect drains"
        );
    }

    #[test]
    fn wake_terminal_finishes_in_flight_streamed_entry() {
        // The terminal is a wake's ONLY flush site (wakes skip PromptResponse).
        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        assert!(
            app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.has_running_entries(),
            "the streamed wake chunk opens a live entry"
        );

        let _ = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", None),
            &mut app,
        );
        assert!(
            !app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.has_running_entries(),
            "the wake terminal must finish the streamed entry"
        );
    }

    #[test]
    fn wake_turn_completed_in_replay_records_pid_and_visible_marker() {
        // A visible wake still records its pid and also gets a marker
        // The chunk must be isReplay: live output_epoch does not count
        let mut app = make_app_with_agent("sess-wake");
        begin_replay(&mut app);
        let _ = handle(
            make_replay_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let started_at = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).turn_started_at;

        let affected = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-wake",
                "task-completed-bg1",
                "end_turn",
                Some(1500),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );

        assert!(!affected);
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            agent
                .replayed_terminal_prompts
                .contains("task-completed-bg1"),
            "the replay arm must keep recording wake pids"
        );
        assert_eq!(agent.scrollback.len(), len_before + 1);
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCompleted { elapsed: Some(_) })
        ));
        assert_eq!(agent.turn_started_at, started_at, "replay must not finalize");
    }

    #[test]
    fn scheduler_fired_turn_completed_keeps_adopted_path() {
        // `/loop` turns are client-driven with a real finalize path, never the wake shortcut
        let mut app = make_app_with_agent("sess-cron");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let affected = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-cron", "scheduler-fired-abc", Some(1_000)),
            &mut app,
        );

        assert!(!affected);
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len(),
            len_before,
            "a scheduler-fired terminal must not push a wake marker"
        );
    }

    #[test]
    fn silent_errored_wake_closes_without_marker() {
        // An errored wake closes like a success: no row, state cleared, duplicates deduped per pid
        let mut app = make_app_with_agent("sess-wake");
        seed_two_bg_tasks(&mut app, "sess-wake");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        for _ in 0..2 {
            let _ = handle_ext_notification(
                &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "error", false),
                &mut app,
            );
        }

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(agent.scrollback.len(), len_before, "an errored wake pushes no marker");
        assert!(agent.session.state.is_idle());
        assert!(agent.running_wake_turn.is_none());
        assert!(agent.finished_wake_prompts.contains("task-completed-bg1"));
        assert_eq!(
            agent.failed_wake_marker_for.as_deref(),
            Some("task-completed-bg1")
        );
        assert_eq!(agent.watchers().commands, 2);
    }

    #[test]
    fn errored_wake_with_error_kind_closes_without_marker() {
        // The typed kind used to pick failure copy; an errored wake now paints nothing regardless of kind
        let mut app = make_app_with_agent("sess-wake");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let _ = handle_ext_notification(
            &a3s_turn_completed_failed_with_error_kind(
                "sess-wake",
                "task-completed-bg1",
                "[unknown] Premature close",
                "max_tokens_truncation",
                false,
            ),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(agent.scrollback.len(), len_before);
        assert!(agent.session.state.is_idle());
    }

    #[test]
    fn errored_user_prompt_terminal_still_pushes_turn_failed() {
        // Only wake pids close silently; a viewer's adopted user turn keeps the failure banner
        let mut app = make_app_with_agent("sess-view");
        app.agents.get_mut(&AgentId(0)).unwrap().attached_as_viewer = true;
        let _ = handle(
            make_agent_chunk_message_with_prompt("sess-view", "chunk", "pid-driver", false),
            &mut app,
        );

        let payload = SessionNotification {
            session_id: acp::SessionId::new("sess-view"),
            update: A3sSessionUpdate::TurnCompleted {
                prompt_id: "pid-driver".into(),
                stop_reason: "error".into(),
                agent_result: Some("[unknown] Premature close".into()),
                error_kind: None,
                usage: None,
                elapsed_ms: None,
            },
            meta: Some(serde_json::json!({ "isReplay": false })),
        };
        let notif = acp::ExtNotification::new(
            "x.ai/session/update",
            std::sync::Arc::from(serde_json::value::to_raw_value(&payload).unwrap()),
        );
        let _ = handle_ext_notification(&notif, &mut app);

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.session.state.is_idle());
        match last_session_event(&agent.scrollback) {
            Some(SessionEvent::TurnFailed { error, .. }) => {
                assert_eq!(
                    error,
                    "Request failed: [unknown] Premature close. Try sending again."
                );
            }
            other => panic!("expected TurnFailed, got {other:?}"),
        }
    }

    #[test]
    fn rate_limited_wake_during_local_turn_keeps_rate_limit_copy() {
        // The busy-wake piercing path must pass rate-limit copy through untouched like `finish_wake_turn` does
        // The generic formatter would strip the upgrade URL and headline it "Request failed"
        let mut app = make_app_with_agent("sess-wake");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.state = AgentState::TurnRunning;
        }
        let rate_limit_copy = "You've hit the rate limit for your plan. Upgrade your \
                               subscription for higher limits: https://a3s/supergrok";
        let payload = SessionNotification {
            session_id: acp::SessionId::new("sess-wake"),
            update: A3sSessionUpdate::TurnCompleted {
                prompt_id: "task-completed-bg1".into(),
                stop_reason: "rate_limit".into(),
                agent_result: Some(rate_limit_copy.into()),
                error_kind: None,
                usage: None,
                elapsed_ms: None,
            },
            meta: Some(serde_json::json!({ "isReplay": false })),
        };
        let notif = acp::ExtNotification::new(
            "x.ai/session/update",
            std::sync::Arc::from(serde_json::value::to_raw_value(&payload).unwrap()),
        );

        let _ = handle_ext_notification(&notif, &mut app);

        let agent = app.agents.get(&AgentId(0)).unwrap();
        match last_session_event(&agent.scrollback) {
            Some(SessionEvent::TurnFailed { error, .. }) => {
                assert_eq!(error, rate_limit_copy, "copy must pass through untouched");
            }
            other => panic!("expected TurnFailed, got {other:?}"),
        }
    }

    #[test]
    fn errored_wake_with_banner_on_screen_still_records_pid() {
        // The retry-state rail already pushed the formatted RequestFailed banner for this failure
        // The wake rail paints nothing either way; the pid still records so the other rail stays quiet
        let mut app = make_app_with_agent("sess-wake");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent
                .scrollback
                .push_block(crate::scrollback::block::RenderBlock::session_event(
                    SessionEvent::RequestFailed {
                        status: Some(400),
                        headline: "Bad request (400)".into(),
                        detail: "The server rejected this request.".into(),
                    },
                ));
        }
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "error", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(agent.scrollback.len(), len_before);
        assert_eq!(
            agent.failed_wake_marker_for.as_deref(),
            Some("task-completed-bg1")
        );
    }

    /// Same on the busy-wake rail (a local turn is running, so the terminal takes the `is_busy` branch instead of `finish_wake_turn`).
    #[test]
    fn errored_wake_during_local_turn_with_banner_on_screen_still_records_pid() {
        use crate::app::agent::AgentState;

        let mut app = make_app_with_agent("sess-wake");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.state = AgentState::TurnRunning;
            agent
                .scrollback
                .push_block(crate::scrollback::block::RenderBlock::session_event(
                    SessionEvent::RequestFailed {
                        status: Some(500),
                        headline: "Server error (500)".into(),
                        detail: String::new(),
                    },
                ));
        }
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "error", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(agent.scrollback.len(), len_before);
        assert_eq!(
            agent.failed_wake_marker_for.as_deref(),
            Some("task-completed-bg1")
        );
    }

    #[test]
    fn goal_terminal_snapshots_epoch_so_next_silent_wake_stays_markerless() {
        // A dirty output epoch made the NEXT silent wake inherit the goal turn's output.
        use crate::app::agent_view::test_fixtures::count_turn_markers;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "goal-summary-g1", 5_000),
            &mut app,
        );
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "goal-summary-g1", "end_turn", false),
            &mut app,
        );
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "end_turn", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent.scrollback.len(),
            len_before,
            "a silent wake after a goal turn must not inherit its output"
        );
        assert_eq!(count_turn_markers(agent), 0);
    }

    #[test]
    fn errored_wake_terminal_during_local_turn_pushes_nothing() {
        // The busy arm never touches the local turn's tracker; an errored wake leaves no row under the local prompt either
        use crate::app::agent::AgentState;

        let mut app = make_app_with_agent("sess-wake");
        app.agents.get_mut(&AgentId(0)).unwrap().session.state = AgentState::TurnRunning;
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        for _ in 0..2 {
            let affected = handle_ext_notification(
                &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "error", false),
                &mut app,
            );
            assert!(!affected);
        }

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(agent.scrollback.len(), len_before);
        assert!(agent.session.state.is_turn_running(), "the local turn is untouched");
        assert!(agent.finished_wake_prompts.contains("task-completed-bg1"));
        assert_eq!(
            agent.failed_wake_marker_for.as_deref(),
            Some("task-completed-bg1")
        );
    }

    #[test]
    fn wake_terminal_during_command_snapshots_epoch_for_next_silent_wake() {
        // A client command (e.g. /compact) skips the wake finish but must not leave the epoch dirty.
        // The next silent wake would claim the skipped wake's output
        use crate::app::agent::{AgentCommand, AgentState};
        use crate::app::agent_view::test_fixtures::count_turn_markers;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        app.agents.get_mut(&AgentId(0)).unwrap().session.state = AgentState::CommandRunning {
            command: AgentCommand::Compact,
            started_at: std::time::Instant::now(),
        };
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "end_turn", false),
            &mut app,
        );
        app.agents.get_mut(&AgentId(0)).unwrap().session.state = AgentState::Idle;
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg2", "end_turn", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent.scrollback.len(),
            len_before,
            "silent wake after a command-skipped terminal must stay markerless"
        );
        assert_eq!(count_turn_markers(agent), 0);
    }

    #[test]
    fn chatty_wake_with_foreign_turn_start_anchor_omits_elapsed() {
        // `turn_start_ms` stamped by another prompt's deltas must not become this wake's elapsed
        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 600_000),
            &mut app,
        );

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg2", "end_turn", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCompleted { elapsed: None })
        ));
    }


    #[test]
    fn silent_cancelled_or_rate_limited_wake_stays_markerless() {
        // Rate limits arrive through the retry notifications instead, matching the real-turn rails
        let mut app = make_app_with_agent("sess-wake");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        for stop_reason in ["cancelled", "rate_limit"] {
            let _ = handle_ext_notification(
                &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", stop_reason, false),
                &mut app,
            );
        }

        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len(),
            len_before,
            "cancelled/rate-limited silent wake terminals push nothing"
        );
    }

    #[test]
    fn chatty_send_now_cancelled_wake_is_markerless() {
        // A wake with output cancelled by send-now must stay silent, the same suppression the other three turn-end rails already apply
        use crate::app::agent_view::test_fixtures::count_turn_markers;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif_with_cancel_trigger(
                "sess-wake",
                "task-completed-bg1",
                "cancelled",
                "send_now",
            ),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent.scrollback.len(),
            len_before,
            "a send-now cancelled chatty wake must push no marker"
        );
        assert_eq!(count_turn_markers(agent), 0);
        assert!(
            !matches!(
                last_session_event(&agent.scrollback),
                Some(SessionEvent::TurnCancelled { .. })
            ),
            "send_now must not surface as Turn cancelled by user"
        );
    }

    #[test]
    fn chatty_user_cancelled_wake_pushes_cancelled_marker() {
        // Genuine cancel (Ctrl+C / Esc, no wire trigger) still shows the marker.
        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "cancelled", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCancelled { .. })
        ));
    }

    #[test]
    fn foreign_send_now_arm_does_not_suppress_wake_cancel_marker() {
        // A flag armed for a different (user) prompt must not eat this wake's genuine cancel marker, and must stay armed after close-out
        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .expect_send_now_cancel = Some("user-prompt-other".into());

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "cancelled", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCancelled { .. })
        ));
        assert_eq!(
            agent.expect_send_now_cancel.as_deref(),
            Some("user-prompt-other"),
            "wake close-out must not clear a foreign send-now arm"
        );
    }

    #[test]
    fn chatty_rate_limited_wake_closes_with_failure_marker() {
        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "rate_limit", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnFailed { .. })
        ));
    }

    #[test]
    fn chatty_errored_wake_closes_without_marker_and_clears_wake_state() {
        // Streamed output does not earn an errored wake a row: no "Worked for", no failure; the tracker still finishes
        use crate::app::agent_view::test_fixtures::count_turn_markers;

        let mut app = make_app_with_agent("sess-wake");
        let _ = handle(
            make_viewer_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        assert!(app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).running_wake_turn.is_some());

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "error", false),
            &mut app,
        );
        assert!(affected, "the wake back-to-idle point still redraws");

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(count_turn_markers(agent), 0);
        assert!(agent.running_wake_turn.is_none());
        assert!(agent.session.state.is_idle());
        assert!(
            !agent.session.tracker.output_since_last_finish(),
            "the errored wake's output must not leak into the next silent wake"
        );
    }

    #[test]
    fn dead_wake_pushes_no_status_line() {
        let mut app = make_app_with_agent("sess-wake");
        seed_two_bg_tasks(&mut app, "sess-wake");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "cancelled", false),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            work_status_lines(&agent.scrollback).is_empty(),
            "a dead wake must not push a work-only status line"
        );
        assert_eq!(
            agent.scrollback.len(),
            len_before,
            "a dead wake pushes nothing"
        );
        assert_eq!(
            agent.watchers().commands,
            2,
            "the still-running work feeds the status-row cue instead"
        );
    }

    #[test]
    fn wake_terminal_during_local_turn_pushes_nothing() {
        // FIFO can deliver a wake's terminal after a fresh local prompt starts; a foreign "Worked for" under that prompt would misattribute
        let mut app = make_app_with_agent("sess-wake");
        seed_two_bg_tasks(&mut app, "sess-wake");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-local".into());
        }
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();

        let affected = handle_ext_notification(
            &a3s_wake_turn_completed_notif("sess-wake", "task-completed-bg1", Some(6_000)),
            &mut app,
        );

        assert!(!affected);
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent.scrollback.len(),
            len_before,
            "no marker and no status line may land under the fresh local prompt"
        );
        assert!(
            agent.session.state.is_turn_running(),
            "the local turn is untouched"
        );
    }

    #[test]
    fn between_turns_completion_pushes_chip_only() {
        let mut app = make_app_with_agent("sess-chip-only");
        seed_two_bg_tasks(&mut app, "sess-chip-only");
        assert!(app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).session.state.is_idle());
        assert_eq!(app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).watchers().commands, 2);

        let _ = handle_ext_notification(
            &make_task_completed_notif("sess-chip-only", "task-1", "sleep 98", Some(0)),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            work_status_lines(&agent.scrollback).is_empty(),
            "no work-only status line after a between-turns completion"
        );
        assert_eq!(
            agent.watchers().commands,
            1,
            "the status-row cue counts down instead"
        );

        let _ = handle_ext_notification(
            &make_task_completed_notif("sess-chip-only", "task-2", "sleep 99", Some(0)),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(work_status_lines(&agent.scrollback).is_empty());
        assert_eq!(agent.watchers().commands, 0, "zero left — cue disappears");
    }

    #[test]
    fn mid_turn_completion_pushes_chip_only() {
        let mut app = make_app_with_agent("sess-midturn");
        seed_two_bg_tasks(&mut app, "sess-midturn");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("p1".into());
        }

        let _ = handle_ext_notification(
            &make_task_completed_notif("sess-midturn", "task-1", "sleep 98", Some(0)),
            &mut app,
        );
        assert!(
            work_status_lines(&app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback).is_empty(),
            "a completion inside an active turn pushes its chip only"
        );
    }

    #[test]
    fn subagent_finished_between_turns_pushes_no_status_line() {
        let mut app = make_app_with_parent_and_child("sess-sub-quiet", "child-1");
        let _ = handle_ext_notification(
            &make_task_backgrounded_notif("sess-sub-quiet", "tc-1", "task-1", "sleep 98"),
            &mut app,
        );

        let _ = handle(
            make_ext_session_notification("sess-sub-quiet", test_subagent_finished("child-1")),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            work_status_lines(&agent.scrollback).is_empty(),
            "a finished subagent pushes no work-only status line"
        );
        assert_eq!(
            agent.watchers().commands,
            1,
            "the remaining bg command stays on the status-row cue"
        );
    }

    #[test]
    fn will_wake_flag_is_ignored_wire_compat_pin() {
        // `will_wake` is a wire-compat field the TUI no longer reads.
        let mut app = make_app_with_agent("sess-wake-skip");
        seed_two_bg_tasks(&mut app, "sess-wake-skip");

        let _ = handle_ext_notification(
            &task_completed_notif("sess-wake-skip", "task-1", "sleep 98", Some(0), None, true),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            work_status_lines(&agent.scrollback).is_empty(),
            "a wake-bound completion pushes its chip only"
        );
    }

    #[test]
    fn child_session_completions_never_spam_root_status() {
        // A background subagent's own task traffic routes to the CHILD view
        // It never counts toward the root's watchers, so its completions must not push root status lines
        let mut app = make_app_with_parent_and_child("sess-child-quiet", "child-1");
        let _ = handle_ext_notification(
            &make_task_backgrounded_notif("child-1", "tc-c1", "task-c1", "sleep 97"),
            &mut app,
        );
        let _ = handle_ext_notification(
            &make_task_backgrounded_notif("child-1", "tc-c2", "task-c2", "sleep 98"),
            &mut app,
        );
        assert!(app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).session.state.is_idle());

        let _ = handle_ext_notification(
            &make_task_completed_notif("child-1", "task-c1", "sleep 97", Some(0)),
            &mut app,
        );
        let _ = handle_ext_notification(
            &make_task_completed_notif("child-1", "task-c2", "sleep 98", Some(0)),
            &mut app,
        );

        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            work_status_lines(&agent.scrollback).is_empty(),
            "child-session completions must not spawn root status lines"
        );
        let child = agent.subagent_views.get("child-1").unwrap();
        assert!(
            work_status_lines(&child.scrollback).is_empty(),
            "and none in the child view either (chips only)"
        );

        // Nested analogue: a SubagentFinished carrying a CHILD session id routes to the child handler, which has no status site
        let _ = handle(
            make_ext_session_notification("child-1", test_subagent_finished("grandchild-1")),
            &mut app,
        );
        assert!(
            work_status_lines(&app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback).is_empty(),
            "nested subagent traffic must not spawn root status lines"
        );
    }

    /// The core reattach-finalization: a `TurnCompleted` seen during a load's replay window records its prompt id.
    /// The post-replay `SessionLoaded` adoption then SKIPS that same id.
    /// A viewer that re-attached after the turn ended does not re-strand on "Waiting…".
    #[test]
    fn replayed_turn_completed_blocks_session_loaded_adoption() {
        use crate::app::dispatch::dispatch;
        use crate::app::actions::{Action, TaskResult};

        let mut app = make_app_with_agent("sess-1");
        let id = AgentId(0);
        app.agents.get_mut(&id).unwrap().session.loading_replay = true;

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif("sess-1", "p-run", "end_turn", true),
            &mut app,
        );
        assert!(
            !affected,
            "a replayed terminal records adoption state, not a redraw"
        );
        assert!(
            app.agents.get(&id).unwrap_or_else(|| panic!("missing map entry")).replayed_terminal_prompts.contains("p-run"),
            "a replayed TurnCompleted must record its prompt id"
        );

        dispatch(
            Action::TaskComplete(TaskResult::SessionLoaded {
                agent_id: id,
                session_id: acp::SessionId::new("sess-1"),
                models: None,
                modes: None,
                code_restored: false,
                restore_summary: None,
                restore_degree: None,
                running_prompt_id: Some("p-run".to_string()),
            }),
            &mut app,
        );

        let agent = &app.agents.get(&id).unwrap_or_else(|| panic!("missing map entry"));
        assert!(
            agent.session.current_prompt_id.is_none(),
            "a terminal-in-replay prompt must NOT be adopted on load"
        );
        assert!(
            agent.session.state.is_idle(),
            "adopting an already-ended turn would re-strand the viewer on Waiting…"
        );
    }

    /// Arming the lost-RPC reconcile from a live `TurnCompleted` must STILL report a change.
    /// Otherwise `event_loop` skips `schedule_tick` and `reconcile_overdue_turn_ends` never fires, stranding the turn on "Waiting…".
    /// The reconcile-arm return must NOT be gated on `is_active`.
    #[test]
    fn background_driver_live_turn_completed_arms_reconcile_and_reports_change() {
        let mut app = make_app_with_agent("sess-bg");
        let id = AgentId(0);
        {
            let agent = app.agents.get_mut(&id).unwrap();
            agent.session.start_turn(&mut agent.scrollback);
            agent.session.current_prompt_id = Some("pid-bg".into());
            assert!(!agent.attached_as_viewer);
        }
        // Make the driver a background tab: the active view is elsewhere.
        app.active_view = ActiveView::Welcome;
        assert!(!is_matched_agent_active(&app, id));

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif("sess-bg", "pid-bg", "cancelled", false),
            &mut app,
        );
        assert!(
            affected,
            "a background driver's reconcile-arm must report a change so the tick is scheduled"
        );
        let agent = app.agents.get(&id).unwrap();
        assert!(
            agent.pending_turn_end_reconcile.is_some(),
            "the lost-RPC reconcile must be armed"
        );
        assert!(
            matches!(agent.session.state, AgentState::TurnRunning),
            "arming must NOT finish the driver's turn"
        );
    }

    /// The replay set never leaks across loads.
    /// A second load enters a fresh replay window via `begin_replay_window`.
    /// That resets ALL coupled fields (the terminal set AND `unexpected_replay_drops`) together.
    #[test]
    fn second_load_does_not_inherit_first_loads_replay_window_state() {
        let mut app = make_app_with_agent("sess-1");
        let id = AgentId(0);
        // First load replay records a terminal; also seed a prior stray-replay drop count so the reset of every coupled field is observable
        {
            let agent = app.agents.get_mut(&id).unwrap();
            agent.session.loading_replay = true;
            agent.unexpected_replay_drops = 3;
        }
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-1", "p-first", "end_turn", true),
            &mut app,
        );
        assert!(
            app.agents.get(&id).unwrap_or_else(|| panic!("missing map entry"))
                .replayed_terminal_prompts
                .contains("p-first")
        );

        // A second load (reconnect) enters a fresh replay window
        // An armed cancel resend belongs to the pre-reload turn and must drop with it
        app.agents.get_mut(&id).unwrap().pending_cancel_resend =
            Some(crate::app::agent_view::PendingCancelResend {
                prompt_id: Some("p-first".into()),
                sent_at: std::time::Instant::now(),
                attempts: 1,
                confirmed: false,
                cancel_subagents: true,
                trigger: crate::app::actions::CancelTrigger::DashboardStop,
            });
        app.agents.get_mut(&id).unwrap().begin_session_reload(1);
        let agent = &app.agents.get(&id).unwrap_or_else(|| panic!("missing map entry"));
        assert!(
            agent.replayed_terminal_prompts.is_empty(),
            "the second load must not inherit the first load's terminal set"
        );
        assert_eq!(
            agent.unexpected_replay_drops, 0,
            "begin_replay_window must reset every replay-coupled field together"
        );
        assert!(
            agent.pending_cancel_resend.is_none(),
            "an armed cancel resend must not survive into the reload window"
        );
        assert!(agent.session.loading_replay);
    }


    fn begin_replay(app: &mut AppView) {
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .session
            .loading_replay = true;
    }

    #[test]
    fn replay_completed_with_elapsed_pushes_worked_for() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "end_turn",
                Some(2500),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p1"));
        match last_session_event(&agent.scrollback) {
            Some(SessionEvent::TurnCompleted { elapsed: Some(d) }) => {
                assert_eq!(d, std::time::Duration::from_millis(2500));
            }
            other => panic!("expected Worked-for marker, got {other:?}"),
        }
        assert!(agent.turn_started_at.is_none());
        assert!(agent.expect_send_now_cancel.is_none());
    }

    #[test]
    fn replay_completed_without_elapsed_is_turn_completed_not_zero() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-1", "p1", "end_turn", true),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        match last_session_event(&agent.scrollback) {
            Some(ev @ SessionEvent::TurnCompleted { elapsed: None }) => {
                assert_eq!(ev.message(), "Turn completed.");
            }
            other => panic!("expected markerless-elapsed completed, got {other:?}"),
        }
    }

    #[test]
    fn replay_cancelled_pushes_cancelled_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "cancelled",
                Some(800),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCancelled {
                cause: crate::scrollback::blocks::CancelledBy::Unspecified,
                ..
            })
        ));
    }

    #[test]
    fn replay_session_close_names_session_closed() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "cancelled",
                Some(800),
                None,
                serde_json::json!({ "cancelTrigger": "session_close" }),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        match last_session_event(&agent.scrollback) {
            Some(ev @ SessionEvent::TurnCancelled { .. }) => {
                assert_eq!(
                    ev.message(),
                    "Turn cancelled because the session closed in 0.8s."
                );
            }
            other => panic!("expected named session-close cancel, got {other:?}"),
        }
    }

    #[test]
    fn replay_hook_denied_pushes_blocked_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "cancelled",
                Some(400),
                None,
                serde_json::json!({ "cancellationCategory": "HookDenied" }),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnBlockedByHook { .. })
        ));
    }

    #[test]
    fn replay_failed_without_banner_pushes_failed_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "error",
                Some(100),
                Some("boom"),
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnFailed { .. })
        ));
    }

    #[test]
    fn replay_failed_error_kind_renders_truncation_copy() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_failed_with_error_kind(
                "sess-1",
                "p1",
                "turn ended early",
                "max_tokens_truncation",
                true,
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        match last_session_event(&agent.scrollback) {
            Some(SessionEvent::TurnFailed { error, .. }) => {
                assert_eq!(error, "Response truncated: turn ended early")
            }
            other => panic!("expected TurnFailed, got {other:?}"),
        }
    }

    #[test]
    fn replay_failed_with_banner_skips_failed_marker() {
        let mut app = make_app_with_agent("sess-1");
        {
            let agent = app.agents.get_mut(&AgentId(0)).unwrap();
            agent.scrollback.push_block(
                crate::scrollback::block::RenderBlock::session_event(SessionEvent::RequestFailed {
                    status: Some(400),
                    headline: "Bad request (400)".into(),
                    detail: "The server rejected this request.".into(),
                }),
            );
        }
        begin_replay(&mut app);
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "error",
                Some(100),
                Some("boom"),
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p1"));
        assert_eq!(agent.scrollback.len(), len_before);
    }

    #[test]
    fn replay_rate_limit_records_pid_no_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "rate_limit",
                Some(50),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p1"));
        assert_eq!(agent.scrollback.len(), len_before);
    }

    #[test]
    fn replay_chatty_rate_limited_wake_paints_failure_marker() {
        // Live `finish_wake_turn` paints TurnFailed for a chatty rate-limited wake; replay must not drop that footer
        let mut app = make_app_with_agent("sess-wake");
        begin_replay(&mut app);
        let _ = handle(
            make_replay_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-wake",
                "task-completed-bg1",
                "rate_limit",
                Some(50),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("task-completed-bg1"));
        assert_eq!(agent.scrollback.len(), len_before + 1);
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnFailed { .. })
        ));
        assert_eq!(
            agent.failed_wake_marker_for.as_deref(),
            Some("task-completed-bg1")
        );
    }

    #[test]
    fn replay_silent_rate_limited_wake_stays_markerless() {
        let mut app = make_app_with_agent("sess-wake");
        begin_replay(&mut app);
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "rate_limit", true),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("task-completed-bg1"));
        assert_eq!(agent.scrollback.len(), len_before);
        assert!(agent.failed_wake_marker_for.is_none());
    }

    #[test]
    fn replay_send_now_records_pid_no_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "cancelled",
                Some(50),
                None,
                serde_json::json!({ "cancelTrigger": "send_now" }),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p1"));
        assert_eq!(agent.scrollback.len(), len_before);
    }

    #[test]
    fn replay_unknown_stop_reason_pushes_completed_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "brand_new_token",
                Some(10),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p1"));
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCompleted { .. })
        ));
    }

    #[test]
    fn replay_duplicate_pid_one_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let n = a3s_turn_completed_replay(
            "sess-1",
            "p1",
            "end_turn",
            Some(10),
            None,
            serde_json::json!({}),
        );
        let _ = handle_ext_notification(&n, &mut app);
        let _ = handle_ext_notification(&n, &mut app);
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            crate::app::agent_view::test_fixtures::count_turn_markers(agent),
            1
        );
    }

    #[test]
    fn replay_two_pids_two_markers() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p1",
                "end_turn",
                Some(10),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p2",
                "end_turn",
                Some(20),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p1"));
        assert!(agent.replayed_terminal_prompts.contains("p2"));
        assert_eq!(
            crate::app::agent_view::test_fixtures::count_turn_markers(agent),
            2
        );
    }

    #[test]
    fn replay_silent_wake_records_pid_no_marker() {
        let mut app = make_app_with_agent("sess-wake");
        begin_replay(&mut app);
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-wake", "task-completed-bg1", "end_turn", true),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("task-completed-bg1"));
        assert_eq!(agent.scrollback.len(), len_before);
    }

    #[test]
    fn replay_wake_suppressed_tool_only_records_pid_no_marker() {
        let mut app = make_app_with_agent("sess-wake");
        begin_replay(&mut app);
        send_replay_suppressed_tool_call(&mut app, "sess-wake", "task-completed-bg1");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-wake",
                "task-completed-bg1",
                "end_turn",
                Some(10),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("task-completed-bg1"));
        assert!(
            !agent.replayed_visible_prompts.contains("task-completed-bg1"),
            "a suppressed TodoWrite must not count as visible output"
        );
        assert_eq!(agent.scrollback.len(), len_before);
    }

    #[test]
    fn replay_chatty_errored_wake_records_pid_no_marker() {
        // Visible output earns a replayed wake a "Worked for"; an errored one closes silently like live
        let mut app = make_app_with_agent("sess-wake");
        begin_replay(&mut app);
        let _ = handle(
            make_replay_chunk_with_turn_start("sess-wake", "task-completed-bg1", 5_000),
            &mut app,
        );
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-wake",
                "task-completed-bg1",
                "error",
                Some(100),
                Some("[unknown] Premature close"),
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("task-completed-bg1"));
        assert!(agent.replayed_visible_prompts.contains("task-completed-bg1"));
        assert_eq!(agent.scrollback.len(), len_before);
        assert_eq!(
            agent.failed_wake_marker_for.as_deref(),
            Some("task-completed-bg1")
        );
    }

    #[test]
    fn replay_goal_summary_error_stays_markerless() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "goal-summary-g1",
                "error",
                Some(10),
                Some("classifier failed"),
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("goal-summary-g1"));
        assert_eq!(agent.scrollback.len(), len_before);
    }

    #[test]
    fn replay_direct_bash_records_pid_no_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        send_replay_bash_tool_call(&mut app, "sess-1", "p-bash");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p-bash",
                "end_turn",
                Some(10),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p-bash"));
        assert!(agent.replayed_bash_prompts.contains("p-bash"));
        assert_eq!(
            agent.scrollback.len(),
            len_before,
            "direct bash must not paint a Worked-for marker"
        );
    }

    #[test]
    fn replay_direct_bash_cancelled_paints_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        send_replay_bash_tool_call(&mut app, "sess-1", "p-bash");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p-bash",
                "cancelled",
                Some(10),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p-bash"));
        assert!(agent.replayed_bash_prompts.contains("p-bash"));
        assert_eq!(
            agent.scrollback.len(),
            len_before + 1,
            "cancelled direct bash must paint TurnCancelled"
        );
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnCancelled { .. })
        ));
    }

    #[test]
    fn replay_direct_bash_error_paints_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        send_replay_bash_tool_call(&mut app, "sess-1", "p-bash");
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "p-bash",
                "error",
                Some(10),
                Some("boom"),
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains("p-bash"));
        assert!(agent.replayed_bash_prompts.contains("p-bash"));
        assert_eq!(
            agent.scrollback.len(),
            len_before + 1,
            "failed direct bash must paint TurnFailed"
        );
        assert!(matches!(
            last_session_event(&agent.scrollback),
            Some(SessionEvent::TurnFailed { .. })
        ));
    }

    #[test]
    fn replay_empty_prompt_id_records_no_marker() {
        let mut app = make_app_with_agent("sess-1");
        begin_replay(&mut app);
        let len_before = app.agents.get(&AgentId(0)).unwrap_or_else(|| panic!("missing map entry")).scrollback.len();
        let _ = handle_ext_notification(
            &a3s_turn_completed_replay(
                "sess-1",
                "",
                "end_turn",
                Some(10),
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        let agent = app.agents.get(&AgentId(0)).unwrap();
        assert!(agent.replayed_terminal_prompts.contains(""));
        assert_eq!(agent.scrollback.len(), len_before);
    }

    /// Builds a live `LastTurnSummary` notification.
    fn a3s_last_turn_summary_notif(
        session_id: &str,
        summary: &str,
        prompt_id: Option<&str>,
    ) -> acp::ExtNotification {
        let payload = SessionNotification {
            session_id: acp::SessionId::new(session_id),
            update: A3sSessionUpdate::LastTurnSummary {
                summary: summary.into(),
                prompt_id: prompt_id.map(String::from),
            },
            meta: None,
        };
        acp::ExtNotification::new(
            "x.ai/session/update",
            std::sync::Arc::from(serde_json::value::to_raw_value(&payload).unwrap()),
        )
    }

    /// Show-until-replaced: a summary stays on the row across a later cancelled turn (the shell generates none for it).
    /// It survives turn start/finish untouched and is replaced by the next delivery.
    /// Viewer-mode, mirroring `live_turn_completed_finalizes_viewer_turn`.
    #[test]
    fn last_turn_summary_shows_until_replaced() {
        let mut app = make_app_with_agent("sess-lts");
        app.agents.get_mut(&AgentId(0)).unwrap().attached_as_viewer = true;

        // Turn A runs, completes, and its summary arrives.
        let _ = handle(
            make_agent_chunk_message_with_prompt("sess-lts", "chunk", "pid-a", false),
            &mut app,
        );
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-lts", "pid-a", "end_turn", false),
            &mut app,
        );
        let affected = handle_ext_notification(
            &a3s_last_turn_summary_notif("sess-lts", "Did the thing", Some("pid-a")),
            &mut app,
        );
        assert!(affected);
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().last_turn_summary.as_deref(),
            Some("Did the thing")
        );

        // Turn B runs and is cancelled (no replacement summary): A's summary stays; the row keeps showing the last successful turn's work
        let _ = handle(
            make_agent_chunk_message_with_prompt("sess-lts", "chunk", "pid-b", false),
            &mut app,
        );
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-lts", "pid-b", "cancelled", false),
            &mut app,
        );
        assert!(app.agents.get(&AgentId(0)).unwrap().session.state.is_idle());
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().last_turn_summary.as_deref(),
            Some("Did the thing"),
            "a cancelled turn must not blank the previous summary"
        );

        // Turn C succeeds; its summary replaces A's.
        let _ = handle(
            make_agent_chunk_message_with_prompt("sess-lts", "chunk", "pid-c", false),
            &mut app,
        );
        let _ = handle_ext_notification(
            &a3s_turn_completed_notif("sess-lts", "pid-c", "end_turn", false),
            &mut app,
        );
        let affected = handle_ext_notification(
            &a3s_last_turn_summary_notif("sess-lts", "Did the next thing", Some("pid-c")),
            &mut app,
        );
        assert!(affected);
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().last_turn_summary.as_deref(),
            Some("Did the next thing")
        );
    }

    fn insert_cancelling_child(app: &mut AppView, child_sid: &str) {
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnCancelling;
        assert!(child.session.current_prompt_id.is_none());
        assert!(!child.attached_as_viewer);
        child.pending_cancel_resend = Some(crate::app::agent_view::PendingCancelResend {
            prompt_id: None,
            sent_at: std::time::Instant::now() - crate::app::dispatch::CANCEL_RESEND_GRACE,
            attempts: 1,
            confirmed: false,
            cancel_subagents: true,
            trigger: crate::app::actions::CancelTrigger::CtrlC,
        });
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));
    }

    fn assert_child_idle_with_turn_cancelled(app: &AppView, child_sid: &str) {
        let child = app.agents.get(&AgentId(0)).unwrap()
            .subagent_views
            .get(child_sid)
            .expect("child view");
        assert!(
            matches!(child.session.state, AgentState::Idle),
            "child terminal must clear the Cancelling spinner, not leave or restart a running turn, got {:?}",
            child.session.state
        );
        assert!(!child.any_cancel_pending());
        assert!(!child.attached_as_viewer);
        assert!(child.pending_turn_end_reconcile.is_none());
        assert_eq!(child.scrollback.len(), 1, "one marker, not a second spinner block");
        assert!(!child.scrollback.needs_animation());
        match last_session_event(&child.scrollback) {
            Some(SessionEvent::TurnCancelled { .. }) => {}
            other => panic!("expected TurnCancelled, got {other:?}"),
        }
    }

    #[test]
    fn child_turn_completed_cancelled_clears_turn_cancelling() {
        let mut app = make_app_with_agent("sess-parent");
        {
            let parent = app.agents.get_mut(&AgentId(0)).unwrap();
            parent.session.start_turn(&mut parent.scrollback);
            parent.session.current_prompt_id = Some("pid-child".into());
        }
        let child_sid = "child-cancel";
        insert_cancelling_child(&mut app, child_sid);
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .get_mut(child_sid)
            .unwrap()
            .session
            .current_prompt_id = Some("pid-child".into());
        let parent_len = app.agents.get(&AgentId(0)).unwrap().scrollback.len();

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-child", "cancelled", false),
            &mut app,
        );
        assert!(affected, "finalizing the open child view must redraw");
        assert_child_idle_with_turn_cancelled(&app, child_sid);
        {
            let parent = app.agents.get(&AgentId(0)).unwrap();
            assert!(
                matches!(parent.session.state, AgentState::TurnRunning),
                "child TurnCompleted must not finish the parent turn"
            );
            assert_eq!(parent.session.current_prompt_id.as_deref(), Some("pid-child"));
            assert!(
                parent.pending_turn_end_reconcile.is_none(),
                "child TurnCompleted must not arm the parent reconcile"
            );
            assert_eq!(parent.scrollback.len(), parent_len);
        }
        assert!(
            crate::app::dispatch::reconcile_overdue_cancels(&mut app).is_none(),
            "leaving TurnCancelling must stop the cancel resend"
        );
        assert!(
            app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap()
                .pending_cancel_resend
                .is_none()
        );

        let len_before = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len();
        let affected = handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-child", "cancelled", false),
            &mut app,
        );
        assert!(!affected, "a duplicate child TurnCompleted must be a no-op");
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len(),
            len_before,
            "a duplicate child TurnCompleted must not push another marker"
        );
        assert_child_idle_with_turn_cancelled(&app, child_sid);
    }

    #[test]
    fn child_turn_completed_on_idle_view_pushes_no_marker() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-idle";
        let child = make_agent(Some(child_sid));
        assert!(child.session.state.is_idle());
        assert!(child.session.current_prompt_id.is_none());
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-late", "cancelled", false),
            &mut app,
        );
        assert!(!affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.is_idle());
        assert_eq!(child.scrollback.len(), 0);
        assert!(last_session_event(&child.scrollback).is_none());
    }

    #[test]
    fn child_turn_completed_during_replay_does_not_finalize() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-replay";
        insert_cancelling_child(&mut app, child_sid);
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .get_mut(child_sid)
            .unwrap()
            .session
            .loading_replay = true;

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-child", "cancelled", false),
            &mut app,
        );
        assert!(!affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(
            matches!(child.session.state, AgentState::TurnCancelling),
            "replay must not apply a live child terminal"
        );
        assert_eq!(child.scrollback.len(), 0);
    }

    #[test]
    fn child_turn_completed_does_not_tear_down_in_flight_command() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-cmd";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::CommandRunning {
            command: crate::app::agent::AgentCommand::Compact,
            started_at: std::time::Instant::now(),
        };
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-child", "cancelled", false),
            &mut app,
        );
        assert!(!affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.command_in_flight().is_some());
        assert_eq!(child.scrollback.len(), 0);
    }

    #[test]
    fn child_prompt_complete_cancelled_clears_turn_cancelling() {
        let mut app = make_app_with_agent("sess-parent");
        {
            let parent = app.agents.get_mut(&AgentId(0)).unwrap();
            parent.session.start_turn(&mut parent.scrollback);
            parent.session.current_prompt_id = Some("parent-pid".into());
        }
        let child_sid = "child-prompt-complete";
        insert_cancelling_child(&mut app, child_sid);

        let affected = handle_ext_notification(
            &prompt_complete_ext_with_reason(child_sid, "cancelled", None),
            &mut app,
        );
        assert!(affected, "child prompt_complete must clear TurnCancelling");
        {
            let parent = app.agents.get(&AgentId(0)).unwrap();
            assert!(
                matches!(parent.session.state, AgentState::TurnRunning),
                "child prompt_complete must not finish or reconcile the parent turn"
            );
            assert_eq!(parent.session.current_prompt_id.as_deref(), Some("parent-pid"));
            assert!(parent.pending_turn_end_reconcile.is_none());
        }
        assert_child_idle_with_turn_cancelled(&app, child_sid);
        assert!(crate::app::dispatch::reconcile_overdue_cancels(&mut app).is_none());

        let len_before = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len();
        let affected = handle_ext_notification(
            &prompt_complete_ext_with_reason(child_sid, "cancelled", None),
            &mut app,
        );
        assert!(!affected);
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len(),
            len_before
        );
    }

    fn live_child_turn_completed(
        session_id: &str,
        prompt_id: &str,
        stop_reason: &str,
        elapsed_ms: Option<u64>,
        agent_result: Option<&str>,
        error_kind: Option<&str>,
        extra_meta: serde_json::Value,
    ) -> acp::ExtNotification {
        let mut meta = serde_json::json!({ "isReplay": false });
        if let Some(meta_obj) = meta.as_object_mut()
            && let Some(extra) = extra_meta.as_object()
        {
            for (k, v) in extra {
                meta_obj.insert(k.clone(), v.clone());
            }
        }
        let payload = SessionNotification {
            session_id: acp::SessionId::new(session_id),
            update: A3sSessionUpdate::TurnCompleted {
                prompt_id: prompt_id.into(),
                stop_reason: stop_reason.into(),
                agent_result: agent_result.map(str::to_string),
                error_kind: error_kind.map(str::to_string),
                usage: None,
                elapsed_ms,
            },
            meta: Some(meta),
        };
        acp::ExtNotification::new(
            "x.ai/session/update",
            std::sync::Arc::from(serde_json::value::to_raw_value(&payload).unwrap()),
        )
    }

    fn start_parent_turn(app: &mut AppView, prompt_id: &str) {
        let parent = app.agents.get_mut(&AgentId(0)).unwrap();
        parent.session.start_turn(&mut parent.scrollback);
        parent.session.current_prompt_id = Some(prompt_id.into());
    }

    fn assert_parent_turn_untouched(app: &AppView, prompt_id: &str) {
        let parent = app.agents.get(&AgentId(0)).unwrap();
        assert!(
            matches!(parent.session.state, AgentState::TurnRunning),
            "child terminal must not finish the parent turn, got {:?}",
            parent.session.state
        );
        assert_eq!(parent.session.current_prompt_id.as_deref(), Some(prompt_id));
        assert!(parent.pending_turn_end_reconcile.is_none());
    }

    #[test]
    fn child_turn_completed_end_turn_from_turn_running() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-end";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnRunning;
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-end", "end_turn", false),
            &mut app,
        );
        assert!(affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.is_idle());
        assert!(!child.attached_as_viewer);
        assert_eq!(child.scrollback.len(), 1);
        assert!(matches!(
            last_session_event(&child.scrollback),
            Some(SessionEvent::TurnCompleted { .. })
        ));
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    #[test]
    fn child_prompt_complete_end_turn_from_turn_running() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-pc-end";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnRunning;
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let affected = handle_ext_notification(
            &prompt_complete_ext_with_reason(child_sid, "end_turn", None),
            &mut app,
        );
        assert!(affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.is_idle());
        assert!(matches!(
            last_session_event(&child.scrollback),
            Some(SessionEvent::TurnCompleted { .. })
        ));
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    #[test]
    fn child_turn_completed_send_now_suppresses_cancelled_marker() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-send-now";
        insert_cancelling_child(&mut app, child_sid);

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif_with_cancel_trigger(
                child_sid,
                "pid-send-now",
                "cancelled",
                "send_now",
            ),
            &mut app,
        );
        assert!(affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.is_idle());
        assert!(
            last_session_event(&child.scrollback).is_none(),
            "send_now must not push TurnCancelled, got {:?}",
            last_session_event(&child.scrollback)
        );
    }

    #[test]
    fn child_turn_completed_hook_denied_does_not_requeue() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-hook";
        insert_cancelling_child(&mut app, child_sid);
        {
            let child = app
                .agents
                .get_mut(&AgentId(0))
                .unwrap()
                .subagent_views
                .get_mut(child_sid)
                .unwrap();
            child
                .self_originated_prompt_ids
                .push_back("pid-hook".into());
            child.session.in_flight_prompt = Some(InFlightPrompt {
                text: "secret prompt".into(),
                images: Vec::new(),
                scrollback_entry: EntryId::new(1),
                combined_scrollback_entries: Vec::new(),
                chip_elements: Vec::new(),
            });
        }

        let affected = handle_ext_notification(
            &live_child_turn_completed(
                child_sid,
                "pid-hook",
                "cancelled",
                None,
                None,
                None,
                serde_json::json!({ "cancellationCategory": "HookDenied" }),
            ),
            &mut app,
        );
        assert!(affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.is_idle());
        assert!(matches!(
            last_session_event(&child.scrollback),
            Some(SessionEvent::TurnBlockedByHook { .. })
        ));
        assert!(
            child.session.pending_prompts.is_empty(),
            "a child hook deny must not requeue in_flight_prompt"
        );
    }

    #[test]
    fn child_turn_completed_error_kind_renders_typed_copy() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-fail";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnRunning;
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let affected = handle_ext_notification(
            &a3s_turn_completed_failed_with_error_kind(
                child_sid,
                "pid-fail",
                "turn ended early",
                "max_tokens_truncation",
                false,
            ),
            &mut app,
        );
        assert!(affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.is_idle());
        match last_session_event(&child.scrollback) {
            Some(SessionEvent::TurnFailed { error, .. }) => {
                assert_eq!(error, "Response truncated: turn ended early");
            }
            other => panic!("expected typed TurnFailed, got {other:?}"),
        }
    }

    #[test]
    fn child_turn_completed_uses_wire_elapsed_when_clock_unset() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-elapsed";
        insert_cancelling_child(&mut app, child_sid);

        let affected = handle_ext_notification(
            &live_child_turn_completed(
                child_sid,
                "pid-elapsed",
                "cancelled",
                Some(4200),
                None,
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        assert!(affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        match last_session_event(&child.scrollback) {
            Some(ev @ SessionEvent::TurnCancelled { elapsed: Some(d), .. }) => {
                assert_eq!(d, std::time::Duration::from_millis(4200));
                assert!(!ev.message().contains("0.0s"));
            }
            other => panic!("expected wire elapsed, got {other:?}"),
        }
    }

    #[test]
    fn child_turn_completed_without_clock_does_not_render_zero() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-no-clock";
        insert_cancelling_child(&mut app, child_sid);

        let affected = handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-none", "cancelled", false),
            &mut app,
        );
        assert!(affected);
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        match last_session_event(&child.scrollback) {
            Some(ev @ SessionEvent::TurnCancelled { elapsed: None, .. }) => {
                assert_eq!(ev.message(), "Turn cancelled.");
                assert!(!ev.message().contains("0.0s"));
            }
            other => panic!("unset clock must not render 0.0s, got {other:?}"),
        }
    }

    #[test]
    fn child_follow_up_prompt_reenters_turn_running_and_keeps_clock() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-follow";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnRunning;
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        assert!(handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-1", "end_turn", false),
            &mut app,
        ));
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(child.session.state.is_idle());
            assert!(child.ended_child_prompt_ids.contains("pid-1"));
        }

        let _ = handle(
            make_replay_chunk_with_turn_start(child_sid, "pid-replay", 1_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(
                child.session.state.is_idle(),
                "a replayed chunk must not re-enter TurnRunning"
            );
        }

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "parent-message-msg-1", 8_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(
                matches!(child.session.state, AgentState::TurnRunning),
                "a parent-message follow-up must re-enter TurnRunning, got {:?}",
                child.session.state
            );
            assert_eq!(
                child.session.current_prompt_id.as_deref(),
                Some("parent-message-msg-1")
            );
            assert!(child.turn_started_at.is_some());
            assert!(!child.attached_as_viewer);
        }

        let len = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len();
        assert!(!handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-1", "cancelled", false),
            &mut app,
        ));
        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-1", 9_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            assert_eq!(
                child.session.current_prompt_id.as_deref(),
                Some("parent-message-msg-1")
            );
            assert_eq!(child.scrollback.len(), len);
        }

        assert!(handle_ext_notification(
            &prompt_complete_ext_with_prompt_id(child_sid, "parent-message-msg-1", "cancelled"),
            &mut app,
        ));
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(child.session.state.is_idle());
        match last_session_event(&child.scrollback) {
            Some(SessionEvent::TurnCancelled { elapsed: Some(d), .. }) => {
                assert!(
                    d > std::time::Duration::from_secs(1),
                    "prompt_complete must use the back-dated clock, got {d:?}"
                );
            }
            other => panic!("expected back-dated TurnCancelled, got {other:?}"),
        }
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    #[test]
    fn child_live_chunk_while_cancelling_same_id_stays_cancelling() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-cancel-same";
        insert_cancelling_child(&mut app, child_sid);
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .get_mut(child_sid)
            .unwrap()
            .session
            .current_prompt_id = Some("pid-cancel".into());

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-cancel", 3_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(
                matches!(child.session.state, AgentState::TurnCancelling),
                "a live chunk of the cancelling prompt must stay TurnCancelling, got {:?}",
                child.session.state
            );
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-cancel"));
            assert!(child.turn_started_at.is_some());
            assert!(!child.attached_as_viewer);
            assert!(child.any_cancel_pending());
            assert!(child.pending_cancel_resend.is_some());
        }
        assert!(
            crate::app::dispatch::reconcile_overdue_cancels(&mut app).is_some(),
            "staying TurnCancelling must keep the cancel resend"
        );
        assert!(
            app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap()
                .pending_cancel_resend
                .is_some()
        );
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    #[test]
    fn child_live_chunk_while_cancelling_without_prompt_id_stays_cancelling() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-cancel-none";
        insert_cancelling_child(&mut app, child_sid);
        assert!(
            app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap()
                .session
                .current_prompt_id
                .is_none()
        );

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-first", 3_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(
                matches!(child.session.state, AgentState::TurnCancelling),
                "adopting the first chunk's id must not leave TurnCancelling, got {:?}",
                child.session.state
            );
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-first"));
            assert!(child.turn_started_at.is_some());
            assert!(!child.attached_as_viewer);
            assert!(child.any_cancel_pending());
            assert!(child.pending_cancel_resend.is_some());
        }
        assert!(
            crate::app::dispatch::reconcile_overdue_cancels(&mut app).is_some(),
            "staying TurnCancelling must keep the cancel resend"
        );
        assert!(
            app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap()
                .pending_cancel_resend
                .is_some()
        );
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    #[test]
    fn child_live_chunk_of_different_prompt_replaces_cancelling_turn() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-cancel-replace";
        insert_cancelling_child(&mut app, child_sid);
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .get_mut(child_sid)
            .unwrap()
            .session
            .current_prompt_id = Some("pid-old".into());

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-new", 2_000),
            &mut app,
        );
        let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
        assert!(
            matches!(child.session.state, AgentState::TurnRunning),
            "a different not-yet-ended prompt must replace TurnCancelling, got {:?}",
            child.session.state
        );
        assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-new"));
        assert!(!child.attached_as_viewer);
        assert!(
            child.superseded_child_prompt_ids.contains("pid-old"),
            "the id left behind must not be adoptable again"
        );
        assert!(
            !child.ended_child_prompt_ids.contains("pid-old"),
            "leaving an id is not the same as its terminal having marked"
        );
    }

    #[test]
    fn child_old_chunk_after_new_prompt_cannot_steal_current_prompt_id() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-steal";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnRunning;
        child.session.current_prompt_id = Some("pid-old".into());
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-new", 2_000),
            &mut app,
        );
        let started = {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-new"));
            assert!(child.superseded_child_prompt_ids.contains("pid-old"));
            assert!(!child.ended_child_prompt_ids.contains("pid-old"));
            child.turn_started_at
        };
        let len = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len();

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-old", 9_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(
                matches!(child.session.state, AgentState::TurnRunning),
                "the old chunk must not finish or replace the live turn, got {:?}",
                child.session.state
            );
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-new"));
            assert_eq!(child.turn_started_at, started);
            assert_eq!(child.scrollback.len(), len);
            assert!(!child.attached_as_viewer);
        }

        let affected = handle_ext_notification(
            &live_child_turn_completed(
                child_sid,
                "pid-old",
                "end_turn",
                Some(50),
                None,
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        assert!(affected, "the old terminal must still push its marker");
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(
                matches!(child.session.state, AgentState::TurnRunning),
                "the old terminal must not finish the live turn, got {:?}",
                child.session.state
            );
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-new"));
            assert_eq!(child.turn_started_at, started);
            assert!(child.ended_child_prompt_ids.contains("pid-old"));
            assert!(!child.ended_child_prompt_ids.contains("pid-new"));
            assert!(!child.attached_as_viewer);
            assert!(child.pending_turn_end_reconcile.is_none());
            match last_session_event(&child.scrollback) {
                Some(SessionEvent::TurnCompleted { elapsed: Some(d) }) => {
                    assert_eq!(d, std::time::Duration::from_millis(50));
                }
                other => panic!("expected the old prompt's marker, got {other:?}"),
            }
        }
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    #[test]
    fn adopting_newer_child_prompt_finishes_superseded_running_rows() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-supersede-rows";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnRunning;
        child.session.current_prompt_id = Some("pid-old".into());
        seed_pending_tool(&mut child, "tc-old", "bash");
        // Resumed transcript: the tracker never saw this row, so finish_turn alone would leave it spinning.
        child.scrollback.push_block(RenderBlock::thinking_streaming());
        child.scrollback.set_last_running(true);
        assert!(matches!(
            child.session.turn_activity(),
            Some(crate::acp::tracker::TurnActivity::ToolRunning { .. })
        ));
        assert!(scrollback_has_running_tool_or_thinking(&child.scrollback));
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-new", 2_000),
            &mut app,
        );
        let len = {
            let child = child_at(&app, child_sid);
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-new"));
            assert!(child.superseded_child_prompt_ids.contains("pid-old"));
            assert!(
                !child.ended_child_prompt_ids.contains("pid-old"),
                "leaving an id is not the same as its terminal having marked"
            );
            assert!(
                !scrollback_has_running_tool_or_thinking(&child.scrollback),
                "superseded tools and thinking must stop before the follow-up ends"
            );
            assert!(
                matches!(
                    child.session.turn_activity(),
                    Some(crate::acp::tracker::TurnActivity::Responding)
                ),
                "the follow-up chunk must own activity, got {:?}",
                child.session.turn_activity()
            );
            child.scrollback.len()
        };

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-old", 9_000),
            &mut app,
        );
        {
            let child = child_at(&app, child_sid);
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-new"));
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            assert_eq!(child.scrollback.len(), len);
            assert!(!scrollback_has_running_tool_or_thinking(&child.scrollback));
        }

        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = handle(
            AcpClientMessage::SessionNotification(a3s_acp_lib::AcpArgs {
                request: acp::SessionNotification::new(
                    acp::SessionId::new(child_sid),
                    acp::SessionUpdate::ToolCall(
                        acp::ToolCall::new(
                            acp::ToolCallId::new(std::sync::Arc::from("tc-new")),
                            "read".to_owned(),
                        )
                        .kind(acp::ToolKind::Other)
                        .status(acp::ToolCallStatus::Pending)
                        .content(vec![])
                        .locations(vec![]),
                    ),
                )
                .meta(
                    serde_json::json!({
                        "promptId": "pid-new",
                        "isReplay": false,
                    })
                    .as_object()
                    .cloned(),
                ),
                response_tx: tx,
            }),
            &mut app,
        );
        {
            let child = child_at(&app, child_sid);
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-new"));
            assert!(
                (0..child.scrollback.len()).any(|i| {
                    child.scrollback.entry(i).is_some_and(|e| {
                        e.is_running && matches!(e.block, RenderBlock::ToolCall(_))
                    })
                }),
                "the follow-up's own tool must still run"
            );
        }
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    fn scrollback_has_running_tool_or_thinking(
        scrollback: &crate::scrollback::state::ScrollbackState,
    ) -> bool {
        (0..scrollback.len()).any(|i| {
            scrollback.entry(i).is_some_and(|e| {
                e.is_running && matches!(e.block, RenderBlock::ToolCall(_) | RenderBlock::Thinking(_))
            })
        })
    }

    #[test]
    fn child_previous_prompt_terminal_after_next_update_records_ended_id() {
        let mut app = make_app_with_agent("sess-parent");
        start_parent_turn(&mut app, "parent-pid");
        let child_sid = "child-reorder";
        let child = make_agent(Some(child_sid));
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-next", 2_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-next"));
            assert!(child.turn_started_at.is_some());
        }
        let started = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().turn_started_at;

        let affected = handle_ext_notification(
            &live_child_turn_completed(
                child_sid,
                "pid-prev",
                "end_turn",
                Some(50),
                None,
                None,
                serde_json::json!({}),
            ),
            &mut app,
        );
        assert!(affected, "the previous prompt's terminal must still push its marker");
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(
                matches!(child.session.state, AgentState::TurnRunning),
                "a late terminal for the previous prompt must not finish the live turn, got {:?}",
                child.session.state
            );
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-next"));
            assert!(child.ended_child_prompt_ids.contains("pid-prev"));
            assert!(!child.ended_child_prompt_ids.contains("pid-next"));
            assert_eq!(child.turn_started_at, started);
            assert!(!child.attached_as_viewer);
            assert!(child.pending_turn_end_reconcile.is_none());
            match last_session_event(&child.scrollback) {
                Some(SessionEvent::TurnCompleted { elapsed: Some(d) }) => {
                    assert_eq!(d, std::time::Duration::from_millis(50));
                }
                other => panic!("expected the previous prompt's marker, got {other:?}"),
            }
        }

        let len = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len();
        assert!(!handle_ext_notification(
            &a3s_turn_completed_notif(child_sid, "pid-prev", "end_turn", false),
            &mut app,
        ));
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap().scrollback.len(),
            len,
            "a duplicate terminal for the ended id must not mark again"
        );

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-prev", 9_000),
            &mut app,
        );
        {
            let child = app.agents.get(&AgentId(0)).unwrap().subagent_views.get(child_sid).unwrap();
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-next"));
            assert_eq!(child.turn_started_at, started);
            assert_eq!(child.scrollback.len(), len);
        }
        assert_parent_turn_untouched(&app, "parent-pid");
    }

    #[test]
    fn child_turn_completed_hydrates_resumed_transcript_before_marker() {
        with_replay_disk_home(|home| {
            let child_sid = "child-resume-turn-end";
            write_child_updates_jsonl(home, child_sid, &(pending_child_tool_line(child_sid) + "\n"));
            let mut app = make_app_with_agent("sess-parent");
            // Parent still in a turn: replay must not sweep running rows, so the child terminal has to.
            start_parent_turn(&mut app, "parent-pid");
            let mut spawned = test_subagent_spawned("sess-parent", child_sid);
            let A3sSessionUpdate::SubagentSpawned { resumed_from, .. } = &mut spawned else {
                unreachable!();
            };
            *resumed_from = Some("orig-child".into());
            let _ = handle(
                make_ext_session_notification_with_method(
                    "sess-parent",
                    "x.ai/session/update",
                    spawned,
                ),
                &mut app,
            );
            assert_eq!(
                child_scrollback_tool_call_count(app.agents.get(&AgentId(0)).unwrap(), child_sid),
                0
            );

            assert!(handle_ext_notification(
                &a3s_turn_completed_notif(child_sid, "pid-resume", "end_turn", false),
                &mut app,
            ));
            let agent = app.agents.get(&AgentId(0)).unwrap();
            assert_eq!(child_scrollback_tool_call_count(agent, child_sid), 1);
            let child = agent.subagent_views.get(child_sid).unwrap();
            assert!(child.session.state.is_idle());
            let mut tool_idx = None;
            let mut marker_idx = None;
            for i in 0..child.scrollback.len() {
                match child.scrollback.entry(i).map(|e| &e.block) {
                    Some(RenderBlock::ToolCall(_)) if tool_idx.is_none() => tool_idx = Some(i),
                    Some(RenderBlock::SessionEvent(b)) if b.event.is_turn_terminal() => {
                        marker_idx = Some(i);
                    }
                    _ => {}
                }
            }
            assert!(
                tool_idx.zip(marker_idx).is_some_and(|(t, m)| t < m),
                "inherited tool call must precede the terminal marker, tool={tool_idx:?} marker={marker_idx:?}"
            );
            assert!(
                (0..child.scrollback.len())
                    .all(|i| child.scrollback.entry(i).is_some_and(|e| !e.is_running)),
                "a resumed turn-end must finish replayed rows the tracker never saw"
            );
        });
    }

    #[test]
    fn child_prompt_complete_hydrates_resumed_transcript_before_marker() {
        with_replay_disk_home(|home| {
            let child_sid = "child-resume-prompt-complete";
            write_child_updates_jsonl(home, child_sid, &(pending_child_tool_line(child_sid) + "\n"));
            let mut app = make_app_with_agent("sess-parent");
            start_parent_turn(&mut app, "parent-pid");
            let mut spawned = test_subagent_spawned("sess-parent", child_sid);
            let A3sSessionUpdate::SubagentSpawned { resumed_from, .. } = &mut spawned else {
                unreachable!();
            };
            *resumed_from = Some("orig-child".into());
            let _ = handle(
                make_ext_session_notification_with_method(
                    "sess-parent",
                    "x.ai/session/update",
                    spawned,
                ),
                &mut app,
            );

            assert!(handle_ext_notification(
                &prompt_complete_ext_with_reason(child_sid, "end_turn", None),
                &mut app,
            ));
            let agent = app.agents.get(&AgentId(0)).unwrap();
            assert_eq!(
                child_scrollback_tool_call_count(agent, child_sid),
                1,
                "prompt_complete must hydrate through child_view_for_live_update_mut"
            );
            let child = agent.subagent_views.get(child_sid).unwrap();
            assert!(matches!(
                last_session_event(&child.scrollback),
                Some(SessionEvent::TurnCompleted { .. })
            ));
            assert!(
                (0..child.scrollback.len())
                    .all(|i| child.scrollback.entry(i).is_some_and(|e| !e.is_running)),
                "prompt_complete on a resumed child must finish replayed running rows"
            );
        });
    }

    fn pending_child_tool_line(child_sid: &str) -> String {
        format!(
            r#"{{"method":"session/update","params":{{"sessionId":"{child_sid}","update":{{"sessionUpdate":"tool_call","toolCallId":"tc1","title":"Read foo","kind":"read","status":"pending","locations":[{{"path":"/tmp/foo"}}]}}}}}}"#
        )
    }

    #[test]
    fn child_live_chunk_during_command_does_not_clobber_command() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-compact";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::CommandRunning {
            command: crate::app::agent::AgentCommand::Compact,
            started_at: std::time::Instant::now(),
        };
        child.session.current_prompt_id = Some("pid-cmd".into());
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));

        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-chunk", 1_000),
            &mut app,
        );
        let child = app
            .agents
            .get(&AgentId(0))
            .unwrap()
            .subagent_views
            .get(child_sid)
            .unwrap();
        assert!(
            matches!(
                child.session.state,
                AgentState::CommandRunning {
                    command: crate::app::agent::AgentCommand::Compact,
                    ..
                }
            ),
            "a live chunk must not replace an in-flight command, got {:?}",
            child.session.state
        );
        assert_eq!(child.session.current_prompt_id.as_deref(), Some("pid-cmd"));
    }

    // Nov 2023, so a follow-up after this start is still before wall-clock now.
    const CLOSED_TURN_START_MS: i64 = 1_700_000_000_000;

    fn close_nameless_child(
        app: &mut AppView,
        child_sid: &str,
        start_ms: Option<i64>,
        prompt_id: Option<&str>,
    ) {
        insert_cancelling_child(app, child_sid);
        {
            let child = app
                .agents
                .get_mut(&AgentId(0))
                .unwrap()
                .subagent_views
                .get_mut(child_sid)
                .unwrap();
            child.turn_start_ms = start_ms;
            child.turn_start_ms_prompt = prompt_id.map(ToOwned::to_owned);
        }
        assert!(handle_ext_notification(
            &prompt_complete_ext_with_reason(child_sid, "cancelled", None),
            app,
        ));
        assert_child_idle_with_turn_cancelled(app, child_sid);
    }

    fn child_chunk(
        session_id: &str,
        prompt_id: Option<&str>,
        turn_start_ms: i64,
    ) -> AcpClientMessage {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let mut meta = serde_json::json!({
            "isReplay": false,
            "turnStartMs": turn_start_ms,
        });
        if let Some(pid) = prompt_id {
            json_set(&mut meta, "promptId", serde_json::json!(pid));
        }
        let request = acp::SessionNotification::new(
            acp::SessionId::new(session_id),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new("driver chunk"),
            ))),
        )
        .meta(meta.as_object().cloned());
        AcpClientMessage::SessionNotification(a3s_acp_lib::AcpArgs {
            request,
            response_tx: tx,
        })
    }

    fn child_at<'a>(app: &'a AppView, child_sid: &str) -> &'a AgentView {
        app.agents
            .get(&AgentId(0))
            .unwrap()
            .subagent_views
            .get(child_sid)
            .unwrap()
    }

    #[test]
    fn late_chunk_after_unidentified_cancel_does_not_restart() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-late-noid";
        close_nameless_child(&mut app, child_sid, Some(CLOSED_TURN_START_MS), None);
        assert_eq!(
            Some(CLOSED_TURN_START_MS),
            child_at(&app, child_sid).unidentified_child_turn_closed_ms
        );
        let len = child_at(&app, child_sid).scrollback.len();

        let _ = handle(child_chunk(child_sid, None, CLOSED_TURN_START_MS), &mut app);
        {
            let child = child_at(&app, child_sid);
            assert!(
                child.session.state.is_idle(),
                "a chunk with the closed turn's start and no id must not re-enter TurnRunning, got {:?}",
                child.session.state
            );
            assert!(child.session.current_prompt_id.is_none());
            assert_eq!(len, child.scrollback.len());
            assert!(!child.scrollback.needs_animation());
        }

        let follow_start = CLOSED_TURN_START_MS + 30_000;
        let _ = handle(
            child_chunk(child_sid, Some("parent-message-msg-9"), follow_start),
            &mut app,
        );
        {
            let child = child_at(&app, child_sid);
            assert!(
                matches!(child.session.state, AgentState::TurnRunning),
                "a parent-message whose start is after the closed turn must re-enter TurnRunning, got {:?}",
                child.session.state
            );
            assert_eq!(
                Some("parent-message-msg-9"),
                child.session.current_prompt_id.as_deref()
            );
            assert!(!child.ended_child_prompt_ids.contains("parent-message-msg-9"));
            assert_eq!(Some(follow_start), child.turn_start_ms);
            assert_eq!(
                Some("parent-message-msg-9"),
                child.turn_start_ms_prompt.as_deref()
            );
        }

        let started = child_at(&app, child_sid).turn_started_at;
        let _ = handle(child_chunk(child_sid, None, CLOSED_TURN_START_MS), &mut app);
        let child = child_at(&app, child_sid);
        assert!(
            matches!(child.session.state, AgentState::TurnRunning),
            "a later chunk of the closed start must not replace the follow-up, got {:?}",
            child.session.state
        );
        assert_eq!(
            Some("parent-message-msg-9"),
            child.session.current_prompt_id.as_deref()
        );
        assert_eq!(Some(follow_start), child.turn_start_ms);
        assert_eq!(
            Some("parent-message-msg-9"),
            child.turn_start_ms_prompt.as_deref()
        );
        assert_eq!(started, child.turn_started_at);
        assert!(!child.ended_child_prompt_ids.contains("parent-message-msg-9"));
    }

    #[test]
    fn child_nameless_cancel_without_start_leaves_bound_unset() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-no-start";
        close_nameless_child(&mut app, child_sid, None, None);
        let child = child_at(&app, child_sid);
        assert_eq!(None, child.unidentified_child_turn_closed_ms);
        assert_eq!(None, child.unidentified_child_turn_closed_prompt);
    }

    #[test]
    fn child_rejected_chunk_does_not_overwrite_live_clock() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-clock";
        close_nameless_child(
            &mut app,
            child_sid,
            Some(CLOSED_TURN_START_MS),
            Some("pid-closed"),
        );
        let follow_start = CLOSED_TURN_START_MS + 30_000;
        let _ = handle(
            child_chunk(child_sid, Some("parent-message-msg-1"), follow_start),
            &mut app,
        );
        let (start_ms, start_prompt, started) = {
            let child = child_at(&app, child_sid);
            assert!(matches!(child.session.state, AgentState::TurnRunning));
            (
                child.turn_start_ms,
                child.turn_start_ms_prompt.clone(),
                child.turn_started_at,
            )
        };
        assert_eq!(Some(follow_start), start_ms);
        assert_eq!(Some("parent-message-msg-1"), start_prompt.as_deref());

        let _ = handle(
            child_chunk(child_sid, Some("pid-closed"), CLOSED_TURN_START_MS),
            &mut app,
        );
        {
            let child = child_at(&app, child_sid);
            assert!(
                matches!(child.session.state, AgentState::TurnRunning),
                "the closed id must not replace the live turn, got {:?}",
                child.session.state
            );
            assert_eq!(
                Some("parent-message-msg-1"),
                child.session.current_prompt_id.as_deref()
            );
            assert_eq!(start_ms, child.turn_start_ms);
            assert_eq!(start_prompt, child.turn_start_ms_prompt);
            assert_eq!(started, child.turn_started_at);
            assert!(child.ended_child_prompt_ids.contains("pid-closed"));
        }

        let _ = handle(
            child_chunk(child_sid, Some("pid-closed"), follow_start + 5_000),
            &mut app,
        );
        let _ = handle(child_chunk(child_sid, None, CLOSED_TURN_START_MS), &mut app);
        let child = child_at(&app, child_sid);
        assert_eq!(start_ms, child.turn_start_ms);
        assert_eq!(start_prompt, child.turn_start_ms_prompt);
        assert_eq!(started, child.turn_started_at);
        assert_eq!(
            Some("parent-message-msg-1"),
            child.session.current_prompt_id.as_deref()
        );
    }

    #[test]
    fn child_parent_message_after_nameless_cancel_reenters() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-parent-msg";
        close_nameless_child(
            &mut app,
            child_sid,
            Some(CLOSED_TURN_START_MS),
            Some("pid-closed"),
        );
        {
            let child = child_at(&app, child_sid);
            assert_eq!(Some(CLOSED_TURN_START_MS), child.unidentified_child_turn_closed_ms);
            assert_eq!(None, child.turn_start_ms);
            assert_eq!(
                Some("pid-closed"),
                child.unidentified_child_turn_closed_prompt.as_deref()
            );
        }

        let follow_start = CLOSED_TURN_START_MS + 30_000;
        assert!(follow_start < chrono::Utc::now().timestamp_millis());
        let _ = handle(
            child_chunk(child_sid, Some("parent-message-msg-1"), follow_start),
            &mut app,
        );
        {
            let child = child_at(&app, child_sid);
            assert!(
                matches!(child.session.state, AgentState::TurnRunning),
                "a parent-message after the closed start, still before now, must re-enter TurnRunning, got {:?}",
                child.session.state
            );
            assert_eq!(
                Some("parent-message-msg-1"),
                child.session.current_prompt_id.as_deref()
            );
            assert!(!child.ended_child_prompt_ids.contains("parent-message-msg-1"));
            assert_eq!(Some(CLOSED_TURN_START_MS), child.unidentified_child_turn_closed_ms);
        }

        let early_sid = "child-before-start";
        close_nameless_child(
            &mut app,
            early_sid,
            Some(CLOSED_TURN_START_MS),
            Some("pid-closed"),
        );
        let _ = handle(
            child_chunk(
                early_sid,
                Some("parent-message-msg-early"),
                CLOSED_TURN_START_MS - 30_000,
            ),
            &mut app,
        );
        let child = child_at(&app, early_sid);
        assert!(
            matches!(child.session.state, AgentState::TurnRunning),
            "a different id before the closed start is a new turn, got {:?}",
            child.session.state
        );
        assert_eq!(
            Some("parent-message-msg-early"),
            child.session.current_prompt_id.as_deref()
        );
        assert!(!child.ended_child_prompt_ids.contains("parent-message-msg-early"));
    }

    #[test]
    fn child_closed_turn_start_chunk_is_dropped() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-drop-closed";
        close_nameless_child(
            &mut app,
            child_sid,
            Some(CLOSED_TURN_START_MS),
            Some("pid-closed"),
        );
        let len = child_at(&app, child_sid).scrollback.len();

        let _ = handle(child_chunk(child_sid, None, CLOSED_TURN_START_MS), &mut app);
        {
            let child = child_at(&app, child_sid);
            assert!(
                child.session.state.is_idle(),
                "no id and the closed start must stay idle, got {:?}",
                child.session.state
            );
            assert!(child.session.current_prompt_id.is_none());
            assert_eq!(len, child.scrollback.len());
        }

        let _ = handle(
            child_chunk(child_sid, Some("pid-closed"), CLOSED_TURN_START_MS),
            &mut app,
        );
        {
            let child = child_at(&app, child_sid);
            assert!(
                child.session.state.is_idle(),
                "the closed id and the closed start must stay idle, got {:?}",
                child.session.state
            );
            assert!(child.session.current_prompt_id.is_none());
            assert_eq!(len, child.scrollback.len());
            assert!(child.ended_child_prompt_ids.contains("pid-closed"));
        }

        let same_start_sid = "child-same-start-other-id";
        close_nameless_child(
            &mut app,
            same_start_sid,
            Some(CLOSED_TURN_START_MS),
            Some("pid-closed"),
        );
        let _ = handle(
            child_chunk(
                same_start_sid,
                Some("parent-message-msg-2"),
                CLOSED_TURN_START_MS,
            ),
            &mut app,
        );
        let child = child_at(&app, same_start_sid);
        assert!(
            matches!(child.session.state, AgentState::TurnRunning),
            "a different id sharing the closed start is a new turn, got {:?}",
            child.session.state
        );
        assert!(!child.ended_child_prompt_ids.contains("parent-message-msg-2"));
        assert_eq!(
            Some("parent-message-msg-2"),
            child.session.current_prompt_id.as_deref()
        );
    }

    #[test]
    fn nameless_prompt_complete_finishes_cancelling_turn_after_id_adopted() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-adopt-then-pc";
        insert_cancelling_child(&mut app, child_sid);
        let _ = handle(
            make_viewer_chunk_with_turn_start(child_sid, "pid-adopted", 1_000),
            &mut app,
        );
        {
            let child = child_at(&app, child_sid);
            assert!(matches!(child.session.state, AgentState::TurnCancelling));
            assert_eq!(Some("pid-adopted"), child.session.current_prompt_id.as_deref());
        }
        assert!(handle_ext_notification(
            &prompt_complete_ext_with_reason(child_sid, "cancelled", None),
            &mut app,
        ));
        let child = child_at(&app, child_sid);
        assert!(
            matches!(child.session.state, AgentState::Idle),
            "a nameless prompt_complete must finish the cancelling turn after its id was adopted, got {:?}",
            child.session.state
        );
        assert!(child.ended_child_prompt_ids.contains("pid-adopted"));
        assert!(!child.any_cancel_pending());
        match last_session_event(&child.scrollback) {
            Some(SessionEvent::TurnCancelled { .. }) => {}
            other => panic!("expected TurnCancelled, got {other:?}"),
        }
    }

    #[test]
    fn follow_up_end_keeps_nameless_close_bound() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-keep-bound";
        close_nameless_child(
            &mut app,
            child_sid,
            Some(CLOSED_TURN_START_MS),
            Some("pid-closed"),
        );
        let _ = handle(
            child_chunk(
                child_sid,
                Some("parent-message-msg-9"),
                CLOSED_TURN_START_MS + 30_000,
            ),
            &mut app,
        );
        assert!(handle_ext_notification(
            &prompt_complete_ext_with_prompt_id(child_sid, "parent-message-msg-9", "end_turn"),
            &mut app,
        ));
        {
            let child = child_at(&app, child_sid);
            assert!(child.session.state.is_idle());
            assert_eq!(Some(CLOSED_TURN_START_MS), child.unidentified_child_turn_closed_ms);
            assert_eq!(
                Some("pid-closed"),
                child.unidentified_child_turn_closed_prompt.as_deref()
            );
        }
        let _ = handle(
            child_chunk(child_sid, Some("pid-closed"), CLOSED_TURN_START_MS),
            &mut app,
        );
        let child = child_at(&app, child_sid);
        assert!(
            child.session.state.is_idle(),
            "a late chunk of the closed turn must not restart after the follow-up ends, got {:?}",
            child.session.state
        );
        assert!(child.ended_child_prompt_ids.contains("pid-closed"));
    }

    #[test]
    fn child_turn_end_clears_parent_activity_label() {
        fn label_of(app: &AppView, child_sid: &str) -> Option<String> {
            app.agents
                .get(&AgentId(0))
                .unwrap()
                .subagent_sessions
                .get(child_sid)
                .unwrap()
                .attempt
                .activity_label
                .clone()
        }
        fn block_label(app: &AppView, child_sid: &str) -> Option<String> {
            let agent = app.agents.get(&AgentId(0)).unwrap();
            let entry_id = agent
                .subagent_sessions
                .get(child_sid)
                .unwrap()
                .attempt
                .scrollback_entry_id
                .unwrap();
            let entry = agent.scrollback.get_by_id(entry_id).unwrap();
            let crate::scrollback::block::RenderBlock::Subagent(sb) = &entry.block else {
                panic!("expected Subagent block");
            };
            sb.activity_label.clone()
        }

        let mut app = make_app_with_agent("sess-parent");
        let turn_sid = "child-activity-turn";
        let _ = handle(
            make_ext_session_notification(
                "sess-parent",
                test_subagent_spawned("sess-parent", turn_sid),
            ),
            &mut app,
        );
        let _ = handle(make_agent_chunk_with_event(turn_sid, "hi", "p-child", None), &mut app);
        assert_eq!(Some("Responding"), label_of(&app, turn_sid).as_deref());
        assert!(handle_ext_notification(
            &a3s_turn_completed_notif(turn_sid, "p-child", "end_turn", false),
            &mut app,
        ));
        assert!(
            label_of(&app, turn_sid).is_none(),
            "TurnCompleted must clear the tasks-pane label"
        );
        assert!(
            block_label(&app, turn_sid).is_none(),
            "TurnCompleted must clear the collapsed block label"
        );

        let pc_sid = "child-activity-pc";
        let _ = handle(
            make_ext_session_notification("sess-parent", test_subagent_spawned("sess-parent", pc_sid)),
            &mut app,
        );
        let _ = handle(make_agent_chunk_with_event(pc_sid, "hi", "p-pc", None), &mut app);
        assert_eq!(Some("Responding"), label_of(&app, pc_sid).as_deref());
        assert!(handle_ext_notification(
            &prompt_complete_ext_with_prompt_id(pc_sid, "p-pc", "end_turn"),
            &mut app,
        ));
        assert!(
            label_of(&app, pc_sid).is_none(),
            "prompt_complete must clear the tasks-pane label"
        );
        assert!(
            block_label(&app, pc_sid).is_none(),
            "prompt_complete must clear the collapsed block label"
        );
    }

    #[test]
    fn prompt_complete_without_id_does_not_finish_named_follow_up() {
        let mut app = make_app_with_agent("sess-parent");
        let child_sid = "child-stale-pc";
        let mut child = make_agent(Some(child_sid));
        child.session.state = AgentState::TurnRunning;
        child.session.current_prompt_id = Some("parent-message-msg-2".into());
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .subagent_views
            .insert(child_sid.to_string(), Box::new(child));
        let len = app
            .agents
            .get(&AgentId(0))
            .unwrap()
            .subagent_views
            .get(child_sid)
            .unwrap()
            .scrollback
            .len();

        assert!(!handle_ext_notification(
            &prompt_complete_ext(child_sid),
            &mut app,
        ));
        let child = app
            .agents
            .get(&AgentId(0))
            .unwrap()
            .subagent_views
            .get(child_sid)
            .unwrap();
        assert!(
            matches!(child.session.state, AgentState::TurnRunning),
            "a nameless prompt_complete must not finish a turn that already has an id, got {:?}",
            child.session.state
        );
        assert_eq!(
            child.session.current_prompt_id.as_deref(),
            Some("parent-message-msg-2")
        );
        assert_eq!(child.scrollback.len(), len);
    }
