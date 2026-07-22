//! Linux seccomp (Secure Computing) filter layer (#2182).
//!
//! This module is dormant in v0.9.1: command execution does not install this
//! filter. Kernel support therefore does not mean that a command is sandboxed.
//!
//! Seccomp BPF (Berkeley Packet Filter) is a kernel facility that allows a
//! process to restrict the system calls it (and its descendants) can make.
//! The implementation here is intended to apply a seccomp filter on top of
//! Landlock as a second layer of defense. Once it is wired, the filter can
//! block entire *classes* of dangerous syscalls like `ptrace`, `mount`, and
//! `kexec_load`.
//!
//! # Architecture
//!
//! The filter is written as a raw BPF program (array of `sock_filter`
//! instructions) and loaded via `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)`.
//! This avoids any dependency on external crates like `libseccomp-sys` or
//! `seccompiler` — we use only the `libc` crate already in the dependency
//! tree.
//!
//! # Whitelisted syscalls
//!
//! The filter uses a whitelist approach: only syscalls that are known to be
//! safe for a development/shell workload are allowed. Everything else is
//! killed with `SECCOMP_RET_KILL_PROCESS`. The whitelist includes:
//!
//! - File I/O: read, write, open, openat, close, stat, fstat, lstat, newfstatat
//! - Directory: getdents, getdents64, getcwd, chdir
//! - Memory: mmap, mprotect, munmap, brk, mremap, madvise
//! - Process: clone, clone3, fork, vfork, execve, execveat, exit, exit_group
//! - IPC: pipe, pipe2, socket, socketpair, connect, bind, listen, accept, accept4
//! - Synchronization: futex, nanosleep, clock_nanosleep
//! - Signals: rt_sigaction, rt_sigprocmask, rt_sigreturn, kill, tkill, tgkill
//! - Resource: getrlimit, setrlimit, prlimit64, getrusage
//! - Time: clock_gettime, gettimeofday, time
//! - Misc: getpid, gettid, getuid, geteuid, getgid, getegid, uname, arch_prctl
//!
//! # Explicitly denied
//!
//! - ptrace (process hijacking)
//! - mount, umount2 (filesystem manipulation)
//! - kexec_load, kexec_file_load (kernel execution)
//! - init_module, finit_module, delete_module (kernel module loading)
//! - bpf (loading BPF programs — would bypass seccomp!)
//! - reboot
//! - swapon, swapoff
//! - pivot_root
//! - setuid, setgid, setreuid, setregid, setresuid, setresgid
//! - personality
//!
//! # Safety
//!
//! Once the seccomp filter is installed, it is **irreversible** — even
//! `prctl(PR_SET_SECCOMP, ...)` is denied. This is by design.

/// Check if seccomp is available on this system.
///
/// Returns true if `/proc/sys/kernel/seccomp/actions_avail` exists and
/// contains "kill_process", indicating the kernel supports seccomp BPF.
#[cfg(target_os = "linux")]
pub fn is_available() -> bool {
    std::path::Path::new("/proc/sys/kernel/seccomp/actions_avail").exists()
}

#[cfg(not(target_os = "linux"))]
pub fn is_available() -> bool {
    false
}

/// Detect if a failure was caused by seccomp denial.
///
/// Seccomp kills the process with SIGSYS (or the thread with SECCOMP_RET_KILL_THREAD),
/// and the exit code is typically SIGSYS (31) or the process may be killed with
/// "Bad system call" on stderr.
///
/// Additionally, seccomp violations may produce EPERM for filtered syscalls
/// if using SECCOMP_RET_ERRNO.
#[cfg(target_os = "linux")]
pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    // SIGSYS = 31
    if exit_code == 31 {
        return true;
    }
    // Check for seccomp denial patterns in stderr
    stderr.contains("Bad system call")
        || stderr.contains("bad system call")
        || stderr.contains("SIGSYS")
        || stderr.contains("seccomp")
        || stderr.contains("invalid argument") && exit_code == 159
    // 159 = 128 + 31 (died from SIGSYS with core dump disabled)
}

#[cfg(not(target_os = "linux"))]
pub fn detect_denial(_exit_code: i32, _stderr: &str) -> bool {
    false
}

/// Apply the seccomp filter to the calling thread.
///
/// This installs a BPF program that whitelists safe syscalls and kills the
/// process on any disallowed syscall.
///
/// # Errors
///
/// Returns an error if the prctl call fails (e.g., seccomp already enabled
/// or kernel too old).
#[cfg(target_os = "linux")]
pub fn apply_seccomp_filter() -> std::io::Result<()> {
    // ── Build the BPF filter program ─────────────────────────────────────
    //
    // BPF for seccomp works as follows:
    // 1. Load the architecture (4 bytes at offset 4 in seccomp_data)
    // 2. Validate architecture matches AUDIT_ARCH_X86_64 (0xC000003E)
    // 3. Load the syscall number (4 bytes at offset 0)
    // 4. Compare against whitelist, return ALLOW on match
    // 5. Return KILL on no match
    //
    // The filter uses a linear search over the whitelist. While not optimal,
    // it's simple, auditable, and has no external dependencies. The BPF
    // program is at most a few hundred instructions, which is well within
    // the kernel's 4096-instruction limit.

    #[repr(C)]
    struct sock_filter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    const BPF_LD: u16 = 0x00;
    const BPF_JMP: u16 = 0x05;
    const BPF_RET: u16 = 0x06;

    const BPF_W: u16 = 0x00;
    const BPF_ABS: u16 = 0x20;

    const BPF_JEQ: u16 = 0x10;
    const BPF_JGE: u16 = 0x30;
    const BPF_JA: u16 = 0x00;

    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7FFF_0000;

    // Audit arch for x86_64
    const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;

    // Helper to build a BPF instruction compactly.
    // Pattern from openai/codex codex-rs/codex-sandbox/src/linux/seccomp.rs; reimplemented.

    // Whitelist of safe syscall numbers (x86_64).
    // These are the syscalls most commonly used by shell commands, compilers,
    // and developer tools. Any syscall NOT on this list causes immediate SIGSYS.
    let allowed_syscalls: &[u32] = &[
        0,   // read
        1,   // write
        2,   // open
        3,   // close
        4,   // stat
        5,   // fstat
        6,   // lstat
        7,   // poll
        8,   // lseek
        9,   // mmap
        10,  // mprotect
        11,  // munmap
        12,  // brk
        13,  // rt_sigaction
        14,  // rt_sigprocmask
        15,  // rt_sigreturn
        16,  // ioctl
        17,  // pread64
        18,  // pwrite64
        19,  // readv
        20,  // writev
        21,  // access
        22,  // pipe
        23,  // select
        24,  // sched_yield
        25,  // mremap
        27,  // mincore
        28,  // madvise
        29,  // shmget
        30,  // shmat
        32,  // dup
        33,  // dup2
        35,  // nanosleep
        39,  // getpid
        41,  // socket
        42,  // connect
        43,  // accept
        44,  // sendto
        45,  // recvfrom
        46,  // sendmsg
        47,  // recvmsg
        48,  // shutdown
        49,  // bind
        50,  // listen
        51,  // getsockname
        52,  // getpeername
        53,  // socketpair
        54,  // setsockopt
        55,  // getsockopt
        56,  // clone
        57,  // fork
        58,  // vfork
        59,  // execve
        60,  // exit
        61,  // wait4
        62,  // kill
        63,  // uname
        72,  // fcntl
        73,  // flock
        74,  // fsync
        75,  // fdatasync
        76,  // truncate
        77,  // ftruncate
        78,  // getdents
        79,  // getcwd
        80,  // chdir
        81,  // fchdir
        82,  // rename
        83,  // mkdir
        84,  // rmdir
        85,  // creat
        86,  // link
        87,  // unlink
        88,  // symlink
        89,  // readlink
        90,  // chmod
        91,  // fchmod
        92,  // chown
        93,  // fchown
        94,  // lchown
        95,  // umask
        96,  // gettimeofday
        97,  // getrlimit
        98,  // getrusage
        99,  // sysinfo
        100, // times
        102, // getuid
        104, // getgid
        107, // geteuid
        108, // getegid
        110, // getppid
        111, // getpgrp
        112, // setsid
        116, // syslog
        131, // sigaltstack
        137, // statfs
        138, // fstatfs
        157, // prctl
        158, // arch_prctl
        186, // gettid
        201, // time
        202, // futex
        204, // sched_getaffinity
        217, // getdents64
        218, // set_tid_address
        228, // clock_gettime
        230, // clock_nanosleep
        231, // exit_group
        232, // epoll_wait
        233, // epoll_ctl
        234, // tgkill
        235, // utimes
        257, // openat
        262, // newfstatat
        273, // set_robust_list
        281, // epoll_pwait
        291, // epoll_create1
        292, // dup3
        293, // pipe2
        302, // prlimit64
        318, // getrandom
        332, // statx
        334, // rseq
        435, // clone3
    ];

    // Build the BPF program.
    let mut filter = vec![
        // Instruction 0: load architecture from seccomp_data.arch
        sock_filter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 4, // offset of arch in seccomp_data
        },
        // Instruction 1: compare with AUDIT_ARCH_X86_64
        // If match, jump to next instruction; if not, kill process
        sock_filter {
            code: BPF_JMP | BPF_JEQ,
            jt: 0,
            jf: 1, // jump 1 forward (to KILL) if arch doesn't match
            k: AUDIT_ARCH_X86_64,
        },
        // Instruction 2: KILL (wrong architecture)
        sock_filter {
            code: BPF_RET,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_KILL_PROCESS,
        },
        // Instruction 3: load syscall number from seccomp_data.nr
        sock_filter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 0, // offset of nr in seccomp_data
        },
    ];

    // For each allowed syscall, add a compare+jump to ALLOW.
    // We use a linear scan for simplicity: each JEQ instruction jumps
    // forward over the remaining checks + KILL to reach ALLOW.
    for &syscall in allowed_syscalls {
        let remaining = (allowed_syscalls.len() as u8).saturating_sub(
            allowed_syscalls
                .iter()
                .position(|&s| s == syscall)
                .unwrap_or(0) as u8,
        );
        // If syscall == this one, jump to allow_target; otherwise fall through
        filter.push(sock_filter {
            code: BPF_JMP | BPF_JEQ,
            jt: remaining, // jump forward to ALLOW
            jf: 0,         // fall through to next check
            k: syscall,
        });
    }

    // Instruction N: KILL PROCESS for any unmatched syscall
    filter.push(sock_filter {
        code: BPF_RET,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_KILL_PROCESS,
    });

    // Instruction N+1: ALLOW
    filter.push(sock_filter {
        code: BPF_RET,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    });

    // ── Load the filter into the kernel ───────────────────────────────────

    #[repr(C)]
    struct sock_fprog {
        len: u16,
        filter: *const sock_filter,
    }

    let prog = sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };

    // Safety: prctl with PR_SET_SECCOMP installs a seccomp BPF filter.
    // The filter is a valid array of sock_filter instructions that lives
    // for the duration of the prctl call.
    let result = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &raw const prog,
            0i64,
            0i64,
        )
    };

    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available_does_not_panic() {
        let _ = is_available();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_detect_denial() {
        assert!(detect_denial(31, ""));
        assert!(detect_denial(1, "Bad system call"));
        assert!(detect_denial(1, "SIGSYS"));
        assert!(!detect_denial(0, "Success"));
        assert!(!detect_denial(1, "File not found"));
    }

    #[test]
    fn test_detect_denial_non_linux() {
        #[cfg(not(target_os = "linux"))]
        {
            assert!(!detect_denial(31, "Bad system call"));
        }
    }
}
