"""Python SessionOptions must expose verifier_enabled (#163)."""

from a3s_code import SessionOptions


def test_verifier_enabled_getter_setter_roundtrip():
    options = SessionOptions()
    assert options.verifier_enabled is None

    options.verifier_enabled = True
    assert options.verifier_enabled is True

    options.verifier_enabled = False
    assert options.verifier_enabled is False

    options.verifier_enabled = None
    assert options.verifier_enabled is None


def test_verifier_enabled_mirrors_allow_process_host_sandbox_pattern():
    options = SessionOptions()
    options.allow_process_host_sandbox = True
    options.verifier_enabled = True
    assert options.allow_process_host_sandbox is True
    assert options.verifier_enabled is True
