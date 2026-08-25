fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::ExitCode::from(codewhale_computer_use::run(args).clamp(0, 255) as u8)
}
