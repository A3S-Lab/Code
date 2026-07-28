// Package bridge contains the versioned machine protocol shared by the Go
// package and the installed a3s-code-go-bridge process.
package bridge

import "encoding/json"

const ProtocolVersion = 1
const EventProtocolVersion = 1

var RequiredOperations = []string{
	"sdk_capabilities",
	"agent_create",
	"agent_refresh_mcp_tools",
	"agent_list_sessions",
	"agent_close_session",
	"agent_is_closed",
	"agent_close",
	"session_create",
	"session_resume",
	"session_info",
	"session_is_closed",
	"session_send",
	"session_stream",
	"session_cancel",
	"session_cancel_and_settle",
	"session_history",
	"session_close",
	"session_save",
	"session_tool_names",
	"session_tool_definitions",
	"session_trace_events",
	"session_get_artifact",
	"session_read_file",
	"session_write_file",
	"session_ls",
	"session_edit_file",
	"session_patch_file",
	"session_bash",
	"session_glob",
	"session_grep",
	"session_tool",
	"session_runs",
	"session_run_snapshot",
	"session_run_events",
	"session_run_event_page",
	"session_current_run",
	"session_active_tools",
	"session_cancel_run",
	"session_pending_confirmations",
	"session_confirm_tool_use",
	"session_cancel_confirmations",
	"session_verification_reports",
	"session_verification_summary",
	"session_verification_summary_text",
	"session_verification_presets",
	"session_verify_commands",
	"session_register_agent_dir",
	"session_skill_names",
	"session_add_mcp_server",
	"session_remove_mcp_server",
	"session_mcp_status",
}

type Request struct {
	ProtocolVersion int            `json:"protocol_version"`
	ID              uint64         `json:"id"`
	Operation       string         `json:"operation"`
	Params          map[string]any `json:"params"`
}

type Envelope struct {
	ProtocolVersion int             `json:"protocol_version"`
	ID              uint64          `json:"id"`
	Kind            string          `json:"kind"`
	OK              bool            `json:"ok"`
	Result          json.RawMessage `json:"result"`
	Event           json.RawMessage `json:"event"`
	Error           *RemoteError    `json:"error"`
}

type RemoteError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}
