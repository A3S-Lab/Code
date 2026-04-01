# A3S Code

**Agentic Agent Framework.** A3S Code is a Rust library with native Python and Node.js bindings. Give an LLM a workspace, a set of tools, and a system prompt — it reads files, runs commands, searches code, and acts on results.

[![crates.io](https://img.shields.io/crates/v/a3s-code-core)](https://crates.io/crates/a3s-code-core)
[![PyPI](https://img.shields.io/pypi/v/a3s-code)](https://pypi.org/project/a3s-code/)
[![npm](https://img.shields.io/npm/v/@a3s-lab/code)](https://www.npmjs.com/package/@a3s-lab/code)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

---

## Install

```bash
# Python
pip install a3s-code

# Node.js
npm install @a3s-lab/code
```

---

## Quick Start

**1. Create an agent config** (`agent.hcl`):

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
```

**2. Run an agent session:**

```python
from a3s_code import Agent

agent = Agent.create("agent.hcl")
session = agent.session("/my-project")

result = session.send("Find all places where we handle authentication errors")
print(result.text)
```

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');
const session = agent.session('/my-project');

const result = await session.send('Find all places where we handle authentication errors');
console.log(result.text);
session.close(); // recommended in short-lived Node.js scripts
```

---

## What the LLM Can Do

**18 built-in tools** — available in sessions by default:

| Category   | Tools                                                         |
| ---------- | ------------------------------------------------------------- |
| Files      | `read`, `write`, `edit`, `patch`                              |
| Search     | `grep`, `glob`, `ls`                                          |
| Agentic    | `agentic_search`, `agentic_parse`                             |
| Shell      | `bash`                                                        |
| Web        | `web_fetch`, `web_search`                                     |
| Git        | `git_worktree`                                                |
| Delegation | `task`, `parallel_task`, `run_team`, `batch`, `Skill`         |

You can configure the built-in agentic tools from `config.hcl`:

```hcl
agentic_search {
  enabled       = true
  default_mode  = "fast"
  max_results   = 10
  context_lines = 2
}

agentic_parse {
  enabled          = true
  default_strategy = "auto"
  max_chars        = 8000
}

document_parser {
  enabled          = true
  max_file_size_mb = 50

  ocr {
    enabled    = false
    model      = "openai/gpt-4.1-mini"
    prompt     = "Extract text from scanned pages and preserve structure."
    max_images = 8
    dpi        = 144
  }
}
```

The `document_parser.ocr` block configures OCR policy. Applications can still supply a custom OCR / vision backend at runtime via `SessionOptions`. When `document_parser.ocr.enabled=true`, the built-in parser can also auto-detect local OCR tooling when available:

- Images: if `tesseract` is available on `PATH` (or `A3S_DOCUMENT_OCR_TESSERACT_BIN` points to it), `CompositeDocumentParser` can OCR image files without an injected provider.
- PDFs: if both `tesseract` and `pdftoppm` are available (or `A3S_DOCUMENT_OCR_PDFTOPPM_BIN` points to `pdftoppm`), `CompositeDocumentParser` can rasterize pages and OCR PDFs without an injected provider.
- Custom providers supplied via `SessionOptions` still take precedence over the built-in fallback.

Current OCR execution matrix:

- `pdf`: when OCR is enabled, OCR fallback runs when native text extraction is weak or empty.
- `png` / `jpg` / `jpeg` / `webp` / `gif` / `bmp` / `tif` / `tiff`: when OCR is enabled, OCR is the primary decode path.
- `docx` / `pptx` / `xlsx` / `xlsm` / `odt` / `ods` / `odp`: when OCR is enabled, OCR fallback runs only when the native structured-text pass yields no extractable content.

Runtime normalization:

- `agentic_search.default_mode` accepts `fast`, `deep`, or `filename_only`; invalid values fall back to `fast`.
- `agentic_search.max_results` is clamped to `1..=100`; `context_lines` is clamped to `0..=20`.
- `agentic_parse.default_strategy` accepts `auto`, `structured`, `narrative`, `tabular`, or `code`; invalid values fall back to `auto`.
- `agentic_parse.max_chars` is clamped to `500..=200000`.
- `document_parser.max_file_size_mb` is clamped to `1..=1024`; OCR `max_images` is clamped to `1..=64`; OCR `dpi` is clamped to `72..=600`.

## Agentic Document Flow

`agentic_search` and `agentic_parse` are built-in tools. They do not parse PDFs, Office files, images, or emails themselves. Both tools delegate file decoding to the shared document parser registry, which includes `CompositeDocumentParser` by default when `document_parser.enabled=true`.

```text
User / LLM
    |
    v
agentic_search / agentic_parse
    |
    v
ToolContext.document_parsers
    |
    v
DocumentParserRegistry
    |
    +--> PlainTextParser
    |       |
    |       `--> text/code/config files
    |
    `--> CompositeDocumentParser
            |
            `--> pdf / office / odf / image / epub / html / xml / eml / rtf
                    |
                    v
              ParsedDocument
              - title
              - blocks[]
              - block.kind / label / content / location
              - optional OCR runtime metadata
                    |
        +-----------+-----------+
        |                       |
        v                       v
agentic_search            agentic_parse
- builds search lines     - builds structural summary
- matches/ranks files     - detects parse strategy
- uses block metadata     - sends block-aware context to LLM
```

Responsibility split:

- `CompositeDocumentParser`: decode project documents into a shared structured model for agent context.
- `CompositeDocumentParser` covers a broad set of document families, including PDF, Office, ODF, iWork, HWP/HWPX, archive containers, HTML/XML, email/mailbox formats, notebooks, citation formats, calendar/contact formats, and common image formats.
- Supported examples include: `pdf`, `doc`, `dot`, `docx`, `docm`, `xls`, `xlt`, `xlsx`, `xlsm`, `xlsb`, `ppt`, `pps`, `pptx`, `pptm`, `odt`, `ods`, `odp`, `hwp`, `hwpx`, `pages`, `numbers`, `key`, `epub`, `zip`, `tar`, `gz`, `tgz`, `7z`, `rtf`, `html`, `xml`, `eml`, `emlx`, `mbox`, `msg`, `ipynb`, `ris`, `bib`, `csl`, `ics`, `vcf`, `png`, `jpg`, `jpeg`, `webp`, `gif`, `bmp`, `tif`, `tiff`.
- `agentic_search`: search and rank over the structured document content.
- `agentic_parse`: summarize, analyze, and answer questions over the structured document content.
- `agentic_parse`: when OCR is used, it also surfaces OCR runtime metadata such as format, provider, model, `max_images`, and `dpi`.

SDK access to parser runtime metadata:

```python
tool = session.tool("agentic_parse", {"path": "docs/scanned.pdf"})
print(tool.metadata)
print(tool.document_runtime)
runtime = tool.document_runtime_info
if runtime and runtime.ocr:
    print(runtime.ocr.provider, runtime.ocr.model, runtime.ocr.dpi)
print(tool.metadata_json)
print(tool.document_runtime_json)
```

```typescript
import { parseDocumentRuntime } from '@a3s-lab/code';

const tool = await session.tool('agentic_parse', { path: 'docs/scanned.pdf' });
console.log(tool.metadata);
console.log(tool.documentRuntime);
const runtime = parseDocumentRuntime(tool);
console.log(runtime?.ocr?.provider, runtime?.ocr?.model, runtime?.ocr?.dpi);
console.log(tool.metadataJson);
console.log(tool.documentRuntimeJson);
```

`metadata` / `metadataJson` exposes the full tool metadata as parsed object plus raw JSON. Python additionally exposes `document_runtime_info` as a typed runtime object. In Node, `documentRuntime` and `parseDocumentRuntime(...)` expose the structured `DocumentRuntimeMetadata` view, while `document_runtime_json` / `documentRuntimeJson` keep the raw JSON string form.

`agentic_search` also exposes structured match metadata, including page / section
locators derived from `CompositeDocumentParser` blocks:

```python
search = session.tool("agentic_search", {"query": "overview", "mode": "fast"})
for result in search.agentic_search_results_info:
    for match in result.matches:
        print(match.line_number, match.locator, match.content)
```

```typescript
import { parseAgenticSearchResults } from '@a3s-lab/code';

const search = await session.tool('agentic_search', { query: 'overview', mode: 'fast' });
for (const result of search.agenticSearchResults ?? parseAgenticSearchResults(search) ?? []) {
  for (const match of result.matches ?? []) {
    console.log(match.lineNumber, match.locator, match.content);
  }
}
```

When `agentic_parse` runs with a `query`, SDK helpers also expose the exact
structured blocks selected for LLM input:

```python
tool = session.tool(
    "agentic_parse",
    {"path": "docs/scanned.pdf", "query": "overview"},
)
for block in tool.agentic_parse_llm_blocks_info:
    location = block.location.display if block.location else None
    print(block.index, block.kind, block.label, location)
```

```typescript
import { parseAgenticParseLlmBlocks } from '@a3s-lab/code';

const tool = await session.tool('agentic_parse', {
  path: 'docs/scanned.pdf',
  query: 'overview',
});
for (const block of tool.agenticParseLlmBlocks ?? parseAgenticParseLlmBlocks(tool) ?? []) {
  console.log(block.index, block.kind, block.label, block.location?.display);
}
```

## Document Intelligence Roadmap

Goal: make `agentic_search` and `agentic_parse` capable of searching and understanding a broad range of project and business document formats through the default document parser stack, while steadily closing the gap with higher-fidelity systems such as `kreuzberg`.

Current state:

- [x] Parse plain text, Markdown, code, CSV/JSON/YAML/TOML, and part of the PDF / Office surface.
- [x] Expand format detection and container handling for more archive, iWork, `xlsb`, and `hwp/hwpx` scenarios.
- [x] Add default pipeline stages for language enrichment, keyword extraction, hierarchical chunking, and quality evaluation.
- [x] Expose structural summaries, quality metadata, language, keywords, chunk highlights, provenance, and confidence in `agentic_parse`.

Phase 1: Structured result surfaces

- [x] Expose `structured_payload` directly in `agentic_parse` output and metadata.
- [x] Expose table payloads in a stable machine-readable form.
- [x] Expose page-level data in `agentic_parse` output and metadata.
- [x] Add stable `tables[]` output instead of relying on text summaries alone.
- [x] Add stable `pages[]` output instead of relying on text summaries alone.
- [ ] Add stable `elements[]` output instead of relying on text summaries alone.
- [ ] Teach `agentic_search` to consume chunk context more directly.
- [ ] Teach `agentic_search` to consume tabular content more directly.
- [ ] Teach `agentic_search` to consume page numbers and locators more directly.
- [ ] Exit criteria: complex PDF and Office documents return stable locators in search results.
- [ ] Exit criteria: parse results are directly consumable by downstream agents.

Phase 2: High-value parser depth

- [x] Improve native PDF extraction quality (lopdf position-aware extraction).
- [x] Reduce dependence on weak text fallbacks for PDF.
- [ ] Reduce dependence on OCR-only recovery for PDF.
- [ ] Deepen true BIFF12 `xlsb` extraction.
- [ ] Improve workbook structure recovery for `xlsb`.
- [ ] Improve multi-sheet recovery for `xlsb`.
- [ ] Improve table fidelity for `xlsb`.
- [ ] Improve iWork extraction depth.
- [ ] Improve HWP/HWPX extraction depth.
- [ ] Improve archive-embedded document extraction depth.
- [ ] Exit criteria: tables, sections, and page-level content are recovered reliably across common business documents.
- [ ] Exit criteria: search recall and parse usefulness improve materially on common business documents.

Phase 3: Unified document object model

- [ ] Build a consistent document object model on top of the default parser stack.
- [ ] Cover blocks, pages, tables, images, metadata, and quality in that model.
- [ ] Standardize provenance output across parsers.
- [ ] Standardize confidence output across parsers.
- [ ] Standardize runtime metadata output across parsers.
- [ ] Standardize validation issue output across parsers.
- [ ] Reduce downstream special-casing by enforcing consistent fields across formats.
- [ ] Exit criteria: `agentic_parse` returns a structurally similar object across major formats.
- [ ] Exit criteria: downstream agents can consume parse results without format-specific branching.

Phase 4: Search-understanding convergence

- [ ] Let `agentic_search` rank with direct access to `tables[]`.
- [ ] Let `agentic_search` rank with direct access to `pages[]`.
- [ ] Let `agentic_search` rank with direct access to `elements[]`.
- [ ] Strengthen query-aware chunk selection.
- [ ] Strengthen heading inheritance during ranking.
- [ ] Strengthen table-first matching when query intent is tabular.
- [ ] Strengthen in-page region localization.
- [ ] Add stronger de-duplication for cross-document retrieval.
- [ ] Add quality-aware ranking for cross-document retrieval.
- [ ] Add OCR-aware ranking for cross-document retrieval.
- [ ] Exit criteria: retrieval behaves more like “find the right evidence first, then answer correctly”.

Phase 5: Multimodal and long-tail format coverage

- [ ] Improve OCR handling for scanned documents.
- [ ] Improve vision handling for chart-heavy files.
- [ ] Improve handling for image-based PDFs.
- [ ] Close remaining long-tail gaps versus `kreuzberg`.
- [ ] Add support for `dbf`.
- [ ] Add support for `djot`.
- [ ] Add support for `man`, `troff`, and `pod`.
- [ ] Expand broader image and archive support.
- [ ] Evaluate whether to integrate higher-fidelity external parsers.
- [ ] Evaluate whether to maintain an internal compatibility layer instead.
- [ ] Exit criteria: the failure rate for arbitrary files keeps dropping.
- [ ] Exit criteria: binary-heavy document sets become meaningfully searchable.

Priority order:

- [ ] First, improve result schemas and downstream consumability.
- [ ] Second, improve PDF, `xlsb`, and Office extraction depth.
- [ ] Third, expand multimodal and long-tail format support.

Definition of done:

- [ ] `agentic_search` consistently returns high-quality, well-located evidence for common document formats.
- [ ] `agentic_parse` returns structured results for common document formats, not just text summaries.
- [ ] The default parser output flows through a unified pipeline with quality, language, keywords, provenance, and confidence.
- [ ] Real-world usability on common document corpora approaches `kreuzberg`-class behavior while staying well integrated with the A3S toolchain.

---

## Slash Commands

Sessions intercept slash commands before the LLM. Type `/help` in any session:

| Command | Description |
|---------|-------------|
| `/help` | List available commands |
| `/model [provider/model]` | Show or switch the current model |
| `/cost` | Show token usage and estimated cost |
| `/clear` | Clear conversation history |
| `/compact` | Manually trigger context compaction |
| `/tools` | List registered tools |
| `/loop [interval] <prompt>` | Schedule a recurring prompt (default: 10m) |
| `/cron-list` | List scheduled tasks |
| `/cron-cancel <id>` | Cancel a scheduled task |

Register custom commands:

```python
session.register_command("status", "Show status", lambda args, ctx: f"Model: {ctx['model']}")
result = session.send("/status")
```

---

## BTW — Ephemeral Side Questions

Ask a side question without it affecting conversation history:

```python
btw = session.btw("What's the default port for PostgreSQL?")
print(btw.answer)        # "5432"
print(btw.total_tokens)  # token usage for this query only
# main conversation continues — btw question not in history
```

---

## Scheduled Tasks

Schedule recurring prompts via `/loop` or the programmatic API:

```python
# Via slash command
session.send('/loop 5m check if tests are still passing')

# Programmatic
task_id = session.schedule_task('summarize git log since last check', 300)

# List and cancel
tasks = session.list_scheduled_tasks()
session.cancel_scheduled_task(task_id)
```

Interval syntax: `30s`, `5m`, `2h`, `1d`. Max 50 tasks per session; auto-expire after 3 days.

---

## Safety and Control

Agents run with **explicit permissions**. Nothing executes by default without a policy allowing it:

```python
from a3s_code import SessionOptions, PermissionPolicy, PermissionRule

opts = SessionOptions()
opts.permission_policy = PermissionPolicy(
    allow=[PermissionRule("read(*)"), PermissionRule("grep(*)")],
    deny=[PermissionRule("bash(*)")],
    default_decision="deny",
)
session = agent.session(".", opts)
```

Other safety features:

- **Human-in-the-loop confirmation** — prompt before any tool call
- **Skill-based tool restrictions** — `allowed-tools` in skill frontmatter limits what the LLM can call
- **AHP integration** — plug in an external harness to block or sanitize tool calls at runtime
- **Auto-compact** — rolls up context before hitting token limits, keeping sessions running
- **Circuit breaker** — stops after 3 consecutive LLM failures, prevents infinite retry loops
- **Continuation injection** — prevents the LLM from stopping early mid-task (max 3 continuation turns)

---

## Hooks — Lifecycle Events

Intercept and modify agent behavior at 11 event points:

```python
from a3s_code import SessionOptions, HookHandler

class MyHook(HookHandler):
    def pre_tool_use(self, tool_name, tool_input, ctx):
        if tool_name == "bash" and "rm -rf" in str(tool_input):
            return self.block("Refusing destructive command")
        return self.continue_()

opts = SessionOptions()
opts.hook_handler = MyHook()
session = agent.session(".", opts)
```

Hook events: `PreToolUse` (blockable), `PostToolUse`, `GenerateStart` (modifiable), `GenerateEnd`, `SessionStart/End`, `SkillLoad/Unload`, `PrePrompt`, `PostResponse`, `OnError`.

---

## Persistence and Memory

Sessions can be saved and resumed. Memory persists across sessions:

```python
from a3s_code import SessionOptions, FileSessionStore, FileMemoryStore

opts = SessionOptions()
opts.session_store = FileSessionStore('./sessions')
opts.memory_store = FileMemoryStore('./memory')
opts.session_id = 'my-session'
opts.auto_save = True

session = agent.session(".", opts)
resumed = agent.resume_session('my-session', opts)
```

---

## Multi-Provider

One config, any LLM:

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers { name = "anthropic";  api_key = env("ANTHROPIC_API_KEY") }
providers { name = "openai";     api_key = env("OPENAI_API_KEY") }
providers { name = "deepseek";   api_key = env("DEEPSEEK_API_KEY") }
providers { name = "kimi";       api_key = env("MOONSHOT_API_KEY") }
providers { name = "together";   api_key = env("TOGETHER_API_KEY") }
providers { name = "groq";       api_key = env("GROQ_API_KEY") }
```

Switch model per session:

```python
session = agent.session(".", model="openai/gpt-4o")
```

---

## Skills

Skills are markdown files that shape LLM behavior — injected into the system prompt automatically:

```markdown
---
name: safe-reviewer
description: Review code without modifying files
allowed-tools: "read(*), grep(*), glob(*)"
---

Review the code in the workspace. You may read and search files,
but you must not write, edit, or execute anything.
```

```python
opts = SessionOptions()
opts.skill_dirs = ["./skills"]
session = agent.session(".", opts)
```

Built-in skills (enabled via `builtin_skills=True`): `agentic-search`, `code-search`, `code-review`, `explain-code`, `find-bugs`, `builtin-tools`, `delegate-task`, `find-skills`.

---

## Multi-Agent

Delegate tasks to subagents or coordinate teams:

```python
# Single subagent
result = session.send('task: explore the codebase and summarize the architecture')

# Parallel tasks
result = session.send('parallel_task: [audit security, check performance, review tests]')

# Agent team (lead decomposes → workers execute → reviewer validates)
result = session.send('run_team: refactor the authentication module')
```

Built-in agent types: `explore` (read-only), `general` (full capabilities), `plan` (analysis only).

---

## Architecture

```
Agent (config + provider registry)
  └── Session (workspace + tools + LLM)
        └── AgentLoop (turn-based execution)
              ├── LlmClient      → sends messages, receives tool calls
              ├── ToolExecutor   → runs tools, enforces permissions
              ├── SkillRegistry  → injects skills into system prompt
              └── PluginManager  → loads optional extension plugins (for example skill bundles)
```

20 trait-based extension points: swap any policy, provider, store, or hook without touching core.

---

## Documentation

Full reference, examples, and guides: **[a3s.dev/docs/code](https://a3s.dev/docs/code)**

- [Sessions & Options](https://a3s.dev/docs/code/sessions)
- [Commands & Scheduling](https://a3s.dev/docs/code/commands) — `/btw`, `/loop`, slash commands
- [Tools](https://a3s.dev/docs/code/tools)
- [Skills](https://a3s.dev/docs/code/skills)
- [Plugin System](https://a3s.dev/docs/code/plugins)
- [Hooks](https://a3s.dev/docs/code/hooks)
- [Security](https://a3s.dev/docs/code/security)
- [Examples](https://a3s.dev/docs/code/examples)

---

## License

MIT
