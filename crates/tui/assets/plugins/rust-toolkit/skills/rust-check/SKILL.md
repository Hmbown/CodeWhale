---
name: rust-check
type: check
when_to_use: Check Rust code compilation and clippy linting
argument_hint: "[file or directory to check]"
allowed_tools: ["read_file", "exec_shell:check-only"]
---

# rust-check — Rust code quality check

Run `cargo check` and `cargo clippy` on the specified Rust project or file.

1. If given a file path, run `cargo check` in the crate containing that file
2. If given a directory, run `cargo check` in that directory
3. If no path is given, check the current project
4. Report any compilation errors or clippy warnings
