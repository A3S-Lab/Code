package code

import (
	"context"
	"strings"
)

type ReadFileOptions struct {
	Offset *uint `json:"offset,omitempty"`
	Limit  *uint `json:"limit,omitempty"`
}

func (session *Session) Tool(
	ctx context.Context,
	name string,
	args any,
) (*ToolCallResult, error) {
	const op = "session_tool"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(name) == "" {
		return nil, invalid(op, "tool name cannot be empty")
	}
	params := session.params()
	params["name"] = name
	if args == nil {
		args = map[string]any{}
	}
	params["args"] = args
	var result ToolCallResult
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (session *Session) Program(
	ctx context.Context,
	options map[string]any,
) (*ToolCallResult, error) {
	return session.Tool(ctx, "program", options)
}

func (session *Session) Task(
	ctx context.Context,
	options DelegateTaskOptions,
) (*ToolCallResult, error) {
	if strings.TrimSpace(options.Description) == "" {
		return nil, invalid("task", "description cannot be empty")
	}
	return session.Tool(ctx, "task", options)
}

func (session *Session) DelegateTask(
	ctx context.Context,
	options DelegateTaskOptions,
) (*ToolCallResult, error) {
	return session.Task(ctx, options)
}

func (session *Session) Tasks(
	ctx context.Context,
	tasks []DelegateTaskOptions,
) (*ToolCallResult, error) {
	if len(tasks) == 0 {
		return nil, invalid("tasks", "at least one task is required")
	}
	return session.Tool(ctx, "parallel_task", map[string]any{"tasks": tasks})
}

func (session *Session) ParallelTask(
	ctx context.Context,
	tasks []DelegateTaskOptions,
) (*ToolCallResult, error) {
	return session.Tasks(ctx, tasks)
}

func (session *Session) WebSearch(
	ctx context.Context,
	options WebSearchOptions,
) (*ToolCallResult, error) {
	if strings.TrimSpace(options.Query) == "" {
		return nil, invalid("web_search", "query cannot be empty")
	}
	return session.Tool(ctx, "web_search", options)
}

func (session *Session) Git(
	ctx context.Context,
	options GitOptions,
) (*ToolCallResult, error) {
	if strings.TrimSpace(options.Command) == "" {
		return nil, invalid("git", "command cannot be empty")
	}
	return session.Tool(ctx, "git", options)
}

func (session *Session) GitCommand(
	ctx context.Context,
	args map[string]any,
) (*ToolCallResult, error) {
	if len(args) == 0 {
		return nil, invalid("git_command", "arguments cannot be empty")
	}
	return session.Tool(ctx, "git", args)
}

func (session *Session) ReadFile(
	ctx context.Context,
	path string,
	options *ReadFileOptions,
) (string, error) {
	const op = "session_read_file"
	if err := validateSession(session, ctx, op); err != nil {
		return "", err
	}
	if strings.TrimSpace(path) == "" {
		return "", invalid(op, "path cannot be empty")
	}
	params := session.params()
	params["path"] = path
	if options != nil {
		if options.Offset != nil {
			params["offset"] = *options.Offset
		}
		if options.Limit != nil {
			params["limit"] = *options.Limit
		}
	}
	var result struct {
		Content string `json:"content"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Content, err
}

func (session *Session) WriteFile(
	ctx context.Context,
	path string,
	content string,
) (*ToolCallResult, error) {
	const op = "session_write_file"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(path) == "" {
		return nil, invalid(op, "path cannot be empty")
	}
	params := session.params()
	params["path"] = path
	params["content"] = content
	return session.toolResultRequest(ctx, op, params)
}

// List lists a workspace directory. An empty path lists the workspace root.
func (session *Session) List(ctx context.Context, path string) (*ToolCallResult, error) {
	const op = "session_ls"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	params := session.params()
	if path != "" {
		params["path"] = path
	}
	return session.toolResultRequest(ctx, op, params)
}

// LS is an alias for List.
func (session *Session) LS(ctx context.Context, path string) (*ToolCallResult, error) {
	return session.List(ctx, path)
}

func (session *Session) EditFile(
	ctx context.Context,
	path string,
	oldString string,
	newString string,
	replaceAll bool,
) (*ToolCallResult, error) {
	const op = "session_edit_file"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(path) == "" {
		return nil, invalid(op, "path cannot be empty")
	}
	params := session.params()
	params["path"] = path
	params["old_string"] = oldString
	params["new_string"] = newString
	params["replace_all"] = replaceAll
	return session.toolResultRequest(ctx, op, params)
}

func (session *Session) PatchFile(
	ctx context.Context,
	path string,
	diff string,
) (*ToolCallResult, error) {
	const op = "session_patch_file"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(path) == "" {
		return nil, invalid(op, "path cannot be empty")
	}
	params := session.params()
	params["path"] = path
	params["diff"] = diff
	return session.toolResultRequest(ctx, op, params)
}

func (session *Session) Bash(ctx context.Context, command string) (string, error) {
	const op = "session_bash"
	if err := validateSession(session, ctx, op); err != nil {
		return "", err
	}
	if strings.TrimSpace(command) == "" {
		return "", invalid(op, "command cannot be empty")
	}
	params := session.params()
	params["command"] = command
	var result struct {
		Output string `json:"output"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Output, err
}

func (session *Session) Glob(ctx context.Context, pattern string) ([]string, error) {
	const op = "session_glob"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(pattern) == "" {
		return nil, invalid(op, "pattern cannot be empty")
	}
	params := session.params()
	params["pattern"] = pattern
	var result struct {
		Paths []string `json:"paths"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result.Paths, nil
}

func (session *Session) Grep(ctx context.Context, pattern string) (string, error) {
	const op = "session_grep"
	if err := validateSession(session, ctx, op); err != nil {
		return "", err
	}
	if strings.TrimSpace(pattern) == "" {
		return "", invalid(op, "pattern cannot be empty")
	}
	params := session.params()
	params["pattern"] = pattern
	var result struct {
		Output string `json:"output"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	return result.Output, err
}

func (session *Session) toolResultRequest(
	ctx context.Context,
	operation string,
	params map[string]any,
) (*ToolCallResult, error) {
	var result ToolCallResult
	if err := session.runtime.Request(ctx, operation, params, &result); err != nil {
		return nil, err
	}
	return &result, nil
}
