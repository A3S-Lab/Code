#![cfg_attr(rustfmt, rustfmt::skip)]
    use super::*;

    #[test]
    fn acp_chunk_for_inactive_agent_lands_in_its_scrollback() {
        // Regression: switching away from a streaming agent must not discard chunks bound for that agent
        // Only `TaskResult::PromptResponse` once survived, so the user saw a bare "Worked for X.Xs" with no body text
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        switch_active_to(&mut app, AgentId(1));

        let affected = handle(make_agent_chunk_message("sess-A", "hello from A"), &mut app);

        let agent_a = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent_message_text(agent_a),
            "hello from A",
            "chunk for inactive agent A must land in A's scrollback"
        );
        assert!(
            !affected,
            "chunk routed to a non-active agent must not request a redraw"
        );
        let agent_b = app.agents.get(&AgentId(1)).unwrap();
        assert!(
            agent_b.scrollback.is_empty(),
            "active agent B's scrollback must remain untouched"
        );
    }

    #[test]
    fn acp_chunk_for_active_agent_returns_affected_true() {
        // Baseline: chunk for the visible agent triggers a redraw.
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        switch_active_to(&mut app, AgentId(1));

        let affected = handle(make_agent_chunk_message("sess-B", "hello from B"), &mut app);

        assert!(affected, "chunk for active agent must request a redraw");
        let agent_b = app.agents.get(&AgentId(1)).unwrap();
        assert_eq!(agent_message_text(agent_b), "hello from B");
    }

    #[test]
    fn acp_chunk_for_subagent_routes_through_parent() {
        // Subagent (child) chunk must land in the parent's `subagent_views[child_sid]` even when a different agent is currently active
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        switch_active_to(&mut app, AgentId(1));

        let child_sid = "sess-A-child";
        {
            let parent = app.agents.get_mut(&AgentId(0)).unwrap();
            parent
                .subagent_sessions
                .insert(child_sid.into(), make_subagent_info(child_sid));
            parent
                .insert_test_child(child_sid.into(), Box::new(make_agent(Some(child_sid))));
        }

        let affected = handle(
            make_agent_chunk_message(child_sid, "hello from subagent"),
            &mut app,
        );

        let parent = app.agents.get(&AgentId(0)).unwrap();
        let child_view = parent
            .subagent_views
            .get(child_sid)
            .expect("child view must still exist");
        assert_eq!(
            agent_message_text(child_view),
            "hello from subagent",
            "subagent chunk must land in subagent_views[child_sid]"
        );
        assert!(
            !affected,
            "subagent chunk for non-active parent must not request a redraw"
        );
    }

    #[test]
    fn acp_chunk_with_unknown_session_id_is_dropped_and_no_redraw() {
        // No agent owns the session_id and the active agent already has a session_id assigned (so the race-window fallback does not fire)
        // The notification must be dropped silently.
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        // make_app_with_agent already activated AgentId(0); no switch needed.

        let affected = handle(
            make_agent_chunk_message("sess-unknown", "stray text"),
            &mut app,
        );

        assert!(!affected, "unknown session_id must not request a redraw");
        assert!(
            app.agents.get(&AgentId(0)).unwrap().scrollback.is_empty(),
            "agent A must not have absorbed a notification for sess-unknown"
        );
        assert!(
            app.agents.get(&AgentId(1)).unwrap().scrollback.is_empty(),
            "agent B must not have absorbed a notification for sess-unknown"
        );
    }

    #[test]
    fn session_id_none_race_window_routes_to_active_agent() {
        // Pin the existing race-window behavior: notifications that arrive before `TaskResult::SessionCreated` must still land on the active agent
        // In that window the active agent has no session_id yet

        // Case 1: active agent A has session_id == None; everyone else has a real id
        // A stray notification routes to A
        {
            let mut app = make_app_with_agent("sess-A");
            app.agents.get_mut(&AgentId(0)).unwrap().session.session_id = None;
            insert_agent(&mut app, AgentId(1), Some("sess-B"));
            // make_app_with_agent already activated AgentId(0); no switch needed.

            let _ = handle(
                make_agent_chunk_message("not-yet-assigned", "racing chunk"),
                &mut app,
            );

            assert_eq!(
                agent_message_text(app.agents.get(&AgentId(0)).unwrap()),
                "racing chunk",
                "race-window fallback should land on active agent A"
            );
            assert!(
                app.agents.get(&AgentId(1)).unwrap().scrollback.is_empty(),
                "non-active agent B must not absorb the race chunk"
            );
        }

        // Case 2: both A and B have session_id == None; the active one wins.
        {
            let mut app = make_app_with_agent("sess-A");
            app.agents.get_mut(&AgentId(0)).unwrap().session.session_id = None;
            insert_agent(&mut app, AgentId(1), None);
            switch_active_to(&mut app, AgentId(1));

            let _ = handle(
                make_agent_chunk_message("not-yet-assigned", "racing chunk"),
                &mut app,
            );

            assert!(
                app.agents.get(&AgentId(0)).unwrap().scrollback.is_empty(),
                "non-active agent A must not absorb the race chunk"
            );
            assert_eq!(
                agent_message_text(app.agents.get(&AgentId(1)).unwrap()),
                "racing chunk",
                "race-window fallback must prefer the active agent (B)"
            );
        }
    }

    #[test]
    fn plan_update_for_inactive_agent_lands_in_its_todo() {
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        switch_active_to(&mut app, AgentId(1));
        // Sanity: A's todo starts empty.
        assert_eq!(
            app.agents.get(&AgentId(0)).unwrap().todo.counts().total(),
            0,
        );

        let _ = handle(make_plan_message("sess-A", &["task1", "task2"]), &mut app);

        let agent_a = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent_a.todo.counts().total(),
            2,
            "Plan update must mutate A's todo even when B is active"
        );
        let agent_b = app.agents.get(&AgentId(1)).unwrap();
        assert_eq!(
            agent_b.todo.counts().total(),
            0,
            "active agent B's todo must not absorb A's plan"
        );
    }

    #[test]
    fn commands_update_for_inactive_agent_bumps_its_generation() {
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        switch_active_to(&mut app, AgentId(1));
        let initial_gen_a = app
            .agents
            .get(&AgentId(0))
            .unwrap()
            .session
            .available_commands_generation;

        let _ = handle(
            make_commands_update_message("sess-A", &["compact", "fork"]),
            &mut app,
        );

        let agent_a = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent_a.session.available_commands.len(),
            2,
            "AvailableCommandsUpdate must replace A's commands list"
        );
        assert_eq!(
            agent_a.session.available_commands_generation,
            initial_gen_a + 1,
            "AvailableCommandsUpdate must bump A's generation counter"
        );
    }

    #[test]
    fn bg_task_stdout_for_inactive_agent_lands_in_its_bg_tasks() {
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        switch_active_to(&mut app, AgentId(1));

        // Pre-register a bg task on A so route_bg_task_stdout has a target.
        let task_id = "task-A-1";
        let tool_call_id = "call-A-1";
        {
            let agent_a = app.agents.get_mut(&AgentId(0)).unwrap();
            agent_a.session.bg_tasks.insert(
                task_id.into(),
                BgTaskState {
                    task_id: task_id.into(),
                    tool_call_id: tool_call_id.into(),
                    command: "sleep 5".into(),
                    description: None,
                    cwd: "/tmp".into(),
                    output_file: "/tmp/out".into(),
                    status: BgTaskStatus::Running,
                    start_time: std::time::SystemTime::now(),
                    end_time: None,
                    exit_code: None,
                    signal: None,
                    stdout: String::new(),
                    stdout_line_count: 0,
                    truncated: false,
                    pending_kill: false,
                    kill_requested_at: None,
                    scrollback_entry_id: None,
                    is_monitor: false,
                    restored_from_replay: false,
                },
            );
            agent_a
                .session
                .bg_tool_call_to_task
                .insert(tool_call_id.into(), task_id.into());
        }

        let _ = handle(
            make_bash_stdout_message("sess-A", tool_call_id, "stdout-from-A"),
            &mut app,
        );

        let agent_a = app.agents.get(&AgentId(0)).unwrap();
        assert_eq!(
            agent_a.session.bg_tasks.get(task_id).unwrap().stdout,
            "stdout-from-A",
            "Bash stdout must land in A's bg_tasks even when B is active"
        );
    }

    #[test]
    fn acp_chunks_for_two_agents_dont_cross_contaminate() {
        // Send chunks to both A and B in sequence
        // Each landing in its own scrollback proves the demux works in both directions regardless of which agent is currently active
        let mut app = make_app_with_agent("sess-A");
        insert_agent(&mut app, AgentId(1), Some("sess-B"));
        switch_active_to(&mut app, AgentId(1));

        let _ = handle(make_agent_chunk_message("sess-A", "A only"), &mut app);
        let _ = handle(make_agent_chunk_message("sess-B", "B only"), &mut app);

        assert_eq!(
            agent_message_text(app.agents.get(&AgentId(0)).unwrap()),
            "A only",
        );
        assert_eq!(
            agent_message_text(app.agents.get(&AgentId(1)).unwrap()),
            "B only",
        );
    }

    fn session_notification(session_id: &str, update: acp::SessionUpdate) -> AcpClientMessage {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        AcpClientMessage::SessionNotification(a3s_acp_lib::AcpArgs {
            request: acp::SessionNotification::new(acp::SessionId::new(session_id), update),
            response_tx: tx,
        })
    }

    fn a3s_ext(session_id: &str, update: serde_json::Value) -> AcpClientMessage {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let payload = serde_json::json!({
            "sessionId": session_id,
            "update": update,
        });
        let raw = serde_json::value::to_raw_value(&payload).unwrap();
        AcpClientMessage::ExtNotification(a3s_acp_lib::AcpArgs {
            request: acp::ExtNotification::new("x.ai/session_notification", raw.into()),
            response_tx: tx,
        })
    }

    /// a3s `tool_raw_output` JSON, including null signal/description and byte `output`.
    fn a3s_bash_raw_output(command: &str, stdout: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "Bash",
            "output": stdout.as_bytes(),
            "output_for_prompt": stdout,
            "exit_code": 0,
            "command": command,
            "truncated": false,
            "signal": null,
            "timed_out": false,
            "description": null,
            "current_dir": "",
            "output_file": "",
            "total_bytes": stdout.len(),
        })
    }

    #[test]
    fn a3s_code_tool_wire_renders_execute_read_and_edit_cards() {
        use crate::scrollback::block::RenderBlock;
        use crate::scrollback::blocks::tool::ToolCallBlock;
        let mut app = make_app_with_agent("sess-tools");

        let _ = handle(
            session_notification(
                "sess-tools",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("bash-1"), "bash")
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::InProgress)
                        .raw_input(Some(serde_json::json!({ "command": "echo ok" }))),
                ),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-tools",
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    acp::ToolCallId::new("bash-1"),
                    acp::ToolCallUpdateFields::new()
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({ "command": "echo ok" })))
                        .raw_output(Some(a3s_bash_raw_output("echo ok", "ok\n")))
                        .content(Some(vec![acp::ToolCallContent::from(
                            acp::ContentBlock::Text(acp::TextContent::new("ok\n")),
                        )])),
                )),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-tools",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("read-1"), "read")
                        .kind(acp::ToolKind::Read)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({ "file_path": "src/main.rs" }))),
                ),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-tools",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("edit-1"), "edit")
                        .kind(acp::ToolKind::Edit)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({
                            "file_path": "src/main.rs",
                            "old_string": "a",
                            "new_string": "b",
                        })))
                        .content(vec![acp::ToolCallContent::Diff(
                            acp::Diff::new("src/main.rs", "b").old_text(Some("a".to_string())),
                        )]),
                ),
            ),
            &mut app,
        );

        let agent = test_agent(&app, AgentId(0));
        let mut saw_execute = false;
        let mut saw_read = false;
        let mut saw_edit = false;
        for index in 0..agent.scrollback.len() {
            let Some(entry) = agent.scrollback.get(index) else {
                continue;
            };
            match &entry.block {
                RenderBlock::ToolCall(ToolCallBlock::Execute(exec)) => {
                    assert_eq!(exec.command, "echo ok");
                    assert_eq!(exec.output.as_deref(), Some("ok\n"));
                    saw_execute = true;
                }
                RenderBlock::ToolCall(ToolCallBlock::Read(read)) => {
                    assert!(read.path.ends_with("src/main.rs"));
                    saw_read = true;
                }
                RenderBlock::ToolCall(ToolCallBlock::Edit(edit)) => {
                    assert!(edit.path.ends_with("src/main.rs"));
                    assert!(!edit.hunks.is_empty());
                    saw_edit = true;
                }
                _ => {}
            }
        }
        assert!(saw_execute, "bash wire JSON must render an Execute card with stdout");
        assert!(saw_read, "read file_path must render a Read card");
        assert!(saw_edit, "edit diff content must render an Edit card");
    }

    #[test]
    fn a3s_code_plan_statuses_render_in_the_todo_pane() {
        use a3s_code_tools::implementations::grok_build::todo::TodoStatus;
        let mut app = make_app_with_agent("sess-plan");
        let mut failed = acp::Meta::new();
        failed.insert("a3sStatus".into(), serde_json::json!("failed"));
        failed.insert("id".into(), serde_json::json!("t-fail"));
        let mut skipped = acp::Meta::new();
        skipped.insert("a3sStatus".into(), serde_json::json!("skipped"));
        let mut cancelled = acp::Meta::new();
        cancelled.insert("cancelled".into(), serde_json::json!(true));
        let entries = vec![
            acp::PlanEntry::new(
                "read the file",
                acp::PlanEntryPriority::Medium,
                acp::PlanEntryStatus::Pending,
            ),
            acp::PlanEntry::new(
                "edit the file",
                acp::PlanEntryPriority::High,
                acp::PlanEntryStatus::InProgress,
            ),
            acp::PlanEntry::new(
                "ship it",
                acp::PlanEntryPriority::Medium,
                acp::PlanEntryStatus::Completed,
            ),
            acp::PlanEntry::new(
                "run tests",
                acp::PlanEntryPriority::High,
                acp::PlanEntryStatus::Completed,
            )
            .meta(Some(failed)),
            acp::PlanEntry::new(
                "optional lint",
                acp::PlanEntryPriority::Low,
                acp::PlanEntryStatus::Completed,
            )
            .meta(Some(skipped)),
            acp::PlanEntry::new(
                "dropped",
                acp::PlanEntryPriority::Medium,
                acp::PlanEntryStatus::Completed,
            )
            .meta(Some(cancelled)),
        ];
        let _ = handle(
            session_notification(
                "sess-plan",
                acp::SessionUpdate::Plan(acp::Plan::new(entries)),
            ),
            &mut app,
        );
        let agent = test_agent(&app, AgentId(0));
        let todos = agent.todo.todos();
        assert_eq!(
            todos.iter().map(|item| item.status).collect::<Vec<_>>(),
            vec![
                TodoStatus::Pending,
                TodoStatus::InProgress,
                TodoStatus::Completed,
                TodoStatus::Cancelled,
                TodoStatus::Cancelled,
                TodoStatus::Cancelled,
            ]
        );
        assert_eq!(todos[3].content, "run tests");
        let counts = agent.todo.counts();
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.in_progress, 1);
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.cancelled, 3);
    }

    #[test]
    fn a3s_code_subagent_wire_registers_and_completes_the_parent_row() {
        use crate::scrollback::block::RenderBlock;
        use crate::scrollback::blocks::SubagentBlockKind;
        let mut app = make_app_with_agent("sess-parent");
        let _ = handle(
            a3s_ext(
                "sess-parent",
                serde_json::json!({
                    "sessionUpdate": "subagent_spawned",
                    "subagent_id": "task-1",
                    "parent_session_id": "sess-parent",
                    "child_session_id": "child-1",
                    "subagent_type": "explore",
                    "description": "scan src/",
                }),
            ),
            &mut app,
        );
        let agent = test_agent(&app, AgentId(0));
        let info = agent
            .subagent_sessions
            .get("child-1")
            .expect("a3s subagent_spawned must register the child session");
        assert_eq!(info.description.as_ref(), "scan src/");
        assert_eq!(info.subagent_type.as_ref(), "explore");
        let entry_id = info
            .attempt
            .scrollback_entry_id
            .expect("spawn must stash a scrollback entry");
        let entry = agent.scrollback.get_by_id(entry_id).unwrap();
        let RenderBlock::Subagent(block) = &entry.block else {
            panic!("spawn must push a Subagent block");
        };
        assert!(matches!(block.kind, SubagentBlockKind::Started));

        let _ = handle(
            a3s_ext(
                "sess-parent",
                serde_json::json!({
                    "sessionUpdate": "subagent_finished",
                    "subagent_id": "task-1",
                    "child_session_id": "child-1",
                    "status": "completed",
                    "tool_calls": 2,
                    "turns": 1,
                    "duration_ms": 500,
                    "tokens_used": 0,
                    "output": "done",
                    "will_wake": false,
                }),
            ),
            &mut app,
        );
        let agent = test_agent(&app, AgentId(0));
        let info = agent.subagent_sessions.get("child-1").unwrap();
        assert!(info.is_finished());
        assert_eq!(info.attempt.status.as_deref(), Some("completed"));
        let entry = agent.scrollback.get_by_id(entry_id).unwrap();
        let RenderBlock::Subagent(block) = &entry.block else {
            panic!("finish must keep the Subagent block");
        };
        assert!(matches!(
            block.kind,
            SubagentBlockKind::Completed { elapsed } if elapsed == std::time::Duration::from_millis(500)
        ));
    }

    #[test]
    fn a3s_code_search_and_fetch_cards_render_from_tool_kind() {
        use crate::scrollback::block::RenderBlock;
        use crate::scrollback::blocks::tool::ToolCallBlock;
        let mut app = make_app_with_agent("sess-search");
        let _ = handle(
            session_notification(
                "sess-search",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("grep-1"), "grep")
                        .kind(acp::ToolKind::Search)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({ "pattern": "fn main" }))),
                ),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-search",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("fetch-1"), "web_fetch")
                        .kind(acp::ToolKind::Fetch)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({ "url": "https://example.com/a" })))
                        .content(vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
                            acp::TextContent::new("page"),
                        ))]),
                ),
            ),
            &mut app,
        );
        let agent = test_agent(&app, AgentId(0));
        let mut saw_search = false;
        let mut saw_fetch = false;
        for index in 0..agent.scrollback.len() {
            let Some(entry) = agent.scrollback.get(index) else {
                continue;
            };
            match &entry.block {
                RenderBlock::ToolCall(ToolCallBlock::Search(search)) => {
                    assert_eq!(search.pattern, "fn main");
                    saw_search = true;
                }
                RenderBlock::ToolCall(ToolCallBlock::WebFetch(fetch)) => {
                    assert_eq!(fetch.url, "https://example.com/a");
                    assert_eq!(fetch.output.as_deref(), Some("page"));
                    saw_fetch = true;
                }
                _ => {}
            }
        }
        assert!(saw_search, "ToolKind::Search must render a Search card");
        assert!(saw_fetch, "ToolKind::Fetch must render a WebFetch card");
    }

    #[test]
    fn a3s_code_subagent_progress_updates_the_parent_attempt() {
        use crate::scrollback::block::RenderBlock;
        use crate::scrollback::blocks::SubagentBlockKind;
        let mut app = make_app_with_agent("sess-parent");
        let _ = handle(
            a3s_ext(
                "sess-parent",
                serde_json::json!({
                    "sessionUpdate": "subagent_spawned",
                    "subagent_id": "task-1",
                    "parent_session_id": "sess-parent",
                    "child_session_id": "child-1",
                    "subagent_type": "explore",
                    "description": "scan src/",
                }),
            ),
            &mut app,
        );
        let _ = handle(
            a3s_ext(
                "sess-parent",
                serde_json::json!({
                    "sessionUpdate": "subagent_progress",
                    "subagent_id": "task-1",
                    "parent_session_id": "sess-parent",
                    "child_session_id": "child-1",
                    "duration_ms": 1200,
                    "turn_count": 2,
                    "tool_call_count": 3,
                    "tokens_used": 400,
                    "context_window_tokens": 0,
                    "context_usage_pct": 0,
                    "tools_used": ["grep"],
                    "error_count": 0,
                }),
            ),
            &mut app,
        );
        let agent = test_agent(&app, AgentId(0));
        let info = agent
            .subagent_sessions
            .get("child-1")
            .expect("progress must land on the spawned child");
        assert!(!info.is_finished());
        assert_eq!(info.attempt.duration_ms, Some(1200));
        assert_eq!(info.attempt.turn_count, Some(2));
        assert_eq!(info.attempt.tool_call_count, Some(3));
        assert_eq!(info.attempt.tokens_used, Some(400));
        assert_eq!(info.attempt.tools_used.len(), 1);
        let entry_id = info.attempt.scrollback_entry_id.expect("spawn row");
        let entry = agent.scrollback.get_by_id(entry_id).unwrap();
        let RenderBlock::Subagent(block) = &entry.block else {
            panic!("progress must keep the parent Subagent row");
        };
        assert!(matches!(block.kind, SubagentBlockKind::Started));
    }

    /// Conversation stream, tool cards, reasoning, subagents, plans, and
    /// concurrent work. Each name is one user-visible effect.
    #[test]
    fn a3s_conversation_flow_function_coverage_is_at_least_95_percent() {
        use crate::scrollback::blocks::SubagentBlockKind;
        use crate::scrollback::blocks::tool::ToolCallBlock;
        use a3s_code_tools::implementations::grok_build::todo::TodoStatus;

        crate::appearance::cache::set_show_thinking_blocks(true);
        let mut covered = Vec::new();
        let mut missing = Vec::new();
        let mut record = |name: &str, ok: bool| {
            if ok {
                covered.push(name.to_string());
            } else {
                missing.push(name.to_string());
            }
        };

        let mut app = make_app_with_agent("sess-flow");
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new("fix the bug")),
                )),
            ),
            &mut app,
        );
        let _ = handle(make_agent_chunk_message("sess-flow", "Looking "), &mut app);
        let _ = handle(make_agent_chunk_message("sess-flow", "now."), &mut app);
        let agent = test_agent(&app, AgentId(0));
        let user = user_prompt_text(agent);
        let assistant = agent_message_text(agent);
        record("stream:user-prompt", user == "fix the bug");
        record("stream:assistant-deltas-merge", assistant == "Looking now.");
        record(
            "stream:user-separate-from-assistant",
            user == "fix the bug" && !assistant.contains("fix the bug"),
        );

        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new("check ")),
                )),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new("the call")),
                )),
            ),
            &mut app,
        );
        let thoughts = thinking_texts(test_agent(&app, AgentId(0)));
        record(
            "reasoning:chunks-merge",
            thoughts.len() == 1 && thoughts[0] == "check the call",
        );
        record(
            "reasoning:separate-from-answer",
            agent_message_text(test_agent(&app, AgentId(0))) == "Looking now.",
        );
        let before_empty = thinking_texts(test_agent(&app, AgentId(0))).len();
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new("")),
                )),
            ),
            &mut app,
        );
        record(
            "reasoning:empty-dropped",
            thinking_texts(test_agent(&app, AgentId(0))).len() == before_empty,
        );
        crate::appearance::cache::set_show_thinking_blocks(false);
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new("hidden")),
                )),
            ),
            &mut app,
        );
        record(
            "reasoning:hidden-when-disabled",
            !thinking_texts(test_agent(&app, AgentId(0)))
                .iter()
                .any(|text| text.contains("hidden")),
        );
        crate::appearance::cache::set_show_thinking_blocks(true);

        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("bash-a"), "bash")
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::InProgress)
                        .raw_input(Some(serde_json::json!({ "command": "echo alpha" }))),
                ),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("bash-b"), "bash")
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::InProgress)
                        .raw_input(Some(serde_json::json!({ "command": "echo beta" }))),
                ),
            ),
            &mut app,
        );
        let running = execute_cards(test_agent(&app, AgentId(0)));
        record(
            "tools:parallel-both-running",
            running.iter().any(|(cmd, out, err)| {
                cmd == "echo alpha" && out.is_none() && err.is_none()
            }) && running.iter().any(|(cmd, out, err)| {
                cmd == "echo beta" && out.is_none() && err.is_none()
            }),
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    acp::ToolCallId::new("bash-a"),
                    acp::ToolCallUpdateFields::new()
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::InProgress)
                        .raw_input(Some(serde_json::json!({ "command": "echo alpha" })))
                        .raw_output(Some(streaming_bash_delta("hel"))),
                )),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    acp::ToolCallId::new("bash-a"),
                    acp::ToolCallUpdateFields::new()
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::InProgress)
                        .raw_input(Some(serde_json::json!({ "command": "echo alpha" })))
                        .raw_output(Some(streaming_bash_delta("lo"))),
                )),
            ),
            &mut app,
        );
        let streamed = execute_cards(test_agent(&app, AgentId(0)));
        record(
            "tools:output-streams",
            streamed
                .iter()
                .any(|(cmd, out, _)| cmd == "echo alpha" && out.as_deref() == Some("hello")),
        );
        record(
            "parallel:one-finish-leaves-the-other",
            streamed
                .iter()
                .any(|(cmd, out, _)| cmd == "echo beta" && out.is_none()),
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    acp::ToolCallId::new("bash-b"),
                    acp::ToolCallUpdateFields::new()
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::Failed)
                        .raw_input(Some(serde_json::json!({ "command": "echo beta" })))
                        .content(Some(vec![acp::ToolCallContent::from(
                            acp::ContentBlock::Text(acp::TextContent::new("exit 1")),
                        )])),
                )),
            ),
            &mut app,
        );
        record(
            "tools:failed",
            execute_cards(test_agent(&app, AgentId(0))).iter().any(|(cmd, _, err)| {
                cmd == "echo beta" && err.as_deref() == Some("exit 1")
            }),
        );

        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("read-1"), "read")
                        .kind(acp::ToolKind::Read)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({ "file_path": "src/lib.rs" }))),
                ),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("edit-1"), "edit")
                        .kind(acp::ToolKind::Edit)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({
                            "file_path": "src/lib.rs",
                            "old_string": "a",
                            "new_string": "b",
                        })))
                        .content(vec![acp::ToolCallContent::Diff(
                            acp::Diff::new("src/lib.rs", "b").old_text(Some("a".into())),
                        )]),
                ),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("grep-1"), "grep")
                        .kind(acp::ToolKind::Search)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({ "pattern": "fn flow" }))),
                ),
            ),
            &mut app,
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(acp::ToolCallId::new("fetch-1"), "web_fetch")
                        .kind(acp::ToolKind::Fetch)
                        .status(acp::ToolCallStatus::Completed)
                        .raw_input(Some(serde_json::json!({ "url": "https://example.com/flow" })))
                        .content(vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
                            acp::TextContent::new("body"),
                        ))]),
                ),
            ),
            &mut app,
        );
        let agent = test_agent(&app, AgentId(0));
        record(
            "tools:read",
            tool_blocks(agent).iter().any(|block| {
                matches!(block, ToolCallBlock::Read(read) if read.path.ends_with("src/lib.rs"))
            }),
        );
        record(
            "tools:edit",
            tool_blocks(agent)
                .iter()
                .any(|block| matches!(block, ToolCallBlock::Edit(edit) if !edit.hunks.is_empty())),
        );
        record(
            "tools:search",
            tool_blocks(agent).iter().any(
                |block| matches!(block, ToolCallBlock::Search(search) if search.pattern == "fn flow"),
            ),
        );
        record(
            "tools:fetch",
            tool_blocks(agent).iter().any(|block| {
                matches!(
                    block,
                    ToolCallBlock::WebFetch(fetch)
                        if fetch.url == "https://example.com/flow"
                            && fetch.output.as_deref() == Some("body")
                )
            }),
        );
        record(
            "stream:answer-survives-tools",
            agent_message_text(agent) == "Looking now.",
        );
        let _ = handle(make_agent_chunk_message("sess-flow", "Done."), &mut app);
        record(
            "stream:reply-after-tools",
            agent_message_text(test_agent(&app, AgentId(0))).contains("Done."),
        );

        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::Plan(acp::Plan::new(vec![
                    acp::PlanEntry::new(
                        "read",
                        acp::PlanEntryPriority::Medium,
                        acp::PlanEntryStatus::Pending,
                    ),
                    acp::PlanEntry::new(
                        "edit",
                        acp::PlanEntryPriority::High,
                        acp::PlanEntryStatus::InProgress,
                    ),
                ])),
            ),
            &mut app,
        );
        let first = test_agent(&app, AgentId(0)).todo.todos();
        record(
            "plan:statuses",
            first.len() == 2
                && first[0].status == TodoStatus::Pending
                && first[1].status == TodoStatus::InProgress,
        );
        let _ = handle(
            session_notification(
                "sess-flow",
                acp::SessionUpdate::Plan(acp::Plan::new(vec![
                    acp::PlanEntry::new(
                        "left",
                        acp::PlanEntryPriority::High,
                        acp::PlanEntryStatus::InProgress,
                    ),
                    acp::PlanEntry::new(
                        "right",
                        acp::PlanEntryPriority::High,
                        acp::PlanEntryStatus::InProgress,
                    ),
                ])),
            ),
            &mut app,
        );
        let parallel_plan = test_agent(&app, AgentId(0)).todo.todos();
        record(
            "plan:replaces-previous",
            parallel_plan.len() == 2 && parallel_plan.iter().all(|item| item.content != "read"),
        );
        record(
            "parallel:plan-steps",
            parallel_plan
                .iter()
                .filter(|item| item.status == TodoStatus::InProgress)
                .count()
                == 2,
        );
        record(
            "plan:coexists-with-tools",
            !execute_cards(test_agent(&app, AgentId(0))).is_empty()
                && !test_agent(&app, AgentId(0)).todo.todos().is_empty(),
        );

        let _ = handle(
            a3s_ext(
                "sess-flow",
                serde_json::json!({
                    "sessionUpdate": "subagent_spawned",
                    "subagent_id": "task-a",
                    "parent_session_id": "sess-flow",
                    "child_session_id": "child-a",
                    "subagent_type": "explore",
                    "description": "scan a",
                }),
            ),
            &mut app,
        );
        let _ = handle(
            a3s_ext(
                "sess-flow",
                serde_json::json!({
                    "sessionUpdate": "subagent_spawned",
                    "subagent_id": "task-b",
                    "parent_session_id": "sess-flow",
                    "child_session_id": "child-b",
                    "subagent_type": "review",
                    "description": "scan b",
                }),
            ),
            &mut app,
        );
        let started = subagent_rows(test_agent(&app, AgentId(0)));
        record(
            "subagent:spawn",
            started.iter().any(|(desc, kind)| {
                desc == "scan a" && matches!(kind, SubagentBlockKind::Started)
            }),
        );
        record(
            "parallel:subagents",
            started.iter().any(|(desc, _)| desc == "scan a")
                && started.iter().any(|(desc, _)| desc == "scan b"),
        );
        let _ = handle(make_agent_chunk_message("child-a", "child note"), &mut app);
        let parent = test_agent(&app, AgentId(0));
        let child = parent.subagent_views.get("child-a");
        record(
            "subagent:child-stream",
            child.is_some_and(|view| agent_message_text(view) == "child note")
                && !agent_message_text(parent).contains("child note"),
        );
        let _ = handle(
            a3s_ext(
                "sess-flow",
                serde_json::json!({
                    "sessionUpdate": "subagent_progress",
                    "subagent_id": "task-a",
                    "parent_session_id": "sess-flow",
                    "child_session_id": "child-a",
                    "duration_ms": 40,
                    "turn_count": 1,
                    "tool_call_count": 2,
                    "tokens_used": 10,
                    "context_window_tokens": 0,
                    "context_usage_pct": 0,
                    "tools_used": ["grep"],
                    "error_count": 0,
                }),
            ),
            &mut app,
        );
        let progress = test_agent(&app, AgentId(0))
            .subagent_sessions
            .get("child-a");
        record(
            "subagent:progress",
            progress.is_some_and(|info| {
                !info.is_finished()
                    && info.attempt.tool_call_count == Some(2)
                    && info.attempt.tools_used.len() == 1
            }),
        );
        let _ = handle(
            a3s_ext(
                "sess-flow",
                serde_json::json!({
                    "sessionUpdate": "subagent_finished",
                    "subagent_id": "task-a",
                    "child_session_id": "child-a",
                    "status": "completed",
                    "tool_calls": 2,
                    "turns": 1,
                    "duration_ms": 80,
                    "tokens_used": 10,
                    "will_wake": false,
                }),
            ),
            &mut app,
        );
        let _ = handle(
            a3s_ext(
                "sess-flow",
                serde_json::json!({
                    "sessionUpdate": "subagent_finished",
                    "subagent_id": "task-b",
                    "child_session_id": "child-b",
                    "status": "failed",
                    "error": "review blocked",
                    "tool_calls": 0,
                    "turns": 1,
                    "duration_ms": 15,
                    "tokens_used": 0,
                    "will_wake": false,
                }),
            ),
            &mut app,
        );
        let rows = subagent_rows(test_agent(&app, AgentId(0)));
        record(
            "subagent:completed",
            rows.iter().any(|(desc, kind)| {
                desc == "scan a" && matches!(kind, SubagentBlockKind::Completed { .. })
            }),
        );
        record(
            "subagent:failed",
            rows.iter().any(|(desc, kind)| {
                desc == "scan b"
                    && matches!(
                        kind,
                        SubagentBlockKind::Failed { error, .. }
                            if error.as_deref() == Some("review blocked")
                    )
            }),
        );
        record(
            "parallel:sibling-failure-leaves-the-other",
            rows.iter().any(|(desc, kind)| {
                desc == "scan a" && matches!(kind, SubagentBlockKind::Completed { .. })
            }) && rows.iter().any(|(desc, kind)| {
                desc == "scan b" && matches!(kind, SubagentBlockKind::Failed { .. })
            }),
        );

        crate::appearance::cache::set_show_thinking_blocks(true);
        let total = covered.len() + missing.len();
        let percent = covered.len() * 100 / total.max(1);
        assert!(
            percent >= 95,
            "conversation flow coverage is {percent}% ({}/{}). Missing: {}",
            covered.len(),
            total,
            missing.join(", ")
        );
    }

    fn user_prompt_text(agent: &AgentView) -> String {
        let mut out = String::new();
        for index in 0..agent.scrollback.len() {
            if let Some(entry) = agent.scrollback.get(index)
                && let crate::scrollback::block::RenderBlock::UserPrompt(prompt) = &entry.block
            {
                out.push_str(&prompt.text);
            }
        }
        out
    }

    fn thinking_texts(agent: &AgentView) -> Vec<String> {
        let mut texts = Vec::new();
        for index in 0..agent.scrollback.len() {
            if let Some(entry) = agent.scrollback.get(index)
                && let crate::scrollback::block::RenderBlock::Thinking(block) = &entry.block
            {
                texts.push(block.text());
            }
        }
        texts
    }

    fn execute_cards(agent: &AgentView) -> Vec<(String, Option<String>, Option<String>)> {
        use crate::scrollback::blocks::tool::ToolCallBlock;
        tool_blocks(agent)
            .into_iter()
            .filter_map(|block| match block {
                ToolCallBlock::Execute(exec) => Some((exec.command, exec.output, exec.error)),
                _ => None,
            })
            .collect()
    }

    fn tool_blocks(agent: &AgentView) -> Vec<crate::scrollback::blocks::tool::ToolCallBlock> {
        let mut blocks = Vec::new();
        for index in 0..agent.scrollback.len() {
            if let Some(entry) = agent.scrollback.get(index)
                && let crate::scrollback::block::RenderBlock::ToolCall(block) = &entry.block
            {
                blocks.push(block.clone());
            }
        }
        blocks
    }

    fn subagent_rows(
        agent: &AgentView,
    ) -> Vec<(String, crate::scrollback::blocks::SubagentBlockKind)> {
        let mut rows = Vec::new();
        for index in 0..agent.scrollback.len() {
            if let Some(entry) = agent.scrollback.get(index)
                && let crate::scrollback::block::RenderBlock::Subagent(block) = &entry.block
            {
                rows.push((block.description.clone(), block.kind.clone()));
            }
        }
        rows
    }

    fn streaming_bash_delta(delta: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "Bash",
            "output": [],
            "output_for_prompt": "",
            "exit_code": 0,
            "command": "",
            "truncated": false,
            "signal": null,
            "timed_out": false,
            "description": null,
            "current_dir": "",
            "output_file": "",
            "total_bytes": delta.len(),
            "output_delta": delta.as_bytes(),
        })
    }

