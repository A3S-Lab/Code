#![cfg(test)]

use super::*;

#[tokio::test]
async fn opted_in_agent_registers_reader_on_open_and_replacement_only() {
    let root = tempfile::tempdir().unwrap();
    let workspace = std::fs::canonicalize(root.path()).unwrap();
    for enabled in [false, true] {
        let mut launch = test_launch(Some(workspace.join(if enabled {
            "enabled"
        } else {
            "disabled"
        })));
        launch.workspace = workspace.clone();
        let agent = A3sCodeAgent::new(launch);
        let mut request = acp::NewSessionRequest::new(workspace.clone());
        if enabled {
            let mut meta = acp::Meta::new();
            meta.insert(
                "a3sStudioDocuments".to_string(),
                serde_json::Value::Bool(true),
            );
            request = request.meta(Some(meta));
        }
        let opened = acp::Agent::new_session(&agent, request).await.unwrap();
        let (session, model, host, components) = {
            let sessions = agent.sessions.lock().await;
            let live = sessions.get(opened.session_id.0.as_ref()).unwrap();
            (
                live.session.clone(),
                live.model_id.clone(),
                live.host.clone(),
                live.components.clone(),
            )
        };
        assert_eq!(
            session
                .tool_names()
                .iter()
                .any(|name| name == "studio_document_read"),
            enabled
        );
        let replaced = agent
            .replace_core_session(
                &session,
                SessionSetup {
                    workspace: workspace.clone(),
                    model_id: &model,
                    effort: None,
                    host,
                    components: &components,
                    mcp_servers: Vec::new(),
                },
            )
            .await
            .unwrap();
        let replaced = agent.finish_plugin_tools(replaced, vec![]).await.unwrap();
        assert_eq!(
            replaced
                .session
                .tool_names()
                .iter()
                .filter(|name| *name == "studio_document_read")
                .count(),
            usize::from(enabled)
        );
        replaced.session.close().await;
        agent.stop_document_bridge();
    }
}
