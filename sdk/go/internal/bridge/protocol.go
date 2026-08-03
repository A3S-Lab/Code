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
	"agent_replace_session",
	"agent_session_for_agent",
	"agent_session_for_worker",
	"agent_list_sessions",
	"agent_close_session",
	"agent_disconnect_idle_mcp",
	"agent_serve_agent_dir",
	"agent_serve_status",
	"agent_stop_serve",
	"agent_is_closed",
	"agent_close",
	"session_create",
	"session_resume",
	"session_info",
	"session_is_closed",
	"session_send",
	"session_resume_run",
	"session_send_with_attachments",
	"session_stream",
	"session_stream_with_attachments",
	"session_parallel",
	"session_parallel_resumable",
	"session_workflow_step",
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
	"session_governed_tool",
	"session_runs",
	"session_run_snapshot",
	"session_run_events",
	"session_run_event_page",
	"session_current_run",
	"session_active_tools",
	"session_subagent_task",
	"session_subagent_tasks",
	"session_pending_subagent_tasks",
	"session_cancel_subagent_task",
	"session_cancel_run",
	"session_pending_confirmations",
	"session_confirm_tool_use",
	"session_cancel_confirmations",
	"session_verification_reports",
	"session_record_verification_reports",
	"session_verification_summary",
	"session_verification_summary_text",
	"session_verification_presets",
	"session_verify_commands",
	"session_register_agent_dir",
	"session_register_worker_agent",
	"session_register_worker_agents",
	"session_add_skill",
	"session_remove_skill",
	"session_skill_names",
	"session_register_dynamic_workflow",
	"session_unregister_dynamic_tool",
	"session_add_mcp_server",
	"session_remove_mcp_server",
	"session_mcp_status",
	"session_has_memory",
	"session_remember_success",
	"session_remember_failure",
	"session_recall_similar",
	"session_recall_by_tags",
	"session_memory_recent",
	"session_memory_stats",
	"session_get_working_memory",
	"session_clear_working_memory",
	"session_get_short_term_memory",
	"session_clear_short_term_memory",
	"session_has_queue",
	"session_set_lane_handler",
	"session_complete_external_task",
	"session_pending_external_tasks",
	"session_queue_stats",
	"session_dead_letters",
	"session_queue_metrics",
	"session_register_hook",
	"session_unregister_hook",
	"session_hook_count",
	"session_set_budget_guard",
	"session_register_command",
	"session_list_commands",
	"callback_response",
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
	Callback        *Callback       `json:"callback"`
	Error           *RemoteError    `json:"error"`
}

type RemoteError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type Callback struct {
	CallbackID uint64          `json:"callback_id"`
	HandlerID  string          `json:"handler_id"`
	Method     string          `json:"method"`
	Payload    json.RawMessage `json:"payload"`
	TimeoutMS  uint64          `json:"timeout_ms"`
}
