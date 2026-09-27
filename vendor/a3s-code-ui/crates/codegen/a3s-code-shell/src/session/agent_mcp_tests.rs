use a3s_code_agent::config::{AgentDefinition, AgentScope, McpServerRef};
use a3s_code_tools::types::compat::CompatConfig;
use agent_client_protocol as acp;

use super::{
    RematerializeParams, apply_agent_mcp_overlay, materialize_agent_mcp_servers,
    overlay_agent_mcp_servers, rematerialize_live_for_agent_seat, rematerialize_with_agent_overlay,
};

fn http(name: &str, url: &str, header: &str, value: &str) -> acp::McpServer {
    acp::McpServer::Http(
        acp::McpServerHttp::new(name, url).headers(vec![acp::HttpHeader::new(header, value)]),
    )
}

fn header_value(server: &acp::McpServer, header: &str) -> Option<String> {
    match server {
        acp::McpServer::Http(s) => s
            .headers
            .iter()
            .find(|h| h.name == header)
            .map(|h| h.value.clone()),
        acp::McpServer::Sse(s) => s
            .headers
            .iter()
            .find(|h| h.name == header)
            .map(|h| h.value.clone()),
        _ => None,
    }
}

fn named_server<'a>(servers: &'a [acp::McpServer], name: &str) -> &'a acp::McpServer {
    servers
        .iter()
        .find(|s| crate::session::mcp_servers::mcp_server_name(s) == name)
        .unwrap_or_else(|| panic!("missing server {name}"))
}

fn definition_with(mcp_servers: Vec<McpServerRef>) -> AgentDefinition {
    let mut definition = AgentDefinition::default_grok_build();
    definition.name = "seat".to_owned();
    definition.scope = AgentScope::User;
    definition.mcp_servers = mcp_servers;
    definition
}

fn inline_http(name: &str, url: &str, header: &str, value: &str) -> McpServerRef {
    McpServerRef::Inline {
        name: name.to_owned(),
        config: serde_json::json!({
            "type": "http",
            "url": url,
            "headers": { header: value },
        }),
    }
}

fn parse_inline_agent(name: &str, header: &str, value: &str) -> AgentDefinition {
    let mut definition = definition_with(vec![inline_http(
        "adder",
        "https://agent.example/mcp",
        header,
        value,
    )]);
    definition.name = name.to_owned();
    definition
}

fn has_server(servers: &[acp::McpServer], name: &str) -> bool {
    servers
        .iter()
        .any(|s| crate::session::mcp_servers::mcp_server_name(s) == name)
}

/// Isolate `find_project_configs` to this tempdir so `enabled = false` is visible to
/// [`crate::util::config::disabled_mcp_server_names`].
fn diskless_compat() -> CompatConfig {
    let mut compat = CompatConfig::default();
    compat.claude.mcps = false;
    compat.cursor.mcps = false;
    compat
}

fn rematerialize(
    definition: &AgentDefinition,
    cwd: &std::path::Path,
    parent_cwd: Option<&std::path::Path>,
) -> Vec<acp::McpServer> {
    rematerialize_with_agent_overlay(RematerializeParams {
        initial_client_mcp_servers: vec![],
        cwd,
        parent_cwd,
        plugin_registry: None,
        compat: &diskless_compat(),
        definition,
    })
}

fn rematerialize_live(
    live: &[acp::McpServer],
    previous: &AgentDefinition,
    new: &AgentDefinition,
    cwd: &std::path::Path,
    parent_cwd: Option<&std::path::Path>,
    client: Vec<acp::McpServer>,
) -> Vec<acp::McpServer> {
    rematerialize_live_for_agent_seat(
        live,
        &previous.mcp_servers,
        RematerializeParams {
            initial_client_mcp_servers: client,
            cwd,
            parent_cwd,
            plugin_registry: None,
            compat: &diskless_compat(),
            definition: new,
        },
    )
}

fn cwd_with_project_mcp(name: &str, enabled: bool) -> tempfile::TempDir {
    let cwd = tempfile::tempdir().unwrap();
    git2::Repository::init(cwd.path()).unwrap();
    std::fs::create_dir_all(cwd.path().join(".grok")).unwrap();
    let enabled_line = if enabled {
        String::new()
    } else {
        "enabled = false\n".to_owned()
    };
    std::fs::write(
        cwd.path().join(".grok").join("config.toml"),
        format!("[mcp_servers.{name}]\nurl = \"https://toml.example/mcp\"\n{enabled_line}"),
    )
    .unwrap();
    cwd
}

fn cwd_with_enabled_false(name: &str) -> tempfile::TempDir {
    cwd_with_project_mcp(name, false)
}

#[test]
fn overlay_insert_wins_on_name() {
    let mut merged = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    overlay_agent_mcp_servers(
        &mut merged,
        vec![http(
            "adder",
            "https://agent.example/mcp",
            "X-Token",
            "from-agent",
        )],
    );
    assert_eq!(1, merged.len());
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&merged, "adder"), "X-Token")
    );
    assert_eq!(
        Some("https://agent.example/mcp"),
        match named_server(&merged, "adder") {
            acp::McpServer::Http(s) => Some(s.url.as_str()),
            _ => None,
        }
    );
}

#[test]
fn overlay_appends_unknown_name_and_empty_is_noop() {
    let toml = http("toml-only", "https://toml.example/mcp", "X-Token", "toml");
    let mut merged = vec![toml.clone()];
    overlay_agent_mcp_servers(&mut merged, vec![]);
    assert_eq!(vec![toml.clone()], merged);

    overlay_agent_mcp_servers(
        &mut merged,
        vec![http(
            "agent-only",
            "https://agent.example/mcp",
            "X-Token",
            "agent",
        )],
    );
    assert_eq!(2, merged.len());
    assert_eq!(
        "toml-only",
        crate::session::mcp_servers::mcp_server_name(
            merged.first().expect("toml-only stays first after append")
        )
    );
    assert_eq!(
        "agent-only",
        crate::session::mcp_servers::mcp_server_name(
            merged.get(1).expect("agent-only is appended last")
        )
    );
}

#[test]
fn overlay_reapply_is_idempotent() {
    let overlay = vec![http(
        "adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-agent",
    )];
    let mut merged = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    overlay_agent_mcp_servers(&mut merged, overlay.clone());
    overlay_agent_mcp_servers(&mut merged, overlay);
    assert_eq!(1, merged.len());
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&merged, "adder"), "X-Token")
    );
}

#[test]
fn materialize_named_ref_resolves_against_pre_overlay_merge() {
    let cwd = tempfile::tempdir().unwrap();
    let lookup = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    let definition = definition_with(vec![McpServerRef::Named("adder".to_owned())]);
    let got = materialize_agent_mcp_servers(&definition, &lookup, cwd.path());
    assert_eq!(lookup, got);
}

#[test]
fn materialize_named_ref_missing_is_dropped() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = definition_with(vec![McpServerRef::Named("missing".to_owned())]);
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert!(got.is_empty());
}

#[test]
fn materialize_inline_http_headers() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = definition_with(vec![inline_http(
        "adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-agent",
    )]);
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert_eq!(1, got.len());
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&got, "adder"), "X-Token")
    );
}

#[test]
fn materialize_ignores_plugin_agent() {
    let cwd = tempfile::tempdir().unwrap();
    let mut definition = definition_with(vec![inline_http(
        "adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-agent",
    )]);
    definition.plugin_name = Some("evil".to_owned());
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert!(got.is_empty());
}

#[test]
fn materialize_ignores_untrusted_project_agent() {
    let cwd = tempfile::tempdir().unwrap();
    crate::agent::folder_trust::record_for_test(cwd.path(), false);
    let mut definition = definition_with(vec![inline_http(
        "adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-agent",
    )]);
    definition.scope = AgentScope::Project;
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert!(got.is_empty());
}

#[test]
fn parent_overlay_prefers_agent_md_headers_over_toml() {
    let cwd = tempfile::tempdir().unwrap();
    let mut merged = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    let definition = parse_inline_agent("seat-a", "X-Token", "from-agent");
    apply_agent_mcp_overlay(&mut merged, &definition, cwd.path());
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&merged, "adder"), "X-Token")
    );
}

#[test]
fn named_ref_on_parent_does_not_replace_toml_headers() {
    let cwd = tempfile::tempdir().unwrap();
    let mut merged = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    let definition = definition_with(vec![McpServerRef::Named("adder".to_owned())]);
    apply_agent_mcp_overlay(&mut merged, &definition, cwd.path());
    assert_eq!(
        Some("from-toml".to_owned()),
        header_value(named_server(&merged, "adder"), "X-Token")
    );
}

#[test]
fn reload_rematerialize_still_prefers_agent_md_headers() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = parse_inline_agent("seat-a", "X-Token", "from-agent");
    let mut merged = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    apply_agent_mcp_overlay(&mut merged, &definition, cwd.path());

    let mut rematerialized = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml-reloaded",
    )];
    apply_agent_mcp_overlay(&mut rematerialized, &definition, cwd.path());
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&rematerialized, "adder"), "X-Token")
    );
}

#[test]
fn rebuild_rematerializes_to_new_seat_agent_md_header() {
    let cwd = tempfile::tempdir().unwrap();
    let mut merged = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    apply_agent_mcp_overlay(
        &mut merged,
        &parse_inline_agent("seat-a", "X-Token", "from-seat-a"),
        cwd.path(),
    );
    assert_eq!(
        Some("from-seat-a".to_owned()),
        header_value(named_server(&merged, "adder"), "X-Token")
    );

    let mut rematerialized = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    apply_agent_mcp_overlay(
        &mut rematerialized,
        &parse_inline_agent("seat-b", "X-Token", "from-seat-b"),
        cwd.path(),
    );
    assert_eq!(
        Some("from-seat-b".to_owned()),
        header_value(named_server(&rematerialized, "adder"), "X-Token")
    );
}

#[test]
fn rematerialize_with_agent_overlay_inserts_over_empty_disk_merge() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = parse_inline_agent("seat-a", "X-Token", "from-agent");
    let merged = rematerialize(&definition, cwd.path(), None);
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&merged, "adder"), "X-Token")
    );
}

#[test]
fn child_materialize_only_does_not_mutate_parent_lookup() {
    let cwd = tempfile::tempdir().unwrap();
    let parent = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    let definition = parse_inline_agent("child", "X-Token", "from-child");
    let owned = materialize_agent_mcp_servers(&definition, &parent, cwd.path());
    assert_eq!(
        Some("from-child".to_owned()),
        header_value(named_server(&owned, "adder"), "X-Token")
    );
    assert_eq!(
        Some("from-toml".to_owned()),
        header_value(named_server(&parent, "adder"), "X-Token")
    );
}

#[test]
fn agent_md_yaml_map_style_headers_materialize() {
    let cwd = tempfile::tempdir().unwrap();
    let mut definition = AgentDefinition::parse(
        r#"---
name: seat-a
description: seat overlay
mcpServers:
  - adder: {type: http, url: "https://agent.example/mcp", headers: {X-Token: from-agent}}
---
body
"#,
    )
    .expect("flow-style agent.md mcpServers parses");
    definition.scope = AgentScope::User;
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&got, "adder"), "X-Token")
    );
}

#[test]
fn empty_definition_overlay_is_noop() {
    let cwd = tempfile::tempdir().unwrap();
    let mut merged = vec![http(
        "adder",
        "https://toml.example/mcp",
        "X-Token",
        "from-toml",
    )];
    apply_agent_mcp_overlay(
        &mut merged,
        &AgentDefinition::default_grok_build(),
        cwd.path(),
    );
    assert_eq!(
        Some("from-toml".to_owned()),
        header_value(named_server(&merged, "adder"), "X-Token")
    );
}

#[test]
fn materialize_drops_enabled_false_name() {
    let cwd = cwd_with_enabled_false("agent_md_ks_adder");
    let definition = definition_with(vec![inline_http(
        "agent_md_ks_adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-agent",
    )]);
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert!(got.is_empty(), "disabled name must not leave materialize");
}

#[test]
fn overlay_does_not_reappend_enabled_false_name() {
    let cwd = cwd_with_enabled_false("agent_md_ks_adder");
    let keep = http(
        "agent_md_ks_keep",
        "https://keep.example/mcp",
        "X-Token",
        "keep",
    );
    let mut merged = vec![keep.clone()];
    apply_agent_mcp_overlay(
        &mut merged,
        &definition_with(vec![inline_http(
            "agent_md_ks_adder",
            "https://agent.example/mcp",
            "X-Token",
            "from-agent",
        )]),
        cwd.path(),
    );
    assert_eq!(vec![keep], merged);
    assert!(!has_server(&merged, "agent_md_ks_adder"));
}

#[test]
fn overlay_still_appends_non_disabled_sibling() {
    let cwd = cwd_with_enabled_false("agent_md_ks_adder");
    let mut merged = vec![];
    apply_agent_mcp_overlay(
        &mut merged,
        &definition_with(vec![
            inline_http(
                "agent_md_ks_adder",
                "https://agent.example/mcp",
                "X-Token",
                "dead",
            ),
            inline_http(
                "agent_md_ks_keep",
                "https://agent.example/keep",
                "X-Token",
                "live",
            ),
        ]),
        cwd.path(),
    );
    assert!(!has_server(&merged, "agent_md_ks_adder"));
    assert_eq!(
        Some("live".to_owned()),
        header_value(named_server(&merged, "agent_md_ks_keep"), "X-Token")
    );
}

/// Hot-reload, agent-switch rebuild, and plugin rematerialize share this overlay.
#[test]
fn reload_rebuild_and_plugin_reload_do_not_reappend_disabled_name() {
    let cwd = cwd_with_enabled_false("agent_md_ks_adder");
    let seat_a = definition_with(vec![inline_http(
        "agent_md_ks_adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-a",
    )]);
    let seat_b = definition_with(vec![inline_http(
        "agent_md_ks_adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-b",
    )]);

    let mut merged = vec![];
    apply_agent_mcp_overlay(&mut merged, &seat_a, cwd.path());
    assert!(
        !has_server(&merged, "agent_md_ks_adder"),
        "spawn/overlay must not append a disabled name"
    );

    let mut rematerialized = vec![];
    apply_agent_mcp_overlay(&mut rematerialized, &seat_a, cwd.path());
    assert!(
        !has_server(&rematerialized, "agent_md_ks_adder"),
        "hot-reload rematerialize must not re-append a disabled name"
    );

    apply_agent_mcp_overlay(&mut rematerialized, &seat_b, cwd.path());
    assert!(
        !has_server(&rematerialized, "agent_md_ks_adder"),
        "agent-switch rebuild must not re-append a disabled name"
    );

    let plugin = rematerialize(&seat_a, cwd.path(), None);
    assert!(
        !has_server(&plugin, "agent_md_ks_adder"),
        "plugin reload rematerialize must not insert a disabled name"
    );
}

#[test]
fn live_rebuild_drops_omitted_seat_and_keeps_in_flight() {
    let cwd = tempfile::tempdir().unwrap();
    let in_flight = http(
        "in-flight",
        "https://inflight.example/mcp",
        "X-Token",
        "session",
    );
    let seat_a = definition_with(vec![
        inline_http("seat-a-only", "https://a.example/mcp", "X-Token", "from-a"),
        inline_http("shared", "https://a.example/shared", "X-Token", "from-a"),
    ]);
    let seat_b = definition_with(vec![
        inline_http("seat-b-only", "https://b.example/mcp", "X-Token", "from-b"),
        inline_http("shared", "https://b.example/shared", "X-Token", "from-b"),
    ]);
    let mut live = vec![in_flight.clone()];
    apply_agent_mcp_overlay(&mut live, &seat_a, cwd.path());

    let desired = rematerialize_live(&live, &seat_a, &seat_b, cwd.path(), None, vec![]);
    assert!(
        !has_server(&desired, "seat-a-only"),
        "rebuild must drop A's unique servers"
    );
    assert!(has_server(&desired, "seat-b-only"));
    assert_eq!(
        Some("from-b".to_owned()),
        header_value(named_server(&desired, "shared"), "X-Token")
    );
    assert_eq!(
        Some("session".to_owned()),
        header_value(named_server(&desired, "in-flight"), "X-Token"),
        "in-flight / session-injected names must survive rebuild"
    );
}

#[test]
fn seat_switch_handle_snapshot_drops_previous_seat_headers() {
    use crate::session::mcp_servers::McpState;

    let cwd = tempfile::tempdir().unwrap();
    let seat_a = definition_with(vec![
        inline_http("seat-a-only", "https://a.example/mcp", "X-Token", "from-a"),
        inline_http("shared", "https://a.example/shared", "X-Token", "from-a"),
    ]);
    let seat_b = definition_with(vec![
        inline_http("seat-b-only", "https://b.example/mcp", "X-Token", "from-b"),
        inline_http("shared", "https://b.example/shared", "X-Token", "from-b"),
    ]);
    let mut spawn = vec![];
    apply_agent_mcp_overlay(&mut spawn, &seat_a, cwd.path());
    let mut state = McpState::new(spawn);
    let handle_mcp = state.admitted_servers();
    let inherited_at_spawn = super::mcp_servers_for_fork(&handle_mcp);
    assert_eq!(
        Some("from-a".to_owned()),
        header_value(named_server(&inherited_at_spawn, "seat-a-only"), "X-Token")
    );

    let desired = rematerialize_live(&state.configs, &seat_a, &seat_b, cwd.path(), None, vec![]);
    assert!(
        state.update_configs_diff(desired).is_some(),
        "primary seat switch commits the new overlay through update_configs_diff"
    );

    let inherited = super::mcp_servers_for_fork(&handle_mcp);
    assert_eq!(
        inherited, state.configs,
        "handle.mcp_servers must match the actor's admitted list after A→B"
    );
    assert!(
        !has_server(&inherited, "seat-a-only"),
        "fork after A→B must not keep seat A's server"
    );
    assert!(has_server(&inherited, "seat-b-only"));
    assert_eq!(
        Some("from-b".to_owned()),
        header_value(named_server(&inherited, "shared"), "X-Token")
    );
    assert_eq!(
        Some("from-b".to_owned()),
        header_value(named_server(&inherited, "seat-b-only"), "X-Token")
    );
    assert!(
        inherited
            .iter()
            .all(|server| header_value(server, "X-Token") != Some("from-a".to_owned())),
        "fork snapshot must not retain seat A's headers"
    );
    assert_eq!(
        Some("from-a".to_owned()),
        header_value(named_server(&inherited_at_spawn, "shared"), "X-Token"),
        "a fork that already snapshotted seat A keeps that copy"
    );
}

#[test]
fn live_rebuild_empty_seat_clears_prior_overlay() {
    let cwd = tempfile::tempdir().unwrap();
    let toml_client = http("adder", "https://toml.example/mcp", "X-Token", "from-toml");
    let seat_a = parse_inline_agent("seat-a", "X-Token", "from-a");
    let mut live = vec![toml_client.clone()];
    apply_agent_mcp_overlay(&mut live, &seat_a, cwd.path());
    assert_eq!(
        Some("from-a".to_owned()),
        header_value(named_server(&live, "adder"), "X-Token")
    );

    let desired = rematerialize_live(
        &live,
        &seat_a,
        &AgentDefinition::default_grok_build(),
        cwd.path(),
        None,
        vec![toml_client.clone()],
    );
    assert_eq!(
        Some("from-toml".to_owned()),
        header_value(named_server(&desired, "adder"), "X-Token"),
        "empty seat must restore the disk/client layer"
    );
}

#[test]
fn live_rebuild_and_parallel_paths_still_hold_kill_switch() {
    let cwd = cwd_with_enabled_false("agent_md_ks_adder");
    let seat_a = definition_with(vec![inline_http(
        "agent_md_ks_adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-a",
    )]);
    let seat_b = definition_with(vec![inline_http(
        "agent_md_ks_adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-b",
    )]);
    let keep = inline_http(
        "agent_md_ks_keep",
        "https://agent.example/keep",
        "X-Token",
        "live",
    );
    let seat_b_with_keep = definition_with(vec![
        inline_http(
            "agent_md_ks_adder",
            "https://agent.example/mcp",
            "X-Token",
            "from-b",
        ),
        keep,
    ]);

    let mut spawn = vec![];
    apply_agent_mcp_overlay(&mut spawn, &seat_a, cwd.path());
    assert!(!has_server(&spawn, "agent_md_ks_adder"));

    let hot_reload = rematerialize(&seat_a, cwd.path(), None);
    assert!(!has_server(&hot_reload, "agent_md_ks_adder"));

    let plugin = rematerialize(&seat_b, cwd.path(), None);
    assert!(!has_server(&plugin, "agent_md_ks_adder"));

    let rebuild = rematerialize_live(&spawn, &seat_a, &seat_b_with_keep, cwd.path(), None, vec![]);
    assert!(!has_server(&rebuild, "agent_md_ks_adder"));
    assert_eq!(
        Some("live".to_owned()),
        header_value(named_server(&rebuild, "agent_md_ks_keep"), "X-Token")
    );

    let child = materialize_agent_mcp_servers(&seat_b, &[], cwd.path());
    assert!(
        child.is_empty(),
        "child materialize must still drop a disabled name"
    );
}

#[test]
fn materialize_inline_acp_stdio() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = definition_with(vec![McpServerRef::Inline {
        name: "stdio-adder".to_owned(),
        config: serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "adder"],
        }),
    }]);
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert_eq!(1, got.len());
    match named_server(&got, "stdio-adder") {
        acp::McpServer::Stdio(s) => {
            assert_eq!(std::path::Path::new("npx"), s.command.as_path());
            assert_eq!(&["-y", "adder"], s.args.as_slice());
        }
        other => panic!("expected stdio, got {other:?}"),
    }
}

#[test]
fn materialize_drops_garbage_inline_and_keeps_sibling() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = definition_with(vec![
        McpServerRef::Inline {
            name: "garbage".to_owned(),
            config: serde_json::json!({ "not": "a-server" }),
        },
        inline_http(
            "keeper",
            "https://agent.example/mcp",
            "X-Token",
            "from-agent",
        ),
    ]);
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert!(!has_server(&got, "garbage"));
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&got, "keeper"), "X-Token")
    );
}

#[test]
fn materialize_sorts_map_style_headers() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = definition_with(vec![McpServerRef::Inline {
        name: "adder".to_owned(),
        config: serde_json::json!({
            "type": "http",
            "url": "https://agent.example/mcp",
            "headers": { "Z-Last": "z", "A-First": "a" },
        }),
    }]);
    let first = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    let second = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert_eq!(
        serde_json::to_string(named_server(&first, "adder")).unwrap(),
        serde_json::to_string(named_server(&second, "adder")).unwrap()
    );
    match named_server(&first, "adder") {
        acp::McpServer::Http(s) => {
            let names: Vec<&str> = s.headers.iter().map(|h| h.name.as_str()).collect();
            assert_eq!(vec!["A-First", "Z-Last"], names);
        }
        other => panic!("expected http, got {other:?}"),
    }
}

#[test]
fn live_rebuild_uses_admitted_client_seed_not_spawn_snapshot() {
    let cwd = tempfile::tempdir().unwrap();
    let spawn_client = http("stale", "https://stale.example/mcp", "X-Token", "spawn");
    let live_client = http("added", "https://added.example/mcp", "X-Token", "live");
    let previous = definition_with(vec![inline_http(
        "added",
        "https://a.example/mcp",
        "X-Token",
        "from-a",
    )]);
    let stale_desired = rematerialize_live(
        std::slice::from_ref(&live_client),
        &previous,
        &AgentDefinition::default_grok_build(),
        cwd.path(),
        None,
        vec![spawn_client.clone()],
    );
    assert!(
        has_server(&stale_desired, "stale"),
        "spawn-time seed would restore a removed client"
    );
    assert!(
        !has_server(&stale_desired, "added"),
        "spawn-time seed drops an added client when the previous seat used the name"
    );

    let desired = rematerialize_live(
        std::slice::from_ref(&live_client),
        &previous,
        &AgentDefinition::default_grok_build(),
        cwd.path(),
        None,
        vec![live_client.clone()],
    );
    assert!(
        !has_server(&desired, "stale"),
        "removed client must not return on rebuild"
    );
    assert_eq!(
        Some("live".to_owned()),
        header_value(named_server(&desired, "added"), "X-Token"),
        "added client must survive even if the previous seat used the name"
    );
}

#[test]
fn materialize_drops_disabled_setup_and_empty_url_keeps_sibling() {
    let cwd = tempfile::tempdir().unwrap();
    let definition = definition_with(vec![
        McpServerRef::Inline {
            name: "disabled".to_owned(),
            config: serde_json::json!({
                "type": "http",
                "url": "https://disabled.example/mcp",
                "enabled": false,
            }),
        },
        McpServerRef::Inline {
            name: "needs-setup".to_owned(),
            config: serde_json::json!({
                "type": "http",
                "url": "https://setup.example/mcp",
                "setup": {
                    "fields": [{ "id": "team", "label": "Team", "type": "select" }],
                },
            }),
        },
        McpServerRef::Inline {
            name: "empty-url".to_owned(),
            config: serde_json::json!({
                "type": "http",
                "url": "",
            }),
        },
        inline_http(
            "keeper",
            "https://agent.example/mcp",
            "X-Token",
            "from-agent",
        ),
    ]);
    let got = materialize_agent_mcp_servers(&definition, &[], cwd.path());
    assert!(!has_server(&got, "disabled"));
    assert!(!has_server(&got, "needs-setup"));
    assert!(!has_server(&got, "empty-url"));
    assert_eq!(
        Some("from-agent".to_owned()),
        header_value(named_server(&got, "keeper"), "X-Token")
    );
}

#[test]
fn rematerialize_honors_parent_kill_switch_from_worktree() {
    let parent = cwd_with_enabled_false("agent_md_ks_adder");
    let worktree = tempfile::tempdir().unwrap();
    let definition = definition_with(vec![inline_http(
        "agent_md_ks_adder",
        "https://agent.example/mcp",
        "X-Token",
        "from-agent",
    )]);
    let without_parent = rematerialize(&definition, worktree.path(), None);
    assert!(
        has_server(&without_parent, "agent_md_ks_adder"),
        "seat cwd alone cannot see the parent project kill switch"
    );
    let got = rematerialize(&definition, worktree.path(), Some(parent.path()));
    assert!(
        !has_server(&got, "agent_md_ks_adder"),
        "reload/rebuild rematerialize must honor the parent project kill switch"
    );
}

#[test]
fn child_rematerialize_does_not_regain_parent_mcp_excluded_by_inheritance() {
    let parent = cwd_with_project_mcp("agent_md_inherit_x", true);
    let worktree = tempfile::tempdir().unwrap();
    let mut child = definition_with(vec![inline_http(
        "owned",
        "https://child.example/mcp",
        "X-Token",
        "owned",
    )]);
    child.mcp_inheritance =
        a3s_code_agent::config::McpInheritance::Except(vec!["agent_md_inherit_x".to_owned()]);

    let via_parent = rematerialize(&child, parent.path(), None);
    assert!(
        has_server(&via_parent, "agent_md_inherit_x"),
        "parent cwd merge would reload the excluded parent server"
    );

    let plugin = rematerialize(&child, worktree.path(), Some(parent.path()));
    assert!(has_server(&plugin, "owned"));
    assert!(
        !has_server(&plugin, "agent_md_inherit_x"),
        "plugin rematerialize must not reload a parent server mcpInheritance excluded"
    );

    let live = rematerialize(&child, worktree.path(), Some(parent.path()));
    let rebuild = rematerialize_live(
        &live,
        &child,
        &child,
        worktree.path(),
        Some(parent.path()),
        vec![],
    );
    assert!(has_server(&rebuild, "owned"));
    assert!(
        !has_server(&rebuild, "agent_md_inherit_x"),
        "rebuild rematerialize must not reload a parent server mcpInheritance excluded"
    );
}
