#![cfg(test)]
use super::*;

#[test]
fn refuses_non_object_answers_and_preserves_typed_values() {
    assert!(submitted_values(r#"{"values":[]}"#).is_none());
    assert!(submitted_values(r#"{"values":null}"#).is_none());
    assert_eq!(
        submitted_values(r#"{"values":{"count":3,"ready":true,"topic":"work"}}"#),
        Some(json!({"count":3,"ready":true,"topic":"work"}))
    );
}

#[tokio::test]
async fn openui_request_round_trips_without_a_core_session() {
    use std::rc::Rc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    let workspace = tempfile::tempdir().expect("empty workspace");
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // The protocol handler has no providers. Neither session/new nor
            // session/load is invoked: this proof cannot construct a runtime.
            let handler = crate::agent::A3sCodeAgent::new(crate::config::LaunchConfig {
                workspace: workspace.path().to_owned(),
                model_id: String::new(),
                config: a3s_code_core::CodeConfig::default(),
            });
            let (raw_client, agent_end) = tokio::io::duplex(4096);
            let (agent_read, agent_write) = tokio::io::split(agent_end);
            let (connection, io) = agent_client_protocol::AgentSideConnection::new(
                handler,
                agent_write.compat_write(),
                agent_read.compat(),
                |future| {
                    tokio::task::spawn_local(future);
                },
            );
            let io_task = tokio::task::spawn_local(io);
            let connection = Rc::new(connection);
            let spec = "form = Stack([Input(name=reason), Button(action=submit)])";
            let exchange = async {
                let raw_reply = async {
                    let (read, mut write) = tokio::io::split(raw_client);
                    let mut reader = BufReader::new(read);
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).await.expect("request") > 0);
                    let frame: Value = serde_json::from_str(&line).expect("JSON-RPC frame");
                    // ACP prefixes extension methods with an underscore on
                    // the wire; Studio matches exactly `_a3s/form/request`.
                    assert_eq!(frame["method"], "_a3s/form/request");
                    assert_eq!(
                        frame["params"],
                        json!({"formId":"form-1","prompt":"请填写原因","spec":spec,"format":"openui"})
                    );
                    let reply = json!({
                        "jsonrpc":"2.0",
                        "id":frame["id"],
                        "result":{"values":{"reason":"需要休息","count":3,"ready":true}}
                    });
                    write
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .expect("reply");
                    (reader, write)
                };
                let (values, _raw_client) = tokio::join!(
                    request_form(&connection, "form-1", "请填写原因", spec),
                    raw_reply
                );
                assert_eq!(
                    values,
                    Some(json!({"reason":"需要休息","count":3,"ready":true}))
                );
            };
            let result = tokio::time::timeout(std::time::Duration::from_secs(2), exchange).await;
            io_task.abort();
            let _ = io_task.await;
            result.expect("bounded protocol exchange");
        })
        .await;
    assert_eq!(
        std::fs::read_dir(workspace.path())
            .expect("workspace")
            .count(),
        0
    );
}
