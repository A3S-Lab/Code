---
name: builtin-tools
description: Built-in file operation and shell tools
version: 1.0.0
tools:
  - name: read
    description: Read the contents of a file. Returns line-numbered output. Supports text files and images.
    backend:
      type: binary
      path: a3s-tools
      args_template: "read"
    parameters:
      type: object
      properties:
        file_path:
          type: string
          description: Path to the file to read (absolute or relative to workspace)
        offset:
          type: integer
          description: Line number to start reading from (0-indexed, default 0)
        limit:
          type: integer
          description: Maximum number of lines to read (default 2000)
      required:
        - file_path

  - name: write
    description: Write content to a file. Creates the file and parent directories if they don't exist.
    backend:
      type: binary
      path: a3s-tools
      args_template: "write"
    parameters:
      type: object
      properties:
        file_path:
          type: string
          description: Path to the file to write
        content:
          type: string
          description: Content to write to the file
      required:
        - file_path
        - content

  - name: edit
    description: Edit a file by replacing a specific string with another. The old_string must be unique in the file unless replace_all is true.
    backend:
      type: binary
      path: a3s-tools
      args_template: "edit"
    parameters:
      type: object
      properties:
        file_path:
          type: string
          description: Path to the file to edit
        old_string:
          type: string
          description: The exact string to replace (must be unique unless replace_all=true)
        new_string:
          type: string
          description: The string to replace it with
        replace_all:
          type: boolean
          description: Replace all occurrences (default false)
      required:
        - file_path
        - old_string
        - new_string

  - name: patch
    description: Apply a unified diff patch to a file. Use this for complex multi-line edits where the edit tool would be cumbersome. The diff must be in unified diff format with @@ hunk headers.
    backend:
      type: binary
      path: a3s-tools
      args_template: "patch"
    parameters:
      type: object
      properties:
        file_path:
          type: string
          description: Path to the file to patch
        diff:
          type: string
          description: "Unified diff content with @@ hunk headers. Example:\n@@ -1,3 +1,3 @@\n line1\n-old_line\n+new_line\n line3"
      required:
        - file_path
        - diff

  - name: bash
    description: Execute a bash command in the workspace directory. Use for running commands, installing packages, running tests, etc.
    backend:
      type: binary
      path: a3s-tools
      args_template: "bash"
    parameters:
      type: object
      properties:
        command:
          type: string
          description: The bash command to execute
        timeout:
          type: integer
          description: Timeout in milliseconds (default 120000)
      required:
        - command

  - name: grep
    description: Search for a pattern in files using ripgrep. Returns matching lines with file paths and line numbers.
    backend:
      type: binary
      path: a3s-tools
      args_template: "grep"
    parameters:
      type: object
      properties:
        pattern:
          type: string
          description: Regular expression pattern to search for
        path:
          type: string
          description: Directory or file to search in (default workspace root)
        glob:
          type: string
          description: Glob pattern to filter files (e.g., '*.rs', '*.{ts,tsx}')
        context:
          type: integer
          description: Number of context lines to show before and after matches
        -i:
          type: boolean
          description: Case insensitive search
      required:
        - pattern

  - name: glob
    description: Find files matching a glob pattern. Returns a list of file paths.
    backend:
      type: binary
      path: a3s-tools
      args_template: "glob"
    parameters:
      type: object
      properties:
        pattern:
          type: string
          description: Glob pattern to match (e.g., '**/*.rs', 'src/**/*.ts')
        path:
          type: string
          description: Base directory for the search (default workspace root)
      required:
        - pattern

  - name: ls
    description: List contents of a directory with file types and sizes.
    backend:
      type: binary
      path: a3s-tools
      args_template: "ls"
    parameters:
      type: object
      properties:
        path:
          type: string
          description: Directory path to list (default workspace root)
      required: []

  - name: web_fetch
    description: |
      Fetch content from a URL and convert to text or markdown.
      - Supports HTML to Markdown conversion
      - 5MB response size limit
      - Configurable timeout (max 120 seconds)
    backend:
      type: binary
      path: a3s-tools
      args_template: "web-fetch"
    parameters:
      type: object
      properties:
        url:
          type: string
          description: The URL to fetch content from (must start with http:// or https://)
        format:
          type: string
          enum: ["markdown", "text", "html"]
          description: "Output format (default: markdown)"
        timeout:
          type: integer
          description: "Timeout in seconds (default: 30, max: 120)"
      required:
        - url

  - name: web_search
    description: |
      Search the web using multiple search engines.
      - Aggregates results from multiple engines (DuckDuckGo, Wikipedia, Google, Brave, Baidu, etc.)
      - Supports proxy configuration for anti-crawler protection
      - Returns deduplicated and ranked results
    backend:
      type: binary
      path: a3s-tools
      args_template: "web-search"
    parameters:
      type: object
      properties:
        query:
          type: string
          description: The search query
        engines:
          type: string
          description: "Comma-separated list of engines to use (default: ddg,wiki). Available: ddg, brave, google, wiki, baidu, sogou, bing_cn, 360"
        limit:
          type: integer
          description: "Maximum number of results to return (default: 10, max: 50)"
        timeout:
          type: integer
          description: "Search timeout in seconds (default: 10, max: 60)"
        proxy:
          type: string
          description: "Proxy URL (e.g., http://127.0.0.1:8080 or socks5://127.0.0.1:1080)"
        format:
          type: string
          enum: ["text", "json"]
          description: "Output format (default: text)"
      required:
        - query

  - name: cron
    description: |
      Manage cron jobs for scheduled task execution.
      - Standard 5-field cron syntax (minute hour day month weekday)
      - Natural language schedule support (English & Chinese)
      - Task persistence and monitoring
      - CRUD operations: create, pause, update, terminate jobs
      - Execution history tracking
    backend:
      type: binary
      path: a3s-tools
      args_template: "cron"
    parameters:
      type: object
      properties:
        action:
          type: string
          enum: ["list", "add", "get", "update", "pause", "resume", "remove", "history", "run", "parse"]
          description: "Action to perform"
        id:
          type: string
          description: "Job ID (required for get, update, pause, resume, remove, history, run)"
        name:
          type: string
          description: "Job name (required for add, optional for get)"
        schedule:
          type: string
          description: "Schedule expression - supports cron syntax OR natural language like 'every 5 minutes', 'daily at 2am', '每天凌晨2点'"
        command:
          type: string
          description: "Command to execute (required for add, optional for update)"
        timeout:
          type: integer
          description: "Execution timeout in milliseconds (default: 60000)"
        limit:
          type: integer
          description: "Number of history records to return (default: 10, for history action)"
        input:
          type: string
          description: "Natural language input to parse (for parse action)"
      required:
        - action
---

# Built-in Tools

Core file operation and shell tools for A3S.

## Tools

- **read**: Read file contents with line numbers
- **write**: Write content to files
- **edit**: Edit files with string replacement
- **patch**: Apply unified diff patches to files
- **bash**: Execute shell commands
- **grep**: Search file contents with ripgrep
- **glob**: Find files by pattern
- **ls**: List directory contents
- **web_fetch**: Fetch web content and convert to text/markdown
- **web_search**: Search the web using multiple search engines
- **cron**: Manage cron jobs for scheduled task execution

## Usage

These tools are automatically loaded when A3S starts. They are implemented as a unified binary (`a3s-tools`) with subcommands for each tool.

Parameters are passed via the `TOOL_ARGS` environment variable as JSON, and the workspace is determined from the current directory.

## Cron Examples

```bash
# List all cron jobs
export TOOL_ARGS='{"action":"list"}'
a3s-tools cron

# Add a job using natural language (English)
export TOOL_ARGS='{"action":"add","name":"backup","schedule":"every day at 2am","command":"./backup.sh"}'
a3s-tools cron

# Add a job using natural language (Chinese)
export TOOL_ARGS='{"action":"add","name":"cleanup","schedule":"每天凌晨3点","command":"rm -rf /tmp/cache/*"}'
a3s-tools cron

# Add a job using cron syntax
export TOOL_ARGS='{"action":"add","name":"heartbeat","schedule":"*/5 * * * *","command":"./ping.sh"}'
a3s-tools cron

# Parse natural language to cron expression
export TOOL_ARGS='{"action":"parse","input":"every monday at 9am"}'
a3s-tools cron

# Pause a job
export TOOL_ARGS='{"action":"pause","id":"<job-id>"}'
a3s-tools cron

# Resume a job
export TOOL_ARGS='{"action":"resume","id":"<job-id>"}'
a3s-tools cron

# View execution history
export TOOL_ARGS='{"action":"history","id":"<job-id>","limit":20}'
a3s-tools cron

# Manually run a job
export TOOL_ARGS='{"action":"run","id":"<job-id>"}'
a3s-tools cron

# Remove a job
export TOOL_ARGS='{"action":"remove","id":"<job-id>"}'
a3s-tools cron
```

### Natural Language Schedule Examples

**English:**
- `every minute` / `every 5 minutes`
- `every hour` / `every 2 hours`
- `daily at 2am` / `every day at 14:30`
- `every monday at 9am` / `every friday at 5pm`
- `every weekday at 8:30` / `every weekend at 10am`
- `monthly on the 1st` / `every month on the 15th at 2am`

**Chinese (中文):**
- `每分钟` / `每5分钟`
- `每小时` / `每2小时`
- `每天凌晨2点` / `每天下午3点30分`
- `每周一上午9点` / `每周五下午5点`
- `工作日早上8点` / `周末上午10点`
- `每月1号` / `每月15日凌晨2点`

### Cron Schedule Syntax

```
┌───────────── minute (0-59)
│ ┌───────────── hour (0-23)
│ │ ┌───────────── day of month (1-31)
│ │ │ ┌───────────── month (1-12)
│ │ │ │ ┌───────────── day of week (0-6, 0=Sunday)
│ │ │ │ │
* * * * *
```

Special characters:
- `*` - any value
- `,` - value list (e.g., `1,3,5`)
- `-` - range (e.g., `1-5`)
- `/` - step (e.g., `*/5` for every 5 units)
