---
name: documents
description: Create, edit, inspect, or convert Word documents and DOCX deliverables such as memos, reports, letters, templates, and forms.
---

# Documents

Use this skill when the user wants a `.docx` or Word-style document, or when an
existing document needs edits, comments, extraction, or conversion.

## Workflow

1. Clarify the output path if needed. Prefer a `.docx` file in the workspace.
2. Preserve existing documents. Write edited copies instead of overwriting
   originals unless the user asked for an in-place edit.
3. Use the best available local tool:
   - `python-docx` for creation and structured edits
   - `pandoc` for Markdown/HTML to DOCX conversion
   - OOXML inspection with `unzip` only for targeted low-level fixes
4. For new documents, choose a simple professional structure: title, metadata
   when useful, clear headings, readable body text, and tables only for real
   row/column data.
5. For edits, make the smallest change that satisfies the request and keep the
   original formatting where practical.
6. Verify by reopening the file with a parser such as `python-docx`, checking
   paragraph/table counts, and extracting representative text. If LibreOffice
   or another renderer is available, render or export a quick visual check.

If required dependencies are missing, ask before installing packages. If the
user only needs content and a DOCX cannot be produced in the environment, offer
Markdown or HTML as a fallback and clearly state the limitation.
