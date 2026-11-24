//! The gate's Rust tests. `cargo test` runs the workspace's test executables
//! one after another and libtest parallelises only inside one, so the step
//! used to cost the sum of every executable's slowest test. Here cargo only
//! compiles them (`--no-run`, the same targets, profile and features), and the
//! executables it reports run [`WORKERS`] at a time, the longest first, each
//! the way cargo runs it: in its package's directory with cargo's variables.
//! Each executable's output prints as one block when it ends, and a table of
//! their times closes the step, so the next long pole is visible.
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{OsStr, OsString},
    fmt::Write as _,
    io::{self, BufRead as _, Read as _, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::Value;

/// How many test executables run at once. Most of their time is sleeps,
/// timeouts and single-threaded loops rather than cores, and four keeps the
/// longest one, not the sum, the step's floor on a 16-core machine.
pub const WORKERS: usize = 4;

/// Each test executable's wall time in tenths of a second, measured four at a
/// time beside Clippy and the Electron half in the gate of 6 October 2026
/// (load 35 to 50), so the queue starts the longest first. An executable not
/// listed counts as [`UNLISTED_WEIGHT`]; the step's closing table is where
/// new numbers come from.
const WEIGHTS: [(&str, u32); 18] = [
    ("interface-host lib", 469),
    ("interface-collaboration lib", 466),
    ("interface-collaboration test suite", 338),
    ("iface lib", 152),
    ("interface-terminal test suite", 100),
    ("interface-orchestration lib", 86),
    ("xtask bin xtask", 75),
    ("interface-desktop-services lib", 67),
    ("interface-settings lib", 65),
    ("interface-host test suite", 56),
    ("iface test tui_pty", 37),
    ("interface-review test suite", 37),
    ("engine-claude lib", 34),
    ("interface-terminal lib", 32),
    ("interface-registry lib", 31),
    ("engine-acp lib", 25),
    ("engine-codex lib", 18),
    ("interface-updater lib", 15),
];
/// A new executable is small until a measurement says otherwise.
const UNLISTED_WEIGHT: u32 = 10;

/// While executables run, how often the step names the ones still running,
/// so a hung test reads apart from a slow one.
const STILL_RUNNING_EVERY: Duration = Duration::from_secs(60);
/// How long an exited executable's output may stay open, held by a process
/// one of its tests left running, before the step stops waiting for it.
const OUTPUT_GRACE: Duration = Duration::from_secs(5);
/// The table names every executable that took at least this long.
const TABLE_FLOOR: Duration = Duration::from_secs(1);
/// How long one test executable may run before the step stops it. The
/// slowest takes about 47 s alone and has taken over 260 s on a loaded
/// machine; a hung one would otherwise hold closeout, and promote with its
/// lock, forever, as `npm run check` would without its own deadline.
const TEST_TIMEOUT: Duration = Duration::from_secs(600);
/// Replaces [`TEST_TIMEOUT`], in whole seconds.
const TEST_TIMEOUT_VARIABLE: &str = "INTERFACE_GATE_TEST_TIMEOUT";
/// How long a stopped executable and what it started get between TERM and
/// KILL.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// When a running executable is stopped: after `limit`, with `grace`
/// between TERM and KILL.
#[derive(Clone, Copy, Debug)]
struct Deadline {
    limit: Duration,
    grace: Duration,
}

/// One test executable `cargo test --no-run` built, and how cargo runs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestBinary {
    /// `<package> lib`, or `<package> <kind> <target>`: how the step names it.
    pub label: String,
    /// The `cargo test` command that runs it alone.
    pub rerun: String,
    pub executable: PathBuf,
    /// Its package's directory, where cargo runs it.
    pub directory: PathBuf,
    /// What cargo sets for a test executable it runs: `CARGO`, the manifest
    /// and package variables, and any `rustc-env` its build script printed.
    pub environment: Vec<(String, OsString)>,
}

/// One executable's verdict, time and output.
#[derive(Debug)]
pub struct BinaryRun {
    pub label: String,
    pub rerun: String,
    pub passed: bool,
    pub elapsed: Duration,
    pub output: String,
}

/// Compiles the workspace's tests once, streaming cargo's progress and
/// diagnostics, and returns the executables it would run, longest first.
/// `all_targets` matches the gate's own target selection: every target but
/// doc tests, which only `cargo test --doc` runs.
pub fn build(root: &Path, cargo: &OsStr, all_targets: bool) -> Result<Vec<TestBinary>> {
    let mut arguments = vec!["test", "--locked", "--workspace"];
    if all_targets {
        arguments.push("--all-targets");
    }
    arguments.extend(["--no-run", "--message-format=json"]);
    let mut child = Command::new(cargo)
        .current_dir(root)
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to launch {}", cargo.to_string_lossy()))?;
    let out = child.stdout.take().context("cargo stdout is unavailable")?;
    let mut messages = String::new();
    for line in io::BufReader::new(out).lines() {
        let line = line.context("could not read cargo's messages")?;
        // A diagnostic prints where cargo would have printed it; everything
        // else is kept for the artifact list.
        match serde_json::from_str::<Value>(&line) {
            Ok(message) if message["reason"] == "compiler-message" => {
                if let Some(rendered) = message["message"]["rendered"].as_str() {
                    io::stderr().write_all(rendered.as_bytes()).ok();
                }
            }
            _ => {
                messages.push_str(&line);
                messages.push('\n');
            }
        }
    }
    let status = child.wait().context("could not await cargo")?;
    ensure!(
        status.success(),
        "cargo {} failed with {status}",
        arguments.join(" ")
    );
    test_binaries(&messages, cargo)
}

/// The test executables in one `--message-format=json` build, longest first:
/// every `compiler-artifact` built in the test profile with an executable,
/// which is exactly the set `cargo test` runs.
pub fn test_binaries(messages: &str, cargo: &OsStr) -> Result<Vec<TestBinary>> {
    let mut script_environment: BTreeMap<String, Vec<(String, OsString)>> = BTreeMap::new();
    let mut artifacts = Vec::new();
    for line in messages.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match message["reason"].as_str() {
            Some("build-script-executed") => {
                let pairs = message["env"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|pair| {
                        Some((
                            pair[0].as_str()?.to_owned(),
                            OsString::from(pair[1].as_str()?),
                        ))
                    });
                script_environment
                    .entry(
                        message["package_id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                    )
                    .or_default()
                    .extend(pairs);
            }
            Some("compiler-artifact")
                if message["profile"]["test"] == true && message["executable"].is_string() =>
            {
                artifacts.push(message);
            }
            _ => {}
        }
    }
    let mut binaries = artifacts
        .iter()
        .map(|message| test_binary(message, cargo, &script_environment))
        .collect::<Result<Vec<_>>>()?;
    binaries.sort_by(|a, b| {
        weight(&b.label)
            .cmp(&weight(&a.label))
            .then_with(|| a.label.cmp(&b.label))
    });
    Ok(binaries)
}

fn test_binary(
    message: &Value,
    cargo: &OsStr,
    script_environment: &BTreeMap<String, Vec<(String, OsString)>>,
) -> Result<TestBinary> {
    let id = message["package_id"].as_str().unwrap_or_default();
    let (package, version) =
        package_name_version(id).with_context(|| format!("unreadable package id {id:?}"))?;
    let manifest = PathBuf::from(
        message["manifest_path"]
            .as_str()
            .with_context(|| format!("{package} has no manifest path"))?,
    );
    let directory = manifest
        .parent()
        .with_context(|| format!("{} has no directory", manifest.display()))?
        .to_path_buf();
    let target = message["target"]["name"].as_str().unwrap_or_default();
    let kind = message["target"]["kind"][0].as_str().unwrap_or_default();
    let (label, rerun) = match kind {
        "bin" | "test" | "example" | "bench" => (
            format!("{package} {kind} {target}"),
            format!("cargo test -p {package} --{kind} {target}"),
        ),
        _ => (
            format!("{package} lib"),
            format!("cargo test -p {package} --lib"),
        ),
    };
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.splitn(3, '.');
    let mut part = || parts.next().unwrap_or_default().to_owned();
    let (major, minor, patch) = (part(), part(), part());
    let pre = version
        .split_once('-')
        .map(|(_, pre)| pre.split('+').next().unwrap_or_default())
        .unwrap_or_default();
    let mut environment = vec![
        ("CARGO".to_owned(), cargo.to_os_string()),
        (
            "CARGO_MANIFEST_DIR".to_owned(),
            directory.clone().into_os_string(),
        ),
        ("CARGO_MANIFEST_PATH".to_owned(), manifest.into_os_string()),
        ("CARGO_PKG_NAME".to_owned(), package.into()),
        ("CARGO_PKG_VERSION".to_owned(), version.into()),
        ("CARGO_PKG_VERSION_MAJOR".to_owned(), major.into()),
        ("CARGO_PKG_VERSION_MINOR".to_owned(), minor.into()),
        ("CARGO_PKG_VERSION_PATCH".to_owned(), patch.into()),
        ("CARGO_PKG_VERSION_PRE".to_owned(), pre.into()),
    ];
    environment.extend(script_environment.get(id).into_iter().flatten().cloned());
    Ok(TestBinary {
        label,
        rerun,
        executable: PathBuf::from(message["executable"].as_str().unwrap_or_default()),
        directory,
        environment,
    })
}

/// A package id's name and version. `path+file:///…/crates/a#0.3.10` names
/// its package by the directory, `…#name@0.3.10` explicitly.
fn package_name_version(id: &str) -> Option<(&str, &str)> {
    let (url, fragment) = id.rsplit_once('#')?;
    let (name, version) = match fragment.split_once('@') {
        Some(named) => named,
        None => (url.rsplit('/').next()?, fragment),
    };
    (!name.is_empty() && !version.is_empty()).then_some((name, version))
}

fn weight(label: &str) -> u32 {
    WEIGHTS
        .iter()
        .find(|(listed, _)| *listed == label)
        .map_or(UNLISTED_WEIGHT, |&(_, weight)| weight)
}

/// Runs every executable, `workers` at a time in the order given, printing
/// each one's block as it ends, then the time table. Fails naming every
/// executable that failed and the command that reruns it.
pub fn run(binaries: Vec<TestBinary>, workers: usize, known_flaky: &[&str]) -> Result<()> {
    let count = binaries.len();
    let deadline = Deadline {
        limit: crate::check_run::timeout_from(TEST_TIMEOUT_VARIABLE, TEST_TIMEOUT)?,
        grace: STOP_GRACE,
    };
    let runs = run_all(binaries, workers, known_flaky, deadline);
    print!("{}", time_table(&runs, workers));
    let failed: Vec<_> = runs.iter().filter(|run| !run.passed).collect();
    if failed.is_empty() {
        println!("check: {count} test executables passed");
        return Ok(());
    }
    bail!(
        "cargo test failed in {} of {count} test executables:\n{}",
        failed.len(),
        failed
            .iter()
            .map(|run| format!("  {} (rerun: {})", run.label, run.rerun))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn run_all(
    binaries: Vec<TestBinary>,
    workers: usize,
    known_flaky: &[&str],
    deadline: Deadline,
) -> Vec<BinaryRun> {
    let workers = workers.clamp(1, binaries.len().max(1));
    let queue = Mutex::new(VecDeque::from(binaries));
    let running: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        for _ in 0..workers {
            let (queue, running, done) = (&queue, &running, done.clone());
            scope.spawn(move || {
                loop {
                    let Some(binary) = queue.lock().ok().and_then(|mut queue| queue.pop_front())
                    else {
                        return;
                    };
                    if let Ok(mut running) = running.lock() {
                        running.insert(binary.label.clone(), Instant::now());
                    }
                    let run = run_with_retry(&binary, known_flaky, deadline);
                    if let Ok(mut running) = running.lock() {
                        running.remove(&binary.label);
                    }
                    if done.send(run).is_err() {
                        return;
                    }
                }
            });
        }
        drop(done);
        let mut runs = Vec::new();
        loop {
            match finished.recv_timeout(STILL_RUNNING_EVERY) {
                Ok(run) => {
                    // One `print!` holds stdout for the whole block, so no
                    // other step's line lands inside it.
                    print!("{}", block(&run));
                    io::stdout().flush().ok();
                    runs.push(run);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Ok(running) = running.lock() {
                        let names: Vec<_> = running
                            .iter()
                            .map(|(label, started)| {
                                format!("{label} ({} s)", started.elapsed().as_secs())
                            })
                            .collect();
                        println!("check: still running: {}", names.join(", "));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return runs,
            }
        }
    })
}

/// Runs one executable. A failure whose every named test is a known flake
/// reruns that executable alone, once; anything else stands.
fn run_with_retry(binary: &TestBinary, known_flaky: &[&str], deadline: Deadline) -> BinaryRun {
    let first = run_binary(binary, deadline);
    if first.passed {
        return first;
    }
    let failed = crate::check_run::failed_test_names(&first.output);
    if !crate::gate::only_listed(known_flaky, &failed) {
        return first;
    }
    let names = failed.join(", ");
    println!(
        "check: flaky test(s) {names} — re-running {} once",
        binary.label
    );
    let mut retry = run_binary(binary, deadline);
    retry.output = format!(
        "{}\ncheck: the first run failed only in known flaky test(s) {names}; this is the retry\n{}",
        first.output, retry.output
    );
    retry.elapsed += first.elapsed;
    retry
}

fn run_binary(binary: &TestBinary, deadline: Deadline) -> BinaryRun {
    let started = Instant::now();
    let (passed, output) = match run_captured(binary, deadline) {
        Ok(result) => result,
        Err(error) => (false, format!("{error:#}\n")),
    };
    BinaryRun {
        label: binary.label.clone(),
        rerun: binary.rerun.clone(),
        passed,
        elapsed: started.elapsed(),
        output,
    }
}

/// Runs one executable in libtest's terse format with stdout and stderr in
/// one pipe, in the order it wrote them, and returns its verdict and output.
/// Past the deadline it stops the executable and everything it started (it
/// stays in this process group, so ^C still reaches all of it) and fails
/// with the output so far.
fn run_captured(binary: &TestBinary, deadline: Deadline) -> Result<(bool, String)> {
    let (mut reader, writer) = io::pipe().context("could not open an output pipe")?;
    let mut command = Command::new(&binary.executable);
    command
        .arg("--quiet")
        .current_dir(&binary.directory)
        .stdin(Stdio::null())
        .stdout(
            writer
                .try_clone()
                .context("could not share the output pipe")?,
        )
        .stderr(writer);
    for (name, value) in &binary.environment {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to launch {}", binary.executable.display()))?;
    // The command holds the pipe's write ends; only the child may keep them.
    drop(command);
    let (chunks, received) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; 8_192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    if chunks.send(buffer[..read].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let stop_at = Instant::now() + deadline.limit;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("could not await a test executable")?
        {
            break Some(status);
        }
        if Instant::now() >= stop_at {
            crate::check_run::stop_tree(&mut child, deadline.grace);
            break None;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let drained_by = Instant::now() + OUTPUT_GRACE;
    let mut output = Vec::new();
    loop {
        match received.recv_timeout(drained_by.saturating_duration_since(Instant::now())) {
            Ok(chunk) => output.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                output.extend_from_slice(
                    b"\ncheck: exited; its output is still held open by a process it left running\n",
                );
                break;
            }
        }
    }
    let mut output = String::from_utf8_lossy(&output).into_owned();
    let Some(status) = status else {
        let _ = writeln!(
            output,
            "check: did not finish within {} s and was stopped \
             ({TEST_TIMEOUT_VARIABLE} sets the limit in seconds)",
            deadline.limit.as_secs_f64()
        );
        return Ok((false, output));
    };
    if !status.success() {
        let _ = writeln!(output, "check: exited with {status}");
    }
    Ok((status.success(), output))
}

/// One executable's block: a header with its verdict and time, then its
/// output as it wrote it.
fn block(run: &BinaryRun) -> String {
    let seconds = run.elapsed.as_secs_f64();
    let header = if run.passed {
        format!("check: test {} passed in {seconds:.1} s", run.label)
    } else {
        format!(
            "check: test {} FAILED after {seconds:.1} s (rerun: {})",
            run.label, run.rerun
        )
    };
    let output = run.output.trim_matches('\n');
    if output.is_empty() {
        format!("{header}\n")
    } else {
        format!("{header}\n{output}\n")
    }
}

/// Every executable that took [`TABLE_FLOOR`] or longer, slowest first, and
/// one line for the rest.
fn time_table(runs: &[BinaryRun], workers: usize) -> String {
    let mut sorted: Vec<&BinaryRun> = runs.iter().collect();
    sorted.sort_by(|a, b| {
        b.elapsed
            .cmp(&a.elapsed)
            .then_with(|| a.label.cmp(&b.label))
    });
    let (long, short): (Vec<&BinaryRun>, Vec<&BinaryRun>) = sorted
        .into_iter()
        .partition(|run| run.elapsed >= TABLE_FLOOR);
    let mut table = format!("check: test executables, {workers} at a time, slowest first:\n");
    for run in long {
        let verdict = if run.passed { "" } else { "  FAILED" };
        let _ = writeln!(
            table,
            "check: {:>7.1} s  {}{verdict}",
            run.elapsed.as_secs_f64(),
            run.label
        );
    }
    if !short.is_empty() {
        let together: Duration = short.iter().map(|run| run.elapsed).sum();
        let failed = short.iter().filter(|run| !run.passed).count();
        let failed = if failed == 0 {
            String::new()
        } else {
            format!(", {failed} FAILED")
        };
        let _ = writeln!(
            table,
            "check: {:>7.1} s  {} more under {} s{failed}",
            together.as_secs_f64(),
            short.len(),
            TABLE_FLOOR.as_secs()
        );
    }
    table
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_fixtures::executable;

    /// The gate's own deadline, which none of these executables comes near.
    const GATE: Deadline = Deadline {
        limit: TEST_TIMEOUT,
        grace: STOP_GRACE,
    };

    /// Lines as `cargo test --no-run --message-format=json` prints them,
    /// trimmed to the fields the runner reads.
    fn messages(root: &Path) -> String {
        let root = root.display();
        [
            format!(
                r#"{{"reason":"build-script-executed","package_id":"path+file://{root}/crates/scripted#0.3.10","env":[["SCRIPTED","yes"]]}}"#
            ),
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"path+file://{root}/crates/scripted#0.3.10","manifest_path":"{root}/crates/scripted/Cargo.toml","target":{{"kind":["lib"],"name":"scripted"}},"profile":{{"test":true}},"executable":"{root}/target/debug/deps/scripted-1"}}"#
            ),
            // Built for a test to launch, not a test: cargo does not run it.
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"path+file://{root}/apps/interface-host#0.3.10","manifest_path":"{root}/apps/interface-host/Cargo.toml","target":{{"kind":["bin"],"name":"interface-host"}},"profile":{{"test":false}},"executable":"{root}/target/debug/interface-host"}}"#
            ),
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"path+file://{root}/apps/interface-host#0.3.10","manifest_path":"{root}/apps/interface-host/Cargo.toml","target":{{"kind":["test"],"name":"suite"}},"profile":{{"test":true}},"executable":"{root}/target/debug/deps/suite-2"}}"#
            ),
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"path+file://{root}/apps/interface-host#0.3.10","manifest_path":"{root}/apps/interface-host/Cargo.toml","target":{{"kind":["lib"],"name":"interface_host"}},"profile":{{"test":true}},"executable":"{root}/target/debug/deps/interface_host-3"}}"#
            ),
            // A library's rlib has no executable.
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"path+file://{root}/crates/scripted#0.3.10","manifest_path":"{root}/crates/scripted/Cargo.toml","target":{{"kind":["lib"],"name":"scripted"}},"profile":{{"test":false}},"executable":null}}"#
            ),
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"path+file://{root}/apps/tool#renamed@1.2.3-beta.1+build","manifest_path":"{root}/apps/tool/Cargo.toml","target":{{"kind":["bin"],"name":"renamed"}},"profile":{{"test":true}},"executable":"{root}/target/debug/deps/renamed-4"}}"#
            ),
            r#"{"reason":"build-finished","success":true}"#.to_owned(),
            "not json".to_owned(),
        ]
        .join("\n")
    }

    #[test]
    fn the_executables_are_what_cargo_test_runs_longest_first() {
        let root = Path::new("/w");
        let binaries = test_binaries(&messages(root), OsStr::new("/bin/cargo")).unwrap();
        let labels: Vec<_> = binaries
            .iter()
            .map(|binary| binary.label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "interface-host lib",
                "interface-host test suite",
                "renamed bin renamed",
                "scripted lib",
            ],
            "measured weights first, then unlisted ones by name; no non-test binary"
        );
        let suite = &binaries[1];
        assert_eq!(suite.rerun, "cargo test -p interface-host --test suite");
        assert_eq!(suite.executable, Path::new("/w/target/debug/deps/suite-2"));
        assert_eq!(suite.directory, Path::new("/w/apps/interface-host"));
        let variable = |binary: &TestBinary, name: &str| {
            binary
                .environment
                .iter()
                .find(|(listed, _)| listed == name)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        assert_eq!(variable(suite, "CARGO").as_deref(), Some("/bin/cargo"));
        assert_eq!(
            variable(suite, "CARGO_MANIFEST_DIR").as_deref(),
            Some("/w/apps/interface-host")
        );
        assert_eq!(
            variable(suite, "CARGO_PKG_NAME").as_deref(),
            Some("interface-host")
        );
        assert_eq!(variable(suite, "SCRIPTED"), None);
        let renamed = &binaries[2];
        assert_eq!(renamed.rerun, "cargo test -p renamed --bin renamed");
        for (name, value) in [
            ("CARGO_PKG_NAME", "renamed"),
            ("CARGO_PKG_VERSION", "1.2.3-beta.1+build"),
            ("CARGO_PKG_VERSION_MAJOR", "1"),
            ("CARGO_PKG_VERSION_MINOR", "2"),
            ("CARGO_PKG_VERSION_PATCH", "3"),
            ("CARGO_PKG_VERSION_PRE", "beta.1"),
        ] {
            assert_eq!(variable(renamed, name).as_deref(), Some(value), "{name}");
        }
        // Cargo also sets a build script's `rustc-env` for its package's tests.
        assert_eq!(variable(&binaries[3], "SCRIPTED").as_deref(), Some("yes"));
        assert_eq!(binaries[3].rerun, "cargo test -p scripted --lib");
    }

    #[test]
    fn package_ids_name_their_package_and_version() {
        assert_eq!(
            package_name_version("path+file:///w/crates/interface-terminal#0.3.10"),
            Some(("interface-terminal", "0.3.10"))
        );
        assert_eq!(
            package_name_version("path+file:///w/apps/tool#renamed@1.0.0"),
            Some(("renamed", "1.0.0"))
        );
        assert_eq!(
            package_name_version(
                "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0"
            ),
            Some(("serde", "1.0.0"))
        );
        assert_eq!(package_name_version("no fragment"), None);
    }

    fn fake(directory: &Path, name: &str, script: &str) -> TestBinary {
        let path = directory.join(name);
        executable(&path, format!("#!/bin/sh\n{script}").as_bytes());
        TestBinary {
            label: format!("fake {name}"),
            rerun: format!("cargo test -p fake --test {name}"),
            executable: path,
            directory: directory.to_path_buf(),
            environment: vec![("CARGO_PKG_NAME".to_owned(), "fake".into())],
        }
    }

    #[test]
    fn an_executable_runs_where_cargo_runs_it_with_its_output_in_one_block() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let binary = fake(
            &directory,
            "where",
            "echo \"$1 $(pwd) $CARGO_PKG_NAME\"\necho on-stderr >&2\necho after\n",
        );
        let run = run_binary(&binary, GATE);
        assert!(run.passed, "{}", run.output);
        assert_eq!(
            run.output,
            format!("--quiet {} fake\non-stderr\nafter\n", directory.display()),
            "libtest's terse format, the package directory, cargo's variables, \
             and stdout and stderr in the order they were written"
        );
        assert_eq!(
            block(&run).lines().next().unwrap(),
            format!(
                "check: test fake where passed in {:.1} s",
                run.elapsed.as_secs_f64()
            )
        );
    }

    #[test]
    fn every_failing_executable_is_named_with_the_command_that_reruns_it() {
        let temp = tempfile::tempdir().unwrap();
        let binaries = vec![
            fake(temp.path(), "slow", "sleep 1\necho fine\n"),
            fake(
                temp.path(),
                "broken",
                "echo 'failures:'\necho '    suite::beta'\nexit 101\n",
            ),
            fake(temp.path(), "quick", "echo fine\n"),
        ];
        let runs = run_all(binaries.clone(), 2, &[], GATE);
        assert_eq!(runs.len(), 3, "every executable runs, failed or not");
        let broken = runs.iter().find(|run| run.label == "fake broken").unwrap();
        assert!(!broken.passed);
        let printed = block(broken);
        assert!(
            printed.starts_with("check: test fake broken FAILED after ")
                && printed.contains("(rerun: cargo test -p fake --test broken)")
                && printed.contains("    suite::beta")
                && printed.contains("exited with"),
            "{printed}"
        );
        let error = run(binaries, 2, &[]).unwrap_err().to_string();
        assert_eq!(
            error,
            "cargo test failed in 1 of 3 test executables:\n  \
             fake broken (rerun: cargo test -p fake --test broken)"
        );
        let table = time_table(&runs, 2);
        assert!(
            table.starts_with("check: test executables, 2 at a time, slowest first:\n"),
            "{table}"
        );
        assert!(table.contains("s  fake slow\n"), "{table}");
        assert!(table.contains("2 more under 1 s, 1 FAILED\n"), "{table}");
    }

    #[test]
    fn only_a_known_flake_reruns_and_only_its_own_executable() {
        let temp = tempfile::tempdir().unwrap();
        let count = |name: &str| {
            fs::read_to_string(temp.path().join(format!("{name}.runs")))
                .unwrap_or_default()
                .lines()
                .count()
        };
        // Fails its first run only, naming one test in libtest's summary.
        let flaky = |name: &str, test: &str| {
            fake(
                temp.path(),
                name,
                &format!(
                    "echo run >> {name}.runs\n\
                     [ $(wc -l < {name}.runs) -gt 1 ] && exit 0\n\
                     printf 'failures:\\n    {test}\\n'\nexit 101\n"
                ),
            )
        };
        let steady = fake(temp.path(), "steady", "echo run >> steady.runs\n");
        let known = run_all(
            vec![flaky("known", "a_timing_test"), steady.clone()],
            2,
            &["a_timing_test"],
            GATE,
        );
        assert!(known.iter().all(|run| run.passed));
        assert_eq!((count("known"), count("steady")), (2, 1));
        let retried = known.iter().find(|run| run.label == "fake known").unwrap();
        assert!(
            retried.output.contains("this is the retry"),
            "{}",
            retried.output
        );
        let unknown = run_all(
            vec![flaky("unknown", "a_real_failure")],
            1,
            &["a_timing_test"],
            GATE,
        );
        assert!(!unknown[0].passed, "an unknown failure is never retried");
        assert_eq!(count("unknown"), 1);
    }

    #[test]
    fn an_executable_that_cannot_start_fails_with_the_reason() {
        let binary = TestBinary {
            label: "fake missing".to_owned(),
            rerun: "cargo test -p fake --lib".to_owned(),
            executable: PathBuf::from("/nonexistent/test-executable"),
            directory: PathBuf::from("/"),
            environment: Vec::new(),
        };
        let run = run_binary(&binary, GATE);
        assert!(!run.passed);
        assert!(
            run.output
                .contains("failed to launch /nonexistent/test-executable")
        );
    }

    #[test]
    fn an_executable_past_its_deadline_is_stopped_with_what_it_started() {
        let temp = tempfile::tempdir().unwrap();
        let recorded = temp.path().join("sleep.pid");
        let binary = fake(
            temp.path(),
            "hung",
            &format!(
                "/bin/sleep 600 &\necho $! > '{}'\necho before the hang\nwait\n",
                recorded.display()
            ),
        );
        let started = Instant::now();
        // A short grace: stopping the tree is under test, not the gate's
        // five seconds.
        let run = run_binary(
            &binary,
            Deadline {
                limit: Duration::from_secs(2),
                grace: Duration::from_millis(200),
            },
        );
        assert!(started.elapsed() < Duration::from_secs(30), "{run:?}");
        assert!(!run.passed);
        assert!(run.output.contains("before the hang"), "{}", run.output);
        assert!(
            run.output
                .contains("did not finish within 2 s and was stopped (INTERFACE_GATE_TEST_TIMEOUT"),
            "{}",
            run.output
        );
        assert!(
            block(&run).starts_with("check: test fake hung FAILED after "),
            "{}",
            block(&run)
        );
        // The sleep the executable started goes with it, not only the shell.
        let sleep = fs::read_to_string(&recorded).unwrap();
        let sleep = sleep.trim();
        let alive = || {
            Command::new("/bin/kill")
                .args(["-0", sleep])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        let gone_by = Instant::now() + Duration::from_secs(10);
        while alive() && Instant::now() < gone_by {
            thread::sleep(Duration::from_millis(100));
        }
        assert!(!alive(), "pid {sleep} outlived the stopped executable");
    }
}
