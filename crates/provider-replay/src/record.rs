//! The recorder: stands at a provider's executable path, runs the real CLI
//! named by `INTERFACE_RECORD_TARGET`, passes stdin and stdout through
//! untouched, and journals every line beside `INTERFACE_RECORD_OUT`
//! (`<out>.parts/`, one owner-private file per process, flushed line by
//! line, because Codex and ACP sessions end by killing their process).
//! [`finish`] turns the journal into a scrubbed transcript at `<out>`; the
//! recorder calls it when its CLI exits, and the recording test calls it
//! again once every session has closed.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead as _, BufReader, Write as _},
    os::unix::{
        fs::{DirBuilderExt as _, OpenOptionsExt as _},
        process::ExitStatusExt as _,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::{
    replay::parse_line,
    scrub::{Identity, Scrubber},
    transcript::{Entry, Probe, TRANSCRIPT_VERSION, Transcript, probe_key},
};

/// The real CLI the recorder runs.
pub const RECORD_TARGET_ENV: &str = "INTERFACE_RECORD_TARGET";
/// Where the scrubbed transcript goes; the raw journal sits beside it.
pub const RECORD_OUT_ENV: &str = "INTERFACE_RECORD_OUT";

/// What a transcript says about its recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Meta {
    pub engine: String,
    pub protocol: String,
    pub cli_version: String,
    pub scenario: String,
    pub recorded_at: String,
}

/// The raw journal directory for the transcript at `out`.
#[must_use]
pub fn journal_directory(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".parts");
    out.with_file_name(name)
}

/// The protocol a provider's CLI speaks, by engine.
#[must_use]
pub fn protocol_of(engine: &str) -> &'static str {
    match engine {
        "claude" => "stream-json",
        "codex" => "app-server",
        _ => "acp",
    }
}

/// Today as `YYYY-MM-DD`, in UTC.
#[must_use]
pub fn today() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or_default();
    let (year, month, day) = interface_common::time::civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Runs the recorder for this process's argv and answers its exit code.
///
/// # Errors
/// The environment does not name a target and an output, or the CLI could
/// not start.
pub fn run_recorder() -> Result<i32, String> {
    let target = std::env::var_os(RECORD_TARGET_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| format!("{RECORD_TARGET_ENV} must name the real CLI by absolute path"))?;
    let out = std::env::var_os(RECORD_OUT_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| format!("{RECORD_OUT_ENV} must be an absolute path"))?;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir()
        .map(|cwd| cwd.to_string_lossy().into_owned())
        .unwrap_or_default();
    let journal = Arc::new(Journal::create(&out)?);
    journal.write(&json!({"argv": argv, "cwd": cwd}));

    let mut child = Command::new(&target)
        .args(&argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", target.display()))?;
    let mut child_stdin = child.stdin.take().ok_or("no stdin pipe")?;
    let child_stdout = child.stdout.take().ok_or("no stdout pipe")?;

    let inbound = Arc::clone(&journal);
    std::thread::spawn(move || {
        let mut stdin = BufReader::new(io::stdin().lock());
        let mut line = Vec::new();
        loop {
            line.clear();
            match stdin.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    // Journaled before it reaches the CLI, so an answer can
                    // never be journaled ahead of the frame it answers.
                    inbound.line("out", &line);
                    if child_stdin.write_all(&line).is_err() || child_stdin.flush().is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut reader = BufReader::new(child_stdout);
    let mut stdout = io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                journal.line("in", &line);
                if stdout.write_all(&line).is_err() || stdout.flush().is_err() {
                    break;
                }
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("could not wait for the CLI: {error}"))?;
    let code = status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1);
    journal.write(&json!({"dir": "exit", "code": code}));
    let engine = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let meta = Meta {
        protocol: protocol_of(&engine).to_owned(),
        cli_version: cli_version(&target),
        scenario: out
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
        recorded_at: today(),
        engine,
    };
    if let Err(error) = finish(&out, &meta, &Identity::current()) {
        eprintln!("provider-record: {error}");
    }
    Ok(code)
}

/// The first line `<cli> --version` prints.
#[must_use]
pub fn cli_version(cli: &Path) -> String {
    Command::new(cli)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(|line| line.trim().to_owned())
        })
        .unwrap_or_default()
}

/// One process's raw journal file.
struct Journal(Mutex<File>);

impl Journal {
    fn create(out: &Path) -> Result<Self, String> {
        let directory = journal_directory(out);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = directory.join(format!("{nanos:024}-{:08}.ndjson", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        Ok(Self(Mutex::new(file)))
    }

    fn line(&self, direction: &str, bytes: &[u8]) {
        self.write(&json!({"dir": direction, "line": String::from_utf8_lossy(bytes)}));
    }

    fn write(&self, value: &Value) {
        let mut file = interface_common::sync::lock(&self.0);
        let _ = writeln!(file, "{value}");
        let _ = file.flush();
    }
}

/// One journaled process.
struct Run {
    argv: Vec<String>,
    cwd: String,
    lines: Vec<(String, String)>,
    exit: Option<i32>,
}

/// Reads the journal beside `out`, scrubs it, and writes the transcript to
/// `out` (atomically). A process that received a frame is a launch; any
/// other is a probe, keyed by its argv.
///
/// # Errors
/// The journal is missing or unreadable, or `out` could not be written.
pub fn finish(out: &Path, meta: &Meta, identity: &Identity) -> Result<Transcript, String> {
    let directory = journal_directory(out);
    let mut names: Vec<PathBuf> = fs::read_dir(&directory)
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "ndjson")
        })
        .collect();
    names.sort();
    let mut runs = Vec::new();
    for path in names {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        if let Some(run) = parse_run(&text) {
            runs.push(run);
        }
    }
    let mut scrub = Scrubber::new(identity.clone());
    let mut probes = BTreeMap::new();
    let mut entries = Vec::new();
    for run in runs {
        let argv: Vec<String> = run
            .argv
            .iter()
            .map(|argument| scrub.text(argument))
            .collect();
        if run.lines.iter().all(|(direction, _)| direction != "out") {
            let stdout: String = run.lines.iter().map(|(_, line)| line.as_str()).collect();
            probes.insert(
                probe_key(&argv),
                Probe {
                    stdout: scrub.text(&stdout),
                    code: run.exit.unwrap_or(0),
                },
            );
            continue;
        }
        entries.push(Entry::Launch {
            argv,
            cwd: scrub.text(&run.cwd),
        });
        for (direction, line) in &run.lines {
            let text = line.trim_end_matches(['\n', '\r']);
            if text.trim().is_empty() {
                continue;
            }
            let frame = scrub.value(&parse_line(text));
            entries.push(if direction == "out" {
                Entry::Out { frame }
            } else {
                Entry::In { frame }
            });
        }
        if let Some(code) = run.exit {
            entries.push(Entry::Exit { code });
        }
    }
    let transcript = Transcript {
        version: TRANSCRIPT_VERSION,
        engine: meta.engine.clone(),
        protocol: meta.protocol.clone(),
        cli_version: scrub.text(&meta.cli_version),
        scenario: meta.scenario.clone(),
        recorded_at: meta.recorded_at.clone(),
        probes,
        entries,
    };
    let temporary = out.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, transcript.to_file_text())
        .and_then(|()| fs::rename(&temporary, out))
        .map_err(|error| format!("could not write {}: {error}", out.display()))?;
    Ok(transcript)
}

fn parse_run(text: &str) -> Option<Run> {
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next()?).ok()?;
    let mut run = Run {
        argv: header["argv"]
            .as_array()?
            .iter()
            .filter_map(|argument| argument.as_str().map(str::to_owned))
            .collect(),
        cwd: header["cwd"].as_str().unwrap_or_default().to_owned(),
        lines: Vec::new(),
        exit: None,
    };
    for line in lines {
        // A process killed mid-write leaves a torn last line: skip it.
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value["dir"].as_str() {
            Some("exit") => {
                run.exit = value["code"]
                    .as_i64()
                    .and_then(|code| i32::try_from(code).ok());
            }
            Some(direction @ ("out" | "in")) => {
                run.lines.push((
                    direction.to_owned(),
                    value["line"].as_str().unwrap_or_default().to_owned(),
                ));
            }
            _ => {}
        }
    }
    Some(run)
}
