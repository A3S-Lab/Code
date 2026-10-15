"""Bounded reference capture using the shipped tests' native PTY query protocol."""
import fcntl
import json
import os
import pty
import selectors
import signal
import struct
import subprocess
import termios
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[4]
CAP = ROOT / "capture"
CONVERTER = Path("/private/tmp/a3s-launch-vt-cells")
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 110, 0, 0))

def own_terminal():
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

child = subprocess.Popen([
    str(REPO / "apps/code/target/debug/examples/acl_env_config"), "run",
    str(REPO / ".a3s/config.acl"), "/usr/bin/python3", str(ROOT / "launch_tui.py")
], stdin=slave, stdout=slave, stderr=slave, cwd=ROOT / "workspace", preexec_fn=own_terminal)
os.close(slave)
selector = selectors.DefaultSelector()
selector.register(master, selectors.EVENT_READ)
raw = bytearray()
events = []
responded = 0
started = time.monotonic()

def read_event(timeout=1):
    global responded
    action = ROOT / "runtime/action.txt"
    if action.exists():
        value = action.read_bytes()
        action.unlink()
        os.write(master, value)
    for _, _ in selector.select(timeout):
        try:
            block = os.read(master, 65536)
        except OSError:
            return
        if not block:
            return
        raw.extend(block)
        events.append([round(time.monotonic() - started, 4), len(raw)])
        if len(raw) > 16 * 1024 * 1024:
            raise RuntimeError("capture exceeded 16 MiB")
        # The pager's own PTY tests respond to CPR; A3S Test currently does not.
        pending = bytes(raw[max(0, responded - 20):])
        if b"\x1b[6n" in pending:
            os.write(master, b"\x1b[1;1R")
        if b"\x1b]11;?" in pending:
            os.write(master, b"\x1b]11;rgb:1414/1414/1414\x1b\\")
        responded = len(raw)

def screen():
    (CAP / "current.vt").write_bytes(raw)
    subprocess.run([str(CONVERTER), str(CAP / "current.vt"), str(CAP / "current.json")], check=True)
    return json.loads((CAP / "current.json").read_text())

def checkpoint(name):
    # Finish the current terminal paint, bounded by output quiescence rather
    # than a fixed sleep. The welcome logo itself refreshes at 12 Hz.
    deadline = time.monotonic() + .7
    while time.monotonic() < deadline:
        before = len(raw)
        read_event(.04)
        if len(raw) == before:
            break
    (CAP / (name + ".vt")).write_bytes(raw)
    subprocess.run([str(CONVERTER), str(CAP / (name + ".vt")), str(CAP / (name + ".json"))], check=True)
    data = json.loads((CAP / (name + ".json")).read_text())
    print(name + ":\n" + data["text"], flush=True)
    return data

def wait_for(needles, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        read_event(min(1, max(0, deadline - time.monotonic())))
        data = screen()
        text = data["text"]
        if "Do you trust the contents of this directory" in text:
            os.write(master, b"y")
            continue
        if any(needle in text for needle in needles):
            return data
        if child.poll() is not None:
            raise RuntimeError("pager exited before expected state")
    checkpoint("timeout")
    raise RuntimeError("reference state did not appear: " + repr(needles))

try:
    wait_for(["A3S Code  9.1.3"], 25)
    checkpoint("01-ready")
    prompt = "直接用 read 工具读取 title.py 和 test_title.py，不调用 bash，不执行命令。用三条要点解释空白输入处理，并列出已有测试覆盖的五种输入。"
    os.write(master, prompt.encode())
    wait_for(["五种输入。"], 8)
    checkpoint("02-draft")
    os.write(master, b"\r")
    deadline = time.monotonic() + 120
    next_snapshot = time.monotonic() + 1
    snapshot = 0
    while time.monotonic() < deadline:
        read_event(.2)
        data = screen()
        if time.monotonic() >= next_snapshot:
            snapshot += 1
            checkpoint(f"work-{snapshot:03}")
            next_snapshot = time.monotonic() + 1
        if "Worked for" in data["text"] or "Turn failed" in data["text"]:
            break
    else:
        raise RuntimeError("read-only turn timed out")
    response = checkpoint("03-response")
    if "Turn failed" in response["text"]:
        raise RuntimeError("read-only turn failed")
    followup = "还缺哪些边界测试？只读分析，给出三条建议，每条不超过20字。"
    os.write(master, followup.encode())
    wait_for([followup], 8)
    checkpoint("04-followup-draft")
    os.write(master, b"\r")
    # The previous Worked-for line remains until the new turn begins.
    wait_for(["Waiting for response", "Responding", "Thinking"], 15)
    wait_for(["Worked for", "Turn failed"], 120)
    checkpoint("05-followup")
    os.write(master, b"/")
    wait_for(["Switch the active model"], 8)
    checkpoint("06-palette")
    os.write(master, b"model")
    wait_for(["/model"], 8)
    checkpoint("07-model-draft")
    os.write(master, b"\r")
    wait_for(["From A3S ACL provider"], 8)
    checkpoint("08-model")
    (CAP / "all-output.vt").write_bytes(raw)
    (CAP / "native-result.json").write_text(json.dumps({
        "success": True,
        "source": "actual a3s-code-tui 9.1.3 and a3s-code-acp 9.1.3",
        "duration_s": round(time.monotonic() - started, 2),
        "output_bytes": len(raw), "events": events,
    }, indent=2) + "\n")

finally:
    if child.poll() is None:
        os.killpg(child.pid, signal.SIGTERM)
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
    selector.close()
    os.close(master)
