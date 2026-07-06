fn main() -> std::process::ExitCode {
    // Reset SIGPIPE to SIG_DFL so piping codewhale output into a command that
    // exits early (e.g. `codewhale doctor | head`) terminates the process
    // cleanly with exit code 141 instead of panicking on the broken-pipe
    // write. Many execution environments (systemd, Docker, some shells)
    // inherit SIGPIPE set to SIG_IGN, which makes write(2) return EPIPE;
    // Rust's `println!` then treats that io::Error as fatal and panics.
    // See issue #4030.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    codewhale_cli::run_cli()
}
