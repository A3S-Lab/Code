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
    for _, _ in selector.select(timeout):
        try:
            block = os.read(master, 65536)
        except OSError:
            return
        if not block:
            return
        raw.extend(block)
        events.append([round(time.monotonic() - started, 4), len(raw)])
        if len(raw) > 4 * 1024 * 1024:
            raise RuntimeError("capture exceeded 4 MiB")
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
    wait_for(["A3S Code  9.1.3", "Shift+Tab", "shortcuts"], 25)
    checkpoint("01-ready")
    prompt = "读取 title.rs，用三条要点说明空输入逻辑，每条不超过20字。"
    os.write(master, prompt.encode())
    wait_for([prompt], 8)
    checkpoint("02-draft")
    os.write(master, b"\r")
    wait_for(["Worked for", "Turn failed"], 120)
    response = checkpoint("03-response")
    os.write(master, b"/")
    wait_for(["Switch the active model", "settings", "/model"], 8)
    checkpoint("04-palette")
    (CAP / "all-output.vt").write_bytes(raw)
    (CAP / "native-result.json").write_text(json.dumps({
        "success": "Turn failed" not in response["text"],
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
