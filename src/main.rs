use std::process::ExitCode;

fn main() -> ExitCode {
    let code = acre::cli::run();
    ExitCode::from(code.clamp(0, 255) as u8)
}
