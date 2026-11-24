//! Stands at a provider CLI's path, runs the real CLI named by
//! `INTERFACE_RECORD_TARGET` with stdin and stdout passed through, and
//! records the session to `INTERFACE_RECORD_OUT` (scrubbed).

fn main() {
    std::process::exit(match provider_replay::record::run_recorder() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("provider-record: {message}");
            127
        }
    });
}
