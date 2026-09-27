//! The name GrokBuild offers for every tool a script can call, and the GrokBuild shape of its
//! arguments. A case writes arguments once, and [`Tool::pick`] fills required fields a case omits,
//! reshapes a call written in another tool's vocabulary, and resolves it against the names offered.
use crate::inference_request::{HistoryToolCall, OfferedTools};
use serde_json::{Map, Value, json};
use std::fmt;
const MCP_NAME_SEPARATOR: &str = "__";
/// The meta tool the shell offers for MCP dispatch when tool search is on; the model names
/// the target in its `tool_name`/`tool_input` arguments rather than calling `server__tool` directly.
const USE_TOOL_NAME: &str = "use_tool";
/// The subagent spawn tool's name in each toolset: GrokBuild spells it `spawn_subagent`, the
/// daemon worker spells it `Task`. Single-sourced so the two spellings cannot drift.
pub const GROK_BUILD_SPAWN_TOOL: &str = "spawn_subagent";
pub const DAEMON_SPAWN_TOOL: &str = "Task";
/// The task id a task call gets when the case names none.
pub(crate) const FIRST_TASK_ID: &str = "1";
/// The interval a cron call gets when the case names none.
pub(crate) const CRON_DEFAULT_INTERVAL: &str = "1h";
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    Shell,
    Read,
    Edit,
    Write,
    Grep,
    Glob,
    List,
    MemorySearch,
    MemoryGet,
    Task,
    Skill,
    SendMessage,
    EnterPlanMode,
    ExitPlanMode,
    WebFetch,
    WebSearch,
    Question,
    Todo,
    SearchTool,
    KillTask,
    Monitor,
    SchedulerCreate,
    SchedulerList,
    SchedulerDelete,
    Workflow,
    Lsp,
    /// A created task in the cases' vocabulary; becomes one todo write.
    TaskCreate,
    /// A task update in the cases' vocabulary; also one todo write.
    TaskUpdate,
    /// A scheduled wakeup in the cases' vocabulary; becomes a scheduler create, defaulting the interval.
    CronCreate,
    McpListResources,
    McpReadResource,
    /// GrokBuild offers `server__tool` taking the arguments directly.
    Mcp {
        server: String,
        tool: String,
    },
}
impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tool::Shell
            | Tool::Read
            | Tool::Edit
            | Tool::Write
            | Tool::Grep
            | Tool::Glob
            | Tool::List
            | Tool::MemorySearch
            | Tool::MemoryGet
            | Tool::Task
            | Tool::Skill
            | Tool::SendMessage
            | Tool::EnterPlanMode
            | Tool::ExitPlanMode
            | Tool::WebFetch
            | Tool::WebSearch
            | Tool::Question
            | Tool::Todo
            | Tool::SearchTool
            | Tool::KillTask
            | Tool::Monitor
            | Tool::SchedulerCreate
            | Tool::SchedulerList
            | Tool::SchedulerDelete
            | Tool::Workflow
            | Tool::Lsp
            | Tool::TaskCreate
            | Tool::TaskUpdate
            | Tool::CronCreate
            | Tool::McpListResources
            | Tool::McpReadResource => write!(f, "{self:?}"),
            Tool::Mcp { server, tool } => write!(f, "{tool} on MCP server {server}"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickedToolCall {
    pub(crate) name: String,
    pub(crate) arguments: Value,
}
/// A field the tool requires that a case may leave out, filled from another field of the call.
#[derive(Clone, Copy)]
struct FieldFill {
    field: &'static str,
    source: &'static str,
}
/// A reshaping of a call's arguments a table of renames cannot express.
type Shape = fn(Map<String, Value>) -> Map<String, Value>;
struct GrokBuildRow {
    /// `None` for the MCP resource kinds, and for an MCP call, whose name is built from
    /// the server and tool.
    name: Option<&'static str>,
    fills: &'static [FieldFill],
    shape: Option<Shape>,
}
impl GrokBuildRow {
    const fn new(name: &'static str) -> Self {
        GrokBuildRow {
            name: Some(name),
            fills: &[],
            shape: None,
        }
    }
    const fn without_grok_build_name() -> Self {
        GrokBuildRow {
            name: None,
            fills: &[],
            shape: None,
        }
    }
    const fn with_fills(mut self, fills: &'static [FieldFill]) -> Self {
        self.fills = fills;
        self
    }
    const fn with_shape(mut self, shape: Shape) -> Self {
        self.shape = Some(shape);
        self
    }
}
impl Tool {
    pub fn mcp(server: impl Into<String>, tool: impl Into<String>) -> Self {
        Tool::Mcp {
            server: server.into(),
            tool: tool.into(),
        }
    }
    fn row(&self) -> GrokBuildRow {
        match self {
            Tool::Shell => GrokBuildRow::new("run_terminal_command").with_fills(&[FieldFill {
                field: "description",
                source: "command",
            }]),
            Tool::Read => GrokBuildRow::new("read_file"),
            Tool::Edit => GrokBuildRow::new("search_replace"),
            Tool::Write => GrokBuildRow::new("write"),
            Tool::Grep => GrokBuildRow::new("grep"),
            Tool::Glob => GrokBuildRow::new("glob"),
            Tool::List => GrokBuildRow::new("list_dir"),
            Tool::MemorySearch => GrokBuildRow::new("memory_search"),
            Tool::MemoryGet => GrokBuildRow::new("memory_get"),
            Tool::Task => GrokBuildRow::new(GROK_BUILD_SPAWN_TOOL).with_fills(&[FieldFill {
                field: "description",
                source: "prompt",
            }]),
            Tool::Skill => GrokBuildRow::new("skill"),
            Tool::SendMessage => GrokBuildRow::new("send_subagent_message"),
            Tool::EnterPlanMode => GrokBuildRow::new("enter_plan_mode"),
            Tool::ExitPlanMode => GrokBuildRow::new("exit_plan_mode"),
            Tool::WebFetch => GrokBuildRow::new("web_fetch"),
            Tool::WebSearch => GrokBuildRow::new("web_search"),
            Tool::Question => GrokBuildRow::new("ask_user_question").with_shape(describe_options),
            Tool::Todo => GrokBuildRow::new("todo_write").with_shape(default_todos_to_pending),
            Tool::SearchTool => GrokBuildRow::new("search_tool"),
            Tool::KillTask => GrokBuildRow::new("kill_command_or_subagent"),
            Tool::Monitor => GrokBuildRow::new("monitor"),
            Tool::SchedulerCreate => GrokBuildRow::new("scheduler_create"),
            Tool::SchedulerList => GrokBuildRow::new("scheduler_list"),
            Tool::SchedulerDelete => GrokBuildRow::new("scheduler_delete"),
            Tool::Workflow => GrokBuildRow::new("workflow"),
            Tool::Lsp => GrokBuildRow::new("lsp"),
            Tool::TaskCreate => {
                GrokBuildRow::new("todo_write").with_shape(todo_write_from_created_task)
            }
            Tool::TaskUpdate => GrokBuildRow::new("todo_write").with_shape(todo_write_from_task),
            Tool::CronCreate => {
                GrokBuildRow::new("scheduler_create").with_shape(fill_cron_interval)
            }
            Tool::McpListResources | Tool::McpReadResource | Tool::Mcp { .. } => {
                GrokBuildRow::without_grok_build_name()
            }
        }
    }
    /// An MCP tool is `server__tool`; the MCP resource kinds have no GrokBuild name.
    fn grok_build_name(&self) -> Option<String> {
        if let Tool::Mcp { server, tool } = self {
            return Some(format!("{server}{MCP_NAME_SEPARATOR}{tool}"));
        }
        self.row().name.map(str::to_owned)
    }
    /// The case's arguments in GrokBuild's shape: the fields a case may leave out are filled, and
    /// the row's shape applies. Arguments that are not a table pass through unchanged.
    fn grok_build_arguments(&self, arguments: &Value) -> Value {
        let Some(fields) = arguments.as_object() else {
            return arguments.clone();
        };
        let row = self.row();
        let mut shaped = fields.clone();
        for fill in row.fills {
            if let Some(value) = shaped.get(fill.source).cloned() {
                shaped.entry(fill.field).or_insert(value);
            }
        }
        Value::Object(match row.shape {
            Some(shape) => shape(shaped),
            None => shaped,
        })
    }
    /// The call for GrokBuild's name when the request offers it.
    pub(crate) fn pick(&self, offered: &OfferedTools, arguments: &Value) -> Option<PickedToolCall> {
        let arguments = self.grok_build_arguments(arguments);
        if let Some(name) = self.grok_build_name().filter(|name| offered.has_tool(name)) {
            return Some(PickedToolCall { name, arguments });
        }
        if let Tool::Mcp { .. } = self
            && offered.has_tool(USE_TOOL_NAME)
            && let Some(name) = self.grok_build_name()
        {
            return Some(PickedToolCall {
                name: USE_TOOL_NAME.to_owned(),
                arguments: json!({
                    "tool_name": name,
                    "tool_input": arguments,
                }),
            });
        }
        None
    }
    /// Whether a call the agent carried back in its history is this tool's under GrokBuild's name.
    pub(crate) fn is_called_by(&self, call: &HistoryToolCall) -> bool {
        if self.grok_build_name().is_some_and(|name| call.name == name) {
            return true;
        }
        if let Tool::Mcp { .. } = self
            && call.name == USE_TOOL_NAME
            && call.arguments.get("tool_name").and_then(Value::as_str)
                == self.grok_build_name().as_deref()
        {
            return true;
        }
        false
    }
}
/// Every question option gets its `label` as the description the tool requires.
fn describe_options(mut fields: Map<String, Value>) -> Map<String, Value> {
    for option in fields
        .get_mut("questions")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(|question| question.get_mut("options"))
        .filter_map(Value::as_array_mut)
        .flatten()
        .filter_map(Value::as_object_mut)
    {
        if let Some(label) = option.get("label").cloned() {
            option.entry("description").or_insert(label);
        }
    }
    fields
}
/// Every todo item without a `status` is pending.
fn default_todos_to_pending(mut fields: Map<String, Value>) -> Map<String, Value> {
    for todo in fields
        .get_mut("todos")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut)
    {
        todo.entry("status").or_insert_with(|| json!("pending"));
    }
    fields
}
/// A created task is pending unless the case says otherwise.
fn todo_write_from_created_task(mut task: Map<String, Value>) -> Map<String, Value> {
    task.entry("status").or_insert_with(|| json!("pending"));
    todo_write_from_task(task)
}
/// One todo write from a task call, with the `subject` carried over as the todo content.
fn todo_write_from_task(task: Map<String, Value>) -> Map<String, Value> {
    let mut todo = Map::from_iter([(
        "id".to_owned(),
        task.get("id")
            .or_else(|| task.get("taskId"))
            .cloned()
            .unwrap_or_else(|| json!(FIRST_TASK_ID)),
    )]);
    if let Some(subject) = task.get("subject") {
        todo.insert("content".to_owned(), subject.clone());
    }
    if let Some(status) = task.get("status") {
        todo.insert("status".to_owned(), status.clone());
    }
    Map::from_iter([("todos".to_owned(), json!([todo]))])
}
/// The scheduler takes an `interval` and no cron expression; the `schedule` stays as written.
fn fill_cron_interval(mut fields: Map<String, Value>) -> Map<String, Value> {
    fields
        .entry("interval")
        .or_insert_with(|| json!(CRON_DEFAULT_INTERVAL));
    fields
}
#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
