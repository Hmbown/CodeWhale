---
name: presentations
description: Create, edit, inspect, or convert PowerPoint decks and PPTX slide presentations with practical layout and verification steps.
---

# Presentations

Use this skill when the user asks for slides, a deck, a PowerPoint file, or a
`.pptx` deliverable.

## Workflow

1. Determine the deck purpose, audience, slide count, and output path.
2. Build an outline with one clear claim or job per slide.
3. Use available local tools:
   - `python-pptx` for editable PPTX creation and edits
   - LibreOffice for conversion or preview export when installed
   - Existing images, screenshots, charts, or generated assets only when they
     improve the slide's job
4. Keep slides editable. Prefer native text boxes, tables, charts, and images
   over flattened screenshots of text.
5. Use restrained layouts: clear title, concise body, enough whitespace, and
   consistent type sizes. Avoid cramming paragraphs onto slides.
6. Verify the PPTX by reopening it, checking slide count and visible text, and
   exporting previews when a renderer is available.

If `python-pptx` or a renderer is missing, ask before installing dependencies.
Do not claim visual QA passed unless previews were actually generated and
inspected.
