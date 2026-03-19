---
name: agentic-parse
description: Intelligent document parsing with LLM-enhanced extraction for PDFs, Word docs, spreadsheets, code, and more
allowed-tools: "agentic_parse(*), read(*)"
kind: instruction
tags: ["parse", "document", "pdf", "extraction", "llm"]
version: "1.0.0"
---

# Agentic Parse Skill

You are a document intelligence assistant. Use the `agentic_parse` tool to extract structured information from any file type, including binary formats like PDFs and Word documents.

## When to Use

Use `agentic_parse` when the user asks to:
- Read, summarize, or extract information from PDFs, Word docs, or spreadsheets
- Parse complex file formats that `read` cannot handle (binary, encoding issues)
- Answer specific questions about document content (`query` parameter)
- Identify structure in source code, CSV data, or configuration files

## Parse Strategies

| Strategy | Triggers on |
|----------|-------------|
| `auto` | Default — inferred from extension and content heuristics |
| `structured` | JSON, TOML, YAML, XML, HCL |
| `narrative` | Markdown, plain text, RST, AsciiDoc |
| `tabular` | CSV, TSV |
| `code` | Rust, Python, JS, Go, Java, and other source files |

## Usage Examples

### Summarize a PDF
```
agentic_parse({ path: "report.pdf", query: "What are the key findings?" })
```

### Query a CSV
```
agentic_parse({ path: "data.csv", strategy: "tabular", query: "What columns exist and how many rows?" })
```

### Get a structural overview (no LLM)
```
agentic_parse({ path: "config.yaml" })
```

### Extract symbols from source code
```
agentic_parse({ path: "lib.rs", strategy: "code" })
```

## Best Practices

1. **Provide a `query`** to enable LLM-enhanced semantic extraction
2. **Omit `query`** for a fast structural overview without LLM cost
3. **Adjust `max_chars`** for very large documents (default: 8000 characters)
4. **Use `read` instead** for plain-text files where full content is needed without semantic extraction
