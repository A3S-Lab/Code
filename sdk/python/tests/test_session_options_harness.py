"""Meta Harness composition through the Python SDK (no provider credentials needed)."""

import pytest

from a3s_code import Agent, Harness, SessionOptions

INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
""".strip()


def test_compose_accepts_stock_and_host_components():
    recipe = Harness.compose(
        components=[
            Harness.system(),
            Harness.tools(),
            Harness.host("intent_stamp"),
            Harness.budget(),
            Harness.infer(),
        ],
        tool_budget=4,
    )
    assert recipe.components == ["system", "tools", "host:intent_stamp", "budget", "infer"]
    assert recipe.tool_budget == 4


def test_compose_rejects_unknown_part():
    with pytest.raises(ValueError):
        Harness.compose(components=["system", "not-a-part"])


def test_session_accepts_host_mount_recipe(tmp_path):
    agent = Agent.create(INLINE_CONFIG)
    options = SessionOptions()
    options.harness = Harness.compose(
        components=[Harness.system(), Harness.host("intent_stamp"), Harness.infer()]
    )
    session = agent.session(str(tmp_path), options)
    try:
        assert options.harness.components == ["system", "host:intent_stamp", "infer"]
    finally:
        session.close()
