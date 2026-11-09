use std::process::ExitCode;

fn main() -> ExitCode {
    match vcal::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("vcal: {err:#}");
            ExitCode::FAILURE
        }
    }
}
