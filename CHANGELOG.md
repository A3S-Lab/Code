# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - 2026-04-02

---

## [v1.6.0] - 2026-04-02

### Added

#### Document Parsing

- **XLSB (Excel Binary) Support**: Added calamine-based BIFF12 parsing for XLSB files with proper cell value extraction, supporting Float, Int, Bool, DateTime, DateTimeIso, and DurationIso types. Significantly improves table fidelity for .xlsb files.

- **HWPX Table Extraction**: Added structured table extraction from Korean HWPX documents. Parses `tbl/tr/tc` XML hierarchy and includes `structured_payload` for `tables[]` output.

- **Vision OCR Provider**: New OCR backend supporting OpenAI-compatible vision APIs for document OCR fallback.

  ```hcl
  document_parser {
    ocr {
      enabled  = true
      model    = "openai/gpt-4.1-mini"
      api_key  = "sk-..."
      base_url = "https://api.openai.com/v1"  # optional
      prompt   = "Extract all text from this document..."
      max_images = 8
      dpi     = 144
    }
  }
  ```

  Provider priority: External provider > Vision API (if model+api_key configured) > Builtin tesseract

#### Search Ranking

- **Tabular Query Intent Detection**: Automatically detects when queries relate to tables (keywords: table, column, row, spreadsheet, excel, csv, cell, data, record, etc.) and boosts table line matches by +10 keyword hits plus 1.3x relevance multiplier.

- **Heading Inheritance Boost**: When search matches appear under headings that also match the query, those matches receive a relevance boost (up to 1.3x). Looks backwards to find the closest preceding heading.

### Changed

#### Configuration

- `DocumentOcrConfig` extended with new fields:
  - `provider: Option<String>` - Backend selection ("vision" or "builtin")
  - `base_url: Option<String>` - Custom API endpoint
  - `api_key: Option<String>` - API authentication

#### Dependencies

- Added `calamine = "0.26"` for XLSB parsing
- Added `reqwest/blocking` feature for Vision API HTTP calls

### Fixed

- Test assertion: `paged_text_blocks_reflow_two_column_preserves_paragraph_breaks` - Corrected expected string "Parser metadata now tracks OCR" vs "Parser metadata now tracks OCR backend"

---

## [v1.5.8] - 2026-03-07

### Added

- Phase 1 structured result surfaces:
  - `structured_payload` exposed in `agentic_parse` output and metadata
  - Table payloads in stable machine-readable form
  - Page-level data in `agentic_parse` output and metadata
  - Stable `tables[]`, `pages[]`, `elements[]` outputs

- Phase 2 PDF extraction improvements:
  - lopdf position-aware text extraction
  - Reduced dependence on weak text fallbacks
  - Position-aware table detection

- `agentic_search` enhancements:
  - Chunk context consumption
  - Tabular content consumption
  - Page numbers and locators support

### Changed

- `ParsedDocument` extended with `tables: Vec<StructuredTable>` and `pages: Vec<PageInfo>`

### Fixed

- Windows shell compatibility improvements

---

## [v1.5.7] - 2026-02-28

### Added

- Runtime session header support for OpenAI configs
- Cross-platform environment variable expansion in tests

---

## [v1.5.6] - 2026-02-20

### Added

- Enhanced agent config, document parser, LLM, tools, and SDKs
- Host shell environment propagation to tool commands

---

## [v1.5.5] - 2026-02-10

### Added

- Zhipu AI client (`ZhipuClient` formerly `GlmClient`)
- Duplicate tool call circuit breaker
- Streaming fallback support
- `agentic_parse` skill

---

## [v1.5.4] - 2026-01-28

### Added

- Session-local skill registries

---

## [v1.5.3] - 2026-01-15

### Added

- Tool schema hardening
- Slash command output restoration
