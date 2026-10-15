"""Launch the real 9.1.3 pager/ACP pair; credentials stay in the child environment."""
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[4]
env = {
    key: os.environ[key]
    for key in ("PATH", "TMPDIR", "A3S_OPENAI_API_KEY", "A3S_OPENAI_BASE_URL")
    if key in os.environ
}
env.update(
    TERM="xterm-256color",
    COLORTERM="truecolor",
    LANG="en_US.UTF-8",
    A3S_CONFIG=os.environ["A3S_CONFIG_FILE"],
    A3S_HOME=str(ROOT / "runtime/home"),
    GROK_HOME=str(ROOT / "runtime/home/grok"),
    A3S_DISABLE_CC_SWITCH="1",
    A3S_DISABLE_GROK_ACCOUNT="1",
    A3S_DISABLE_CURSOR_ACCOUNT="1",
    A3S_ACP_AGENT_BIN=str(REPO / "apps/code/target/debug/a3s-code-acp"),
)
binary = REPO / "apps/code/vendor/a3s-code-ui/target/debug/a3s-code-tui"
os.execve(binary, [str(binary), "--no-auto-update", "--no-leader", "--cwd", str(ROOT / "workspace")], env)
