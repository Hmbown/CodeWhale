---
name: spreadsheets
description: Create, edit, analyze, clean, or convert spreadsheets including XLSX, CSV, TSV, formulas, charts, and tabular reports.
---

# Spreadsheets

Use this skill when the input or deliverable is a spreadsheet: `.xlsx`, `.xlsm`,
`.csv`, `.tsv`, or a table that should become one of those formats.

## Workflow

1. Identify whether the task is cleaning, analysis, formatting, charting, or
   workbook generation.
2. Preserve source files. Write a new output workbook unless the user asked for
   an in-place edit.
3. Use appropriate tools:
   - Python `csv` or `pandas` for data cleaning and analysis
   - `openpyxl` for XLSX formulas, styles, tables, filters, freeze panes, and
     charts
   - LibreOffice only when conversion or recalculation is needed and available
4. Keep raw data separate from summary sheets when that helps auditability.
5. Use formulas when the workbook should remain interactive; use fixed values
   when the result must be stable and reproducible.
6. Verify by loading the workbook, checking sheet names, row/column counts,
   formulas, and representative cell values. For CSV/TSV, check delimiters,
   quoting, headers, and row widths.

If dependencies are missing, ask before installing packages. Never silently
drop rows, change date/time semantics, or coerce IDs with leading zeroes into
numbers.
