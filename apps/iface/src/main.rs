use std::{env, io::Write, process::ExitCode};

use iface::{
    app::{self, AppEnvironment, read_stdin_if_needed, set_product_version},
    argv::{CLI_USAGE, parse_argv},
    errors, sanitize,
};

/// What `iface --version` and the TUI title report and the Codex and ACP
/// adapters send in their handshakes: the release-stamped product version
/// (see `interface_common::product_version!`). Read in the binary target only:
/// the library then stays cached across releases.
const PRODUCT_VERSION: &str = interface_common::product_version!();

#[tokio::main]
async fn main() -> ExitCode {
    set_product_version(PRODUCT_VERSION);
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let parsed = match parse_argv(&arguments) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("iface: {error}");
            eprintln!("{CLI_USAGE}");
            return ExitCode::from(error.exit_code());
        }
    };
    let mut environment = match AppEnvironment::system(arguments) {
        Ok(environment) => environment,
        Err(error) => {
            eprintln!("iface: {}", sanitize::for_display(&error));
            return ExitCode::from(errors::CLI_EXIT.generic);
        }
    };
    environment.stdin = match read_stdin_if_needed(&parsed) {
        Ok(stdin) => stdin,
        Err(result) => {
            let _ = std::io::stdout().write_all(result.stdout.as_bytes());
            let _ = std::io::stderr().write_all(result.stderr.as_bytes());
            return ExitCode::from(result.exit_code);
        }
    };
    let progress: interface_collaboration::away::Progress = std::sync::Arc::new(|text| {
        println!("{}", sanitize::for_display(text));
        let _ = std::io::stdout().flush();
    });
    let result = app::run_with_progress(parsed, &mut environment, Some(progress)).await;
    let _ = std::io::stdout().write_all(result.stdout.as_bytes());
    let _ = std::io::stderr().write_all(result.stderr.as_bytes());
    ExitCode::from(result.exit_code)
}
