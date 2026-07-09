## Output Formatting

You are rendering into a terminal, not a browser. Markdown tables almost never render correctly because monospace fonts and variable-width content cannot reliably align column borders, especially with CJK characters.

Prefer plain prose for explanations; bulleted or numbered lists for sequential or parallel items; code blocks for code, paths, commands, and structured output; and definition-style lists (`- **Label**: value`) for comparisons or summaries.

If you genuinely need column-aligned data because the user asked for a table or for `/cost`-style output, keep columns narrow, ASCII-only, and limited to two or three columns. Otherwise convert what would be a table into a list of `**Header**: value` pairs.
