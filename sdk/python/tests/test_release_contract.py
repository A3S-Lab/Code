"""Release bindings preserve actual Core Run identity without external models."""

import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import get_args

import pytest

from a3s_code import Agent, CodeErrorCode, PermissionPolicy, SessionOptions


def test_original_run_policy_evidence_and_option_refusal(tmp_path):
    release_partial = threading.Event()
    blocking_response = threading.Event()

    class Peer(BaseHTTPRequestHandler):
        calls = 0
        counter_lock = threading.Lock()

        def do_POST(self):
            self.rfile.read(int(self.headers["Content-Length"]))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            time.sleep(0.04)
            with self.counter_lock:
                Peer.calls += 1
                call = Peer.calls
            values = [
                {"id": "fixture", "choices": [{"index": 0, "delta": {
                    "role": "assistant", "content": "fixture-answer"}, "finish_reason": None}]},
                {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                 "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13}},
            ]
            for index, value in enumerate(values):
                self.wfile.write(f"data: {json.dumps(value)}\n\n".encode())
                self.wfile.flush()
                if call in (3, 5) and index == 0:
                    if call == 5:
                        blocking_response.set()
                    release_partial.wait(timeout=10)
                    return
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()

        def log_message(self, *_):
            pass

    peer = ThreadingHTTPServer(("127.0.0.1", 0), Peer)
    thread = threading.Thread(target=peer.serve_forever, daemon=True)
    thread.start()
    config = f'''default_model = "openai/fixture"
providers "openai" {{
  api_key = "local-fixture-unused"
  base_url = "http://127.0.0.1:{peer.server_port}/v1"
  models "fixture" {{ name = "Fixture" }}
}}
memory {{ llm_extraction = false }}'''
    agent = Agent.create(config)
    try:
        invalid = SessionOptions()
        invalid.workspace_read_overlay = {"../outside.txt": "refuse"}
        with pytest.raises(ValueError, match="boundary"):
            agent.session(str(tmp_path), invalid)
        (tmp_path / "buffer.txt").write_text("saved\n")
        (tmp_path / "disk.txt").write_text("disk\n")
        overlay_options = SessionOptions()
        overlay_options.workspace_read_overlay = {"./buffer.txt": "unsaved\n"}
        overlay_options.permission_policy = PermissionPolicy(default_decision="allow")
        overlay = agent.session(str(tmp_path), overlay_options)
        try:
            assert "unsaved" in overlay.read_file("buffer.txt")
            assert "disk" in overlay.read_file("disk.txt")
            assert (tmp_path / "buffer.txt").read_text() == "saved\n"
        finally:
            overlay.close()
        options = SessionOptions()
        with pytest.raises(ValueError, match="sandbox network mode"):
            options.sandbox_network = "invalid"
        options.path_injection_budget = 0
        with pytest.raises(RuntimeError, match="budget"):
            agent.session(str(tmp_path), options)
        options.path_injection_budget = 4096
        options.verification_contracts = [{"id": "", "command": "", "covers": []}]
        with pytest.raises(RuntimeError, match="contract"):
            agent.session(str(tmp_path), options)
        options.verification_contracts = []
        options.session_id = "python-release-contract"
        options.planning_mode = "disabled"
        options.manual_delegation_enabled = False
        options.continuation_enabled = False
        options.watch_skill_dirs = [str(tmp_path / "future-skills")]
        options.projected_skill_files = []
        options.sandbox_network = "deny"
        session = agent.session(str(tmp_path), options)
        with pytest.raises(ValueError, match="posture"):
            session.set_permission_posture("invalid")
        session.set_permission_posture("plan")
        policy = PermissionPolicy(default_decision="deny")
        session.set_permission_policy(policy)
        assert isinstance(session.projected_capability_names(), list)
        local = session.stream_with_run("/help")
        assert local["run"] is None
        list(local["stream"])
        first = session.stream_with_run("Return fixture-answer without tools.")
        assert first["run"].session_id == session.session_id
        list(first["stream"])
        second = session.stream_with_run("Repeat fixture-answer without tools.")
        assert second["run"].id != first["run"].id
        assert not first["run"].cancel()
        list(second["stream"])
        assert second["run"].snapshot()["status"] == "completed"
        context = session.context_report()
        assert context["usage"]["prompt_tokens"] == 20
        assert context["usage"]["completion_tokens"] == 6
        assert context["tool_results"] == 0
        assert context["heaviest"] == []
        request = {"target": {"schema": "a3s.code.execution-target.v1",
                              "session_id": session.session_id, "run_id": second["run"].id}}
        assert session.read_evaluation_evidence(request)["target"] == request["target"]
        request["target"]["session_id"] = "foreign"
        with pytest.raises(RuntimeError, match="not found"):
            session.read_evaluation_evidence(request)
        cancelled = session.stream_with_run("Cancel this partial fixture response.")
        types = []
        accepted = False
        for event in cancelled["stream"]:
            types.append(event.type)
            if event.type == "text_delta" and not accepted:
                accepted = cancelled["run"].cancel()
                assert accepted
        assert accepted and types.count("agent_end") == 0 and types.count("error") == 1
        assert cancelled["run"].snapshot()["status"] == "cancelled"
        continued = session.stream_with_run("Continue after cancellation.")
        assert continued["run"].id != cancelled["run"].id
        assert not cancelled["run"].cancel()
        list(continued["stream"])
        assert continued["run"].snapshot()["status"] == "completed"
        assert Peer.calls == 4
        with ThreadPoolExecutor(max_workers=1) as caller:
            blocking = caller.submit(session.send, "Cancel this blocking partial response.")
            assert blocking_response.wait(timeout=5)
            active = session.current_run()
            assert active["id"]
            assert session.cancel_run(active["id"])
            with pytest.raises(RuntimeError) as failure:
                blocking.result(timeout=5)
            assert failure.value.code == "RUN_CANCELLED"
            assert failure.value.code in get_args(CodeErrorCode)
            assert active["id"] in str(failure.value)
        assert session.run_snapshot(active["id"])["status"] == "cancelled"
        after_blocking = session.stream_with_run("Continue after blocking cancellation.")
        assert after_blocking["run"].id != active["id"]
        assert not session.cancel_run(active["id"])
        list(after_blocking["stream"])
        assert after_blocking["run"].snapshot()["status"] == "completed"
        assert Peer.calls == 6
        assert session.fork_session_at(2**64 - 1) == 0
        with pytest.raises(OverflowError):
            session.fork_session_at(-1)
    finally:
        agent.close()
        release_partial.set()
        peer.shutdown()
        peer.server_close()
        thread.join(timeout=2)
