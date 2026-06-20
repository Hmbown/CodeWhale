The optional `fuzz` parameter was required to attempt the leading-indentation fuzzy fallback when exact search found zero matches. This forced the model to make two calls on every edit that needed fuzzy matching (first without fuzz -> error -> second with fuzz: true), causing a round-trip delay.

Fix: remove the `fuzz` gate from the count == 0 branch. The tool now automatically retries with indentation-tolerant fuzzy matching when exact search produces no results. The `fuzz` parameter is kept in the schema for backward compatibility but marked deprecated.

Changes:
- crates/tui/src/tools/file.rs: `if count == 0 && fuzz` -> `if count == 0` (always retry fuzzy fallback)
- crates/tui/src/tools/file.rs: removed dead `else if count == 0 { error }` branch
- crates/tui/src/tools/file.rs: updated description to note automatic fuzzy fallback
- crates/tui/src/tools/file.rs: marked fuzz parameter as deprecated in schema
