---
name: builtin-tools
description: Built-in file operation and shell tools
version: 1.0.0
kind: instruction
tags:
  - tools
  - documentation
tools:
  - name: read
    description: Read the contents of a file. Returns line-numbered output. Supports text files and images.
    backend:
      type: builtin
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
      type: builtin
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
      type: builtin
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
      type: builtin
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
    description: Execute a shell command in the workspace directory. On Windows this runs in PowerShell, not GNU bash. Use for running commands, installing packages, running tests, and local diagnostics.
    backend:
      type: builtin
    parameters:
      type: object
      properties:
        command:
          type: string
          description: The shell command to execute. On Windows, write PowerShell-compatible commands by default. A limited compatibility shim exists for curl, wget, bare HTTP verbs (GET/POST/PUT/PATCH/DELETE/OPTIONS), which, and head.
        timeout:
          type: integer
          description: Timeout in milliseconds (default 120000)
      required:
        - command

  - name: grep
    description: Search for a pattern in files using ripgrep. Returns matching lines with file paths and line numbers.
    backend:
      type: builtin
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
      type: builtin
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
      type: builtin
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
      type: builtin
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

  - name: box
    description: |
      Run code or commands inside an A3S Box secure sandbox (VM-level isolation).
      Use this when you need to execute untrusted code, run experiments safely,
      or isolate side effects from the host system.
      The sandbox has its own filesystem, network, and process namespace.
    backend:
      type: builtin
    parameters:
      type: object
      properties:
        command:
          type: string
          description: The command to execute inside the sandbox
        image:
          type: string
          description: "Sandbox image to use (default: ubuntu-22.04)"
        timeout:
          type: integer
          description: Timeout in milliseconds (default 60000)
        files:
          type: object
          description: Files to inject into the sandbox before execution (key=path, value=content)
      required:
        - command

  - name: git
    description: |
      Execute Git operations using the system git command. Supports: status, log, branch, checkout, diff, stash, remote, and worktree management. **Automatically downloads and installs git if not available** on Windows, macOS, and Linux.

      Auto-installation: Downloads official pre-built git binaries from GitHub/git-scm.com to `~/.local/git/bin/`. Does not require any package manager.
    backend:
      type: builtin
    parameters:
      type: object
      properties:
        command:
          type: string
          enum: ["status", "log", "branch", "checkout", "diff", "stash", "remote", "worktree"]
          description: "Required. Git command to execute."
        name:
          type: string
          description: Branch name for branch/checkout/worktree operations.
        path:
          type: string
          description: Path for worktree operations.
        ref:
          type: string
          description: Reference for checkout command.
        target:
          type: string
          description: Target ref for diff (e.g., HEAD~1).
        max_count:
          type: integer
          description: Maximum log entries to show (default 10).
        message:
          type: string
          description: Message for stash.
        include_untracked:
          type: boolean
          description: Include untracked files in stash.
        force:
          type: boolean
          description: Force checkout/create operation.
        new_branch:
          type: boolean
          description: Create new branch for worktree (default true).
        base:
          type: string
          description: Base ref for new branch (default HEAD).
      required:
        - command

  - name: web_search
    description: |
      Search the web using multiple search engines.
      - Aggregates results from multiple engines (DuckDuckGo, Wikipedia, Google, Brave, Baidu, etc.)
      - Supports proxy configuration for anti-crawler protection
      - Returns deduplicated and ranked results
    backend:
      type: builtin
    parameters:
      type: object
      properties:
        query:
          type: string
          description: The search query
        engines:
          type: array
          items:
            type: string
          description: "List of engines to use (default: [\"ddg\",\"wiki\"]). Available: ddg (DuckDuckGo), brave (Brave Search), wiki (Wikipedia), sogou (Sogou), 360 / so360 (360 Search). Note: use 'engines' (plural), not 'engine' (singular)."
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
---

# Built-in Tools

Core file operation and shell tools for A3S Code.

## Tools

- **read**: Read file contents with line numbers
- **write**: Write content to files
- **edit**: Edit files with string replacement
- **patch**: Apply unified diff patches to files
- **bash**: Execute shell commands in the workspace
- **box**: Run commands inside an A3S Box secure sandbox (VM-level isolation)
- **grep**: Search file contents with ripgrep
- **glob**: Find files by pattern
- **ls**: List directory contents
- **git**: Execute Git operations with auto-install support (auto-downloads git if not available)
- **web_fetch**: Fetch web content and convert to text/markdown
- **web_search**: Search the web using multiple search engines

## When to Use box vs bash

- Use **bash** for normal workspace operations (read files, run tests, build code)

## Windows Shell Rules

- On Windows, the **bash** tool executes the command through **PowerShell**, not GNU bash and not `cmd.exe`.
- Prefer native PowerShell syntax on Windows.
- For HTTP calls on Windows, prefer `curl.exe` or `Invoke-RestMethod`. A compatibility shim also accepts `curl`, `wget`, and bare verbs like `GET http://127.0.0.1:18790/health`.
- Do not assume Unix utilities or GNU shell behavior on Windows. Avoid patterns like `grep`, `sed`, `awk`, `tail -f`, `cmd1 && cmd2`, or bash-specific quoting unless you explicitly convert them to PowerShell syntax.
- If you need to inspect only the first few output lines in PowerShell, prefer `Select-Object -First N`. The compatibility shim also supports `head -N`.
- Use **box** when you need:
  - VM-level isolation from the host
  - Safe execution of untrusted or experimental code
  - Sandboxed network/filesystem access
  - Running code that might have side effects you want to contain

## Usage

These tools are native Rust implementations automatically available in every agent session.

Parameters are passed as JSON objects matching each tool's parameter schema defined in the YAML frontmatter above.

## Examples

### File Operations

```json
// Read a file
{"file_path": "src/main.rs", "offset": 0, "limit": 100}

// Write to a file
{"file_path": "output.txt", "content": "Hello, world!"}

// Edit a file
{"file_path": "config.json", "old_string": "\"debug\": false", "new_string": "\"debug\": true"}

// Apply a patch
{"file_path": "src/lib.rs", "diff": "@@ -1,3 +1,3 @@\n line1\n-old\n+new\n line3"}
```

### Search Operations

```json
// Search with grep
{"pattern": "fn main", "glob": "**/*.rs", "context": 3}

// Find files with glob
{"pattern": "**/*.{rs,toml}", "path": "."}

// List directory
{"path": "src"}
```

### Sandbox Execution (A3S Box)

```json
// Run a command in a secure sandbox
{"command": "python3 -c 'import os; print(os.listdir(\"/\"))'", "image": "ubuntu-22.04"}

// Run with injected files
{"command": "python3 /sandbox/script.py", "files": {"/sandbox/script.py": "print('hello from sandbox')"}}
```

### Execution

```json
// Run a bash command
{"command": "cargo test", "timeout": 120000}
```

### Web Operations

```json
// Fetch web content
{"url": "https://example.com", "format": "markdown", "timeout": 30}

// Search the web
{"query": "rust async programming", "engines": ["ddg", "wiki"], "limit": 10}
```

### Git Operations

```json
// Show repository status
{"command": "status"}

// Show commit log
{"command": "log", "max_count": 5}

// List all branches
{"command": "branch"}

// Create a new branch
{"command": "branch", "name": "feature-x"}

// Checkout a branch
{"command": "checkout", "ref": "feature-x"}

// Show diff against HEAD
{"command": "diff"}

// Show diff between commits
{"command": "diff", "target": "HEAD~1"}

// List stashes
{"command": "stash"}

// Create a stash
{"command": "stash", "message": "WIP: work in progress"}

// Show remotes
{"command": "remote"}

// List all worktrees
{"command": "worktree", "command": "list"}

// Create a new worktree
{"command": "worktree", "command": "create", "name": "feature-x", "path": "../repo-feature-x"}

// Remove a worktree
{"command": "worktree", "command": "remove", "path": "../repo-feature-x", "force": true}
```
