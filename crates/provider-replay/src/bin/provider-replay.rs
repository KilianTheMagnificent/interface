//! Stands at a provider CLI's path and replays the transcript named by
//! `INTERFACE_REPLAY_TRANSCRIPT`. Exits 3 with a message on stderr when the
//! host sends a frame the recording does not expect.

use std::io::{self, BufRead as _, Write};

use provider_replay::{Start, TRANSCRIPT_ENV, Transcript, replay::parse_line, start};

/// The exit code of a failed replay.
const FAILED: i32 = 3;

fn main() {
    std::process::exit(match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("provider-replay: {message}");
            FAILED
        }
    });
}

fn run() -> Result<i32, String> {
    let path =
        std::env::var_os(TRANSCRIPT_ENV).ok_or_else(|| format!("{TRANSCRIPT_ENV} is not set"))?;
    let transcript = Transcript::read(path.as_ref())?;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir()
        .map(|cwd| cwd.to_string_lossy().into_owned())
        .map_err(|error| format!("no working directory: {error}"))?;
    let mut stdout = io::stdout().lock();
    let (mut replay, lines) = match start(&transcript, &argv, &cwd)? {
        Start::Probe { stdout: text, code } => {
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
            return Ok(code);
        }
        Start::Session { replay, lines } => (replay, lines),
    };
    write_lines(&mut stdout, &lines);
    let mut stdin = io::stdin().lock();
    let mut line = String::new();
    loop {
        line.clear();
        let read = stdin
            .read_line(&mut line)
            .map_err(|error| format!("stdin failed: {error}"))?;
        if read == 0 {
            return Ok(replay.finish());
        }
        let text = line.trim_end_matches(['\n', '\r']);
        if text.trim().is_empty() {
            continue;
        }
        let lines = replay.accept(&parse_line(text))?;
        write_lines(&mut stdout, &lines);
    }
}

/// Writes frames one per line. A host that stopped reading is not this
/// process's failure: it ends at stdin's close or its kill.
fn write_lines(stdout: &mut impl Write, lines: &[String]) {
    for line in lines {
        if writeln!(stdout, "{line}").is_err() {
            return;
        }
    }
    let _ = stdout.flush();
}
