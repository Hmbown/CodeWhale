# Sandbox threat model

Codewhale executes shell commands spawned by AI reasoning. The sandbox
module restricts what those commands can do to the host system. This
document describes what each platform's sandbox actually enforces,
what is best-effort, and what is explicitly out of scope.

## Platform overview

| Mechanism | Platform | Type | Status |
|---|---|---|---|
| Seatbelt | macOS | Mandatory access control | Enforced |
| Landlock | Linux | Filesystem access control | Enforced |
| seccomp BPF | Linux | Syscall filter | Enforced |
| Process hardening | Linux | Kernel prctl / rlimit | Enforced |
| Bubblewrap (bwrap) | Linux | Namespace isolation | Optional |
| Windows Job Object | Windows | Process-tree containment | v1 (PR #2220) |

## Threat model: what each layer addresses

### 1. Process hardening (Linux only)

**When it runs:** Before any threads are spawned, before Tokio boots,
before any data is loaded into memory.

**What it does:**

- `PR_SET_DUMPABLE=0` — prevents ptrace, makes `/proc/<pid>/` root-owned
- `PR_SET_NO_NEW_PRIVS=1` — irreversible; no child can ever gain privileges
- `RLIMIT_CORE=0` — no core dumps, so sensitive data never hits disk

**What it protects against:**
- Process inspection via ptrace/strace/gdb
- Privilege escalation via setuid/setgid/fscaps
- Core dumps leaking API keys, tokens, prompt content

**What it does NOT protect against:**
- A compromised child reading its parent's `/proc/<pid>/mem` (already blocked
  by `PR_SET_DUMPABLE=0` making `/proc/<pid>/` root-owned)
- Kernel exploits that bypass prctl

### 2. Landlock (Linux, kernel 5.13+)

**When it runs:** Applied to each child process at spawn time via a
helper script or `landlock_restrict_self`. Only restrictable by the
process itself — parent cannot force Landlock on a child.

**What it does:**
- Restricts filesystem access to a whitelist of paths
- Handles: `EXECUTE`, `READ_FILE`, `READ_DIR`, `WRITE_FILE`, `REMOVE_DIR`,
  `REMOVE_FILE`, `MAKE_DIR`, `MAKE_REG`, `MAKE_SYM`, `TRUNCATE`

**What it protects against:**
- Reading files outside the workspace (e.g., `/etc/passwd`, `~/.ssh`)
- Writing to system directories (`/usr`, `/bin`, `/lib`)
- Creating or deleting files in protected locations

**What it does NOT protect against:**
- Network access (Landlock is filesystem-only)
- Process inspection (use seccomp for this)
- Reading files that are already mapped (Landlock applies at `open()` time)

**Detection:** `detect_denial()` checks stderr for `Permission denied`,
`Operation not permitted`, `EACCES`, `EPERM`.

### 3. seccomp BPF (Linux only)

**When it runs:** Installed via `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)`
on the child process.

**What it does:**
- Whitelist of ~100 safe syscalls (file I/O, memory, process, IPC,
  synchronization, signals, time)
- **Explicitly denied:** `ptrace`, `mount`, `umount2`, `kexec_load`,
  `kexec_file_load`, `init_module`, `finit_module`, `delete_module`,
  `bpf`, `reboot`, `swapon`, `swapoff`, `pivot_root`,
  `setuid`/`setgid`/`setreuid`/`setregid`/`setresuid`/`setresgid`,
  `personality`
- Any syscall not on the whitelist → `SECCOMP_RET_KILL_PROCESS` (SIGSYS)

**What it protects against:**
- Process hijacking via ptrace
- Mounting filesystems (bypassing Landlock read-only restrictions)
- Loading kernel modules
- Loading BPF programs (would bypass seccomp itself!)
- Rebooting the system
- Privilege changes via setuid/setgid

**What it does NOT protect against:**
- Legitimate use of allowed syscalls for malicious purposes
- Side-channel attacks via allowed syscalls (e.g., timing)

**Detection:** `detect_denial()` checks exit code 31 (SIGSYS) or stderr
for `Bad system call`, `bad system call`, `SIGSYS`, `seccomp`.

### 4. Bubblewrap / bwrap (Linux, optional)

**When it runs:** If `/usr/bin/bwrap` is present AND the config key
`[sandbox] prefer_bwrap = true` is set. Runs as an outer wrapper around
the child command.

**What it does:**
- Creates a new mount namespace with `--unshare-all`
- Read-only bind-mounts the entire root filesystem
- Bind-mounts the workspace directory with read-write access
- Changes into the workspace with `--chdir`

**What it protects against:**
- Any filesystem write outside the workspace (stronger than Landlock alone
  because it's enforced at the namespace level, not just filesystem access)
- Accidental modification of system files

**What it does NOT protect against:**
- Network access (bwrap does not create a network namespace by default with
  `--unshare-all`; the child still has full network access)
- Process inspection
- Memory attacks

**Installation:** User must install bubblewrap themselves:
- Ubuntu/Debian: `apt install bubblewrap`
- Fedora: `dnf install bubblewrap`
- Arch: `pacman -S bubblewrap`

Codewhale does NOT vendor bwrap.

**Fallback:** If bwrap is not installed, the sandbox falls back to Landlock
only.

### 5. Seatbelt (macOS)

**When it runs:** Applied via the `sandbox-exec` wrapper command. The
seatbelt profile is generated dynamically based on the `SandboxPolicy`.

**What it does:**
- Restricts filesystem access based on the policy profile
- Can restrict network access (when `network_access: false`)

**What it protects against:**
- Reading/writing files outside allowed paths
- Network connections (when configured)

**What it does NOT protect against:**
- Process inspection (Seatbelt does not block ptrace)
- Syscall-level attacks

**Detection:** Checks stderr for `file-write` and `network` denial patterns.

### 6. Windows Job Object (v1, PR #2220)

**When it runs:** Applied at process spawn time via
`PROC_THREAD_ATTRIBUTE_JOB_LIST` and restricted token assignment.

**What it does (v1):**
- Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — all child
  processes terminate when the parent exits
- Memory cap: 1 GB per process, 2 GB per job
- Active process limit: 64
- UI restrictions: no desktop handle access
- Restricted token: drops Administrators group SID, sets medium-low
  integrity level

**What is deferred (v2):**
- WFP (Windows Filtering Platform) firewall rules — network is open in v1
- Filesystem ACL integration at spawn time (deferred)
- AppContainer isolation
- Registry key isolation

**Detection:** Checks stderr for `Access is denied`, `STATUS_ACCESS_DENIED`,
`ERROR_ACCESS_DENIED`, `ERROR_PRIVILEGE_NOT_HELD`,
`ERROR_ACCESS_DISABLED_BY_POLICY`, and integrity/AppContainer patterns.

## Defense in depth

The Linux sandbox applies layers in order:

```
Process hardening (prctl)    ← before threads
    ↓
Landlock (filesystem)        ← at child spawn
    ↓
seccomp BPF (syscalls)       ← at child spawn
    ↓
bwrap (namespace isolation)  ← optional outer wrapper
```

Each layer addresses a different threat surface. seccomp cannot protect the
filesystem (that's Landlock's job). Landlock cannot stop ptrace (that's
seccomp + PR_SET_DUMPABLE). bwrap adds namespace-level isolation that
neither Landlock nor seccomp can provide.

## Configuration

Relevant config keys in `~/.codewhale/config.toml`:

```toml
# Sandbox policy mode
sandbox_mode = "workspace-write"  # read-only | workspace-write | danger-full-access | external-sandbox

# Linux bubblewrap passthrough
prefer_bwrap = false              # requires `bubblewrap` package installed

# External sandbox backend
sandbox_backend = "none"          # "none" or "opensandbox"
sandbox_url = "http://localhost:8080"
sandbox_api_key = "YOUR_API_KEY"
```

Environment variable overrides:

- `DEEPSEEK_SANDBOX_MODE` → `sandbox_mode`
- `DEEPSEEK_PREFER_BWRAP=true` → `prefer_bwrap`
- `DEEPSEEK_SANDBOX_BACKEND` → `sandbox_backend`
- `DEEPSEEK_SANDBOX_URL` → `sandbox_url`
- `DEEPSEEK_SANDBOX_API_KEY` → `sandbox_api_key`

## Detecting sandbox denials

When a command fails, the sandbox manager checks for denial patterns:

| Platform | Denial mechanism | Exit code | Stderr patterns |
|---|---|---|---|
| macOS Seatbelt | sandbox-exec violation | non-zero | `file-write`, `network` |
| Linux Landlock | EACCES / EPERM | non-zero | `Permission denied`, `Operation not permitted` |
| Linux seccomp | SIGSYS (31) | 31 or 159 | `Bad system call`, `SIGSYS` |
| Linux bwrap | Mount/namespace failure | non-zero | varies |
| Windows | Access denied / privilege | non-zero | `Access is denied`, `ERROR_PRIVILEGE_NOT_HELD` |

The `was_denied()` method on `SandboxManager` aggregates all platform-specific
checks. The `denial_message()` method returns a human-readable explanation.

## Limitations

### What the sandbox does NOT protect against

- **Network attacks** — only macOS Seatbelt can block network; Linux and
  Windows v1 leave network open
- **Memory attacks** — no platform prevents a child process from reading
  its own memory or exploiting memory corruption bugs
- **Timing side channels** — allowed syscalls on Linux can be used for
  timing-based information leaks
- **Resource exhaustion** — the Linux job object limits memory and process
  count, but does not limit CPU, file descriptors, or disk I/O
- **Kernel vulnerabilities** — if the kernel itself has a vulnerability,
  the sandbox cannot prevent exploitation (this applies to all platforms)
- **Supply chain** — if the child process downloads and executes untrusted
  code, the sandbox limits what that code can do, but does not prevent the
  download

### Platform-specific gaps

- **Linux:** Landlock only protects filesystem access. seccomp adds syscall
  filtering but uses a whitelist that may need updates for new syscalls.
- **macOS:** Seatbelt profiles are generated at runtime. A misconfigured
  profile could be too permissive.
- **Windows v1:** No filesystem ACL enforcement at spawn time. Network is
  fully open. Job Object is process-tree only.

## Related

- `crates/tui/src/sandbox/` — implementation
- `crates/config/src/lib.rs` — config keys
- `crates/tui/src/tools/diagnostics.rs` — `diagnostics` tool reports
  `sandbox_available`, `sandbox_type`, `bwrap_available`, `cgroup_version`
- `config.example.toml` — annotated config reference
- Issue #2180 — this document
- Issue #2182 — seccomp filter implementation
- Issue #2183 — process hardening
- Issue #2184 — bwrap passthrough
- Issue #2185 — Windows Job Object v1
- Issue #2186 — SandboxExecutor trait unification
- Issue #2187 — sandbox parity tests
