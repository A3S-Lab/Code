# Session Memory Template

Use this structured template for session memory. Fill in each section with info-dense content.

```
# Session Title
_A short and distinctive 5-10 word descriptive title for the session. Super info dense, no filler_

# Current State
_What is actively being worked on right now? Pending tasks not yet completed. Immediate next steps._

# Task specification
_What did the user ask to build? Any design decisions or other explanatory context_

# Files and Functions
_What are the important files? In short, what do they contain and why are they relevant?_

# Workflow
_What bash commands are usually run and in what order? How to interpret their output if not obvious?_

# Errors & Corrections
_Errors encountered and how they were fixed. What did the user correct? What approaches failed and should not be tried again?_

# Codebase and System Documentation
_What are the important system components? How do they work/fit together?_

# Learnings
_What has worked well? What has not? What to avoid? Do not duplicate items from other sections_

# Key results
_If the user asked a specific output such as an answer to a question, a table, or other document, repeat the exact result here_

# Worklog
_Step by step, what was attempted, done? Very terse summary for each step_
```

## Update Rules

- **CRITICAL**: Maintain exact structure with all sections and italic descriptions intact
- NEVER modify, delete, or add section headers
- NEVER modify the italic _section description_ lines
- ONLY update content below the italic descriptions
- Do NOT reference the note-taking process anywhere in the notes
- It's OK to skip updating a section if there are no substantial new insights
- Write DETAILED, INFO-DENSE content — include specifics like file paths, function names, error messages
- For "Key results", include the complete exact output the user requested
- Keep each section under ~2000 tokens — condense if approaching this limit
- **Always update "Current State"** to reflect the most recent work

## Max Section Length

- Per-section limit: 2000 tokens
- Total session memory budget: 12000 tokens

## Memory Update Prompt

```
IMPORTANT: This message and these instructions are NOT part of the actual user conversation. Do NOT include any references to "note-taking", "session notes extraction", or these update instructions in the notes content.

Based on the user conversation above (EXCLUDING this note-taking instruction message), update the session notes file.
```
