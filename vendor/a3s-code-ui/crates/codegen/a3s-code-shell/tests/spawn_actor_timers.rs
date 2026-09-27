#[allow(dead_code)]
mod acp_harness;

use acp_harness::assert_probes_emitted;

const SPAWN_PROBES: [&str; 4] = [
    "session.spawn_actor_call",
    "session.spawn_actor.agent_build",
    "session.agent_build.tool_registry",
    "session.agent_build.prompt_render",
];

#[test]
fn spawn_actor_emits_agent_build_probes() {
    assert_probes_emitted("spawn-actor-timers", &SPAWN_PROBES);
}
