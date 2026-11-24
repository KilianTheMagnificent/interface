//! The canonical gate's two suites. The Rust half runs formatting, the
//! workspace tests (`test_runner`) with Clippy beside them, and the Swift
//! helper's build and tests; `npm run check` runs alongside it. Each step and
//! half is timed, and both verdicts are reported. Which suites run, and the
//! receipts they earn, is `gate_cache`'s decision.
use std::{
    collections::{BTreeSet, VecDeque},
    env,
    ffi::OsStr,
    io::{self, BufRead as _, Read, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

use crate::package::{
    SMOKE_TAIL_LINES, VOICE_PACKAGE_DIR, XCRUN, build_voice_helper, run_command, run_command_os,
};

/// How long `npm run check` may run before the gate stops it. The Electron
/// suite takes about three minutes; a stalled lane would otherwise hold
/// closeout, and promote with its lock, forever. Load around 14 is normal
/// here, hence the margin.
const NPM_CHECK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Replaces [`NPM_CHECK_TIMEOUT`], in whole seconds.
const NPM_CHECK_TIMEOUT_VARIABLE: &str = "INTERFACE_GATE_NPM_TIMEOUT";
/// How long a signalled npm process tree gets before it is killed, and how
/// long npm's output may stay open after it exits.
const NPM_STOP_GRACE: Duration = Duration::from_secs(5);
/// Chooses how many renderer DOM lanes run at once (`test-renderer-dom.mjs`).
const RENDERER_POOL_VARIABLE: &str = "INTERFACE_RENDERER_POOL";
/// What marks the Electron suite's lines among cargo's.
const NPM_PREFIX: &str = "npm| ";

/// One half of the gate: its verdict and its own wall time, zero when the
/// half did not run.
pub struct HalfOutcome {
    pub result: Result<()>,
    pub elapsed: Duration,
}

impl HalfOutcome {
    const fn skipped() -> Self {
        Self {
            result: Ok(()),
            elapsed: Duration::ZERO,
        }
    }
}

/// Both halves of one gate run.
pub struct CheckOutcome {
    pub rust: HalfOutcome,
    pub js: HalfOutcome,
    /// The Swift helper's steps' time, when they ran in this run and passed.
    pub voice: Option<Duration>,
}

/// Runs the selected halves. `voice` runs the Swift helper's steps within the
/// Rust half; the caller skips them only when a receipt covers identical
/// helper inputs.
pub fn run_check_results(root: &Path, rust: bool, js: bool, voice: bool) -> CheckOutcome {
    // The Electron check shares no state with the cargo pipeline, so it runs
    // alongside it. Its lines stream as they arrive under `npm| `, so a slow
    // suite reads apart from a hung one, and a stalled one is stopped at its
    // deadline rather than holding the gate forever.
    let electron = root.join("electron");
    let pool = renderer_pool(rust);
    let npm = js.then(|| {
        thread::spawn(move || {
            // One step, `npm run check`, which prints its own parts' times.
            timed_half("Electron", || {
                // An explicit pool size the caller exported still wins.
                let variables: Vec<(&str, &str)> = if env::var_os(RENDERER_POOL_VARIABLE).is_some()
                {
                    Vec::new()
                } else {
                    vec![(RENDERER_POOL_VARIABLE, pool)]
                };
                run_npm(
                    OsStr::new("npm"),
                    &["run", "check"],
                    &electron,
                    npm_check_timeout()?,
                    NPM_STOP_GRACE,
                    &variables,
                )
            })
        })
    });
    let voice = rust && voice;
    let mut voice_passed = None;
    let cargo = if rust {
        timed_half("Rust", || {
            // `cargo fmt` takes no `--locked`; the two that resolve dependencies
            // do, so the gate can never quietly rewrite Cargo.lock.
            timed_step("cargo fmt", || {
                run_cargo(root, ["fmt", "--all", "--", "--check"])
            })?;
            thread::scope(|scope| {
                // The helper ships inside the app, so a Swift break has to fail
                // the gate. Buffer tests need no microphone, permissions, or
                // speech asset; the interactive audio self-test remains a
                // separate runtime gate. No Rust step reads the built helper
                // (the protocol tests drive fakes), so the Swift steps run
                // beside clippy and the tests instead of before them.
                let swift = voice.then(|| {
                    scope.spawn(|| {
                        let started = Instant::now();
                        timed_step("voice helper build", || build_voice_helper(root, "debug"))?;
                        timed_step("swift test", || {
                            run_command(XCRUN, ["swift", "test"], &root.join(VOICE_PACKAGE_DIR))
                        })?;
                        Ok(started.elapsed())
                    })
                });
                if !voice {
                    println!(
                        "check: voice helper reused — identical {VOICE_PACKAGE_DIR} inputs and toolchain"
                    );
                }
                let cargo = run_rust_tests(root, crate::gate::runs_smoke_pins(rust, js));
                let swift = swift.map_or(Ok(None), |swift| {
                    swift
                        .join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("the Swift steps panicked")))
                        .map(Some)
                });
                voice_passed = swift.as_ref().ok().copied().flatten();
                all_of(vec![cargo, swift.map(drop)])
            })
        })
    } else {
        HalfOutcome::skipped()
    };
    // Always join and report both halves, even when Cargo failed first.
    let js = npm.map_or_else(HalfOutcome::skipped, |npm| {
        npm.join().unwrap_or_else(|_| HalfOutcome {
            result: Err(anyhow::anyhow!("npm run check panicked")),
            elapsed: Duration::ZERO,
        })
    });
    CheckOutcome {
        rust: cargo,
        js,
        voice: voice_passed,
    }
}

/// How many renderer DOM lanes the Electron half runs at once: five when it
/// has the machine to itself, and four beside the Rust half. Each lane is an
/// Electron, a GPU and a renderer process. Four beside the parallel Rust tests
/// used to fail a CLI theme lane's first capture (its focus ring caught in a
/// 0.01ms reduced-motion transition) and the hierarchy lanes' fixed slide
/// wait; both fixtures now wait for their animations to end.
const fn renderer_pool(rust: bool) -> &'static str {
    if rust { "4" } else { "5" }
}

/// The workspace tests: one compile, then the test executables in a pool with
/// Clippy (and doc tests, when any exist) beside them. Clippy takes Cargo's
/// build lock, which the executables never need, so it no longer runs before
/// them. Every failure is reported, not only the first.
fn run_rust_tests(root: &Path, smoke_pins: bool) -> Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // `--all-targets` is every target but doc tests. rustdoc's pass over each
    // library crate cost about 20 s of every gate with no doc test to run, so
    // they run only once a doc comment opens a code fence.
    let doctests = workspace_has_doctests(root);
    let binaries = timed_step("cargo test build", || {
        crate::test_runner::build(root, &cargo, !doctests)
    })?;
    thread::scope(|scope| {
        let lint = scope.spawn(|| {
            let clippy = timed_step("cargo clippy", || {
                run_cargo(
                    root,
                    [
                        "clippy",
                        "--locked",
                        "--workspace",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                    ],
                )
            });
            // `--quiet` puts libtest in its terse format, as for the
            // executables: one character per test rather than one line.
            let doc = if doctests {
                timed_step("cargo test --doc", || {
                    run_cargo(
                        root,
                        ["test", "--locked", "--workspace", "--doc", "--", "--quiet"],
                    )
                })
            } else {
                Ok(())
            };
            all_of(vec![clippy, doc])
        });
        let tests = timed_step("cargo test run", || {
            crate::test_runner::run(
                binaries,
                crate::test_runner::WORKERS,
                crate::gate::KNOWN_FLAKY,
            )
        });
        let pins = if smoke_pins {
            timed_step("smoke pins", || run_smoke_pins(root))
        } else {
            Ok(())
        };
        let lint = lint
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("cargo clippy panicked")));
        all_of(vec![tests, pins, lint])
    })
}

/// Succeeds when every result did; otherwise fails with each error in turn.
fn all_of(results: Vec<Result<()>>) -> Result<()> {
    let errors: Vec<String> = results
        .into_iter()
        .filter_map(|result| result.err().map(|error| format!("{error:#}")))
        .collect();
    if errors.is_empty() {
        return Ok(());
    }
    bail!("{}", errors.join("\n"))
}

/// Runs one step of the gate and prints how long it took, passed or failed, so
/// a slow gate says which step made it slow.
fn timed_step<T>(step: &str, run: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = Instant::now();
    let result = run();
    println!(
        "check: {}",
        step_line(step, result.is_ok(), started.elapsed())
    );
    result
}

/// Runs one half of the gate, printing and returning its own wall time.
fn timed_half(half: &str, run: impl FnOnce() -> Result<()>) -> HalfOutcome {
    let started = Instant::now();
    let result = run();
    let elapsed = started.elapsed();
    println!(
        "check: {}",
        step_line(&format!("{half} half"), result.is_ok(), elapsed)
    );
    HalfOutcome { result, elapsed }
}

/// `cargo clippy 12.5 s`, or `cargo clippy failed after 3.1 s`.
fn step_line(step: &str, passed: bool, elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64();
    if passed {
        format!("{step} {seconds:.1} s")
    } else {
        format!("{step} failed after {seconds:.1} s")
    }
}

/// The gate's deadline for `npm run check`: [`NPM_CHECK_TIMEOUT`] unless
/// [`NPM_CHECK_TIMEOUT_VARIABLE`] names another whole number of seconds.
fn npm_check_timeout() -> Result<Duration> {
    timeout_from(NPM_CHECK_TIMEOUT_VARIABLE, NPM_CHECK_TIMEOUT)
}

/// `default`, unless the environment `variable` names another whole number
/// of seconds.
pub fn timeout_from(variable: &str, default: Duration) -> Result<Duration> {
    let Some(value) = env::var_os(variable) else {
        return Ok(default);
    };
    let seconds = value
        .to_str()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .with_context(|| {
            format!(
                "{variable} must be a positive whole number of seconds, not {}",
                value.to_string_lossy()
            )
        })?;
    Ok(Duration::from_secs(seconds))
}

/// Runs npm, with `variables` added to its environment, and every line echoed
/// under [`NPM_PREFIX`] as it arrives. Past `timeout` it stops npm and
/// everything it started, giving that tree `grace` before killing it, and
/// fails with the last lines, as a failed smoke does. npm stays in this
/// process group, so ^C still reaches all of it.
fn run_npm(
    program: &OsStr,
    arguments: &[&str],
    directory: &Path,
    timeout: Duration,
    grace: Duration,
    variables: &[(&str, &str)],
) -> Result<()> {
    let mut child = Command::new(program)
        .current_dir(directory)
        .args(arguments)
        .envs(variables.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch npm")?;
    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(SMOKE_TAIL_LINES)));
    let keep = |tail: Arc<Mutex<VecDeque<String>>>| {
        move |line: &str| {
            if let Ok(mut tail) = tail.lock() {
                if tail.len() == SMOKE_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line.trim_end_matches(['\n', '\r']).to_owned());
            }
        }
    };
    let out = child.stdout.take().context("npm stdout is unavailable")?;
    let errors = child.stderr.take().context("npm stderr is unavailable")?;
    let readers = [
        thread::spawn({
            let keep = keep(Arc::clone(&tail));
            move || tee(out, true, NPM_PREFIX, keep)
        }),
        thread::spawn({
            let keep = keep(Arc::clone(&tail));
            move || tee(errors, false, NPM_PREFIX, keep)
        }),
    ];
    let deadline = Instant::now() + timeout;
    let mut exited: Option<(ExitStatus, Instant)> = None;
    loop {
        if exited.is_none() {
            exited = child
                .try_wait()
                .context("could not await npm")?
                .map(|status| (status, Instant::now()));
        }
        if let Some((status, at)) = exited {
            // npm's exit is the verdict. Its output is drained first unless a
            // process it left running holds the pipes open past the grace.
            let drained = readers.iter().all(thread::JoinHandle::is_finished);
            if drained || at.elapsed() >= grace {
                if drained {
                    for reader in readers {
                        reader
                            .join()
                            .map_err(|_| anyhow::anyhow!("the npm output reader panicked"))??;
                    }
                } else {
                    println!(
                        "check: npm exited; its output is still held open by a process it left running"
                    );
                }
                ensure!(status.success(), "npm failed with {status}");
                return Ok(());
            }
        } else if Instant::now() >= deadline {
            stop_tree(&mut child, grace);
            let lines = tail
                .lock()
                .map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            bail!(
                "npm run check did not finish within {}s and was stopped \
                 ({NPM_CHECK_TIMEOUT_VARIABLE} sets the limit in seconds); its last lines:\n{lines}",
                timeout.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// Stops a child and every process descending from it: TERM, `grace`, then
/// KILL for whatever of that tree is still there. The tree is read before the
/// first signal, because an exiting parent's children are re-parented and no
/// longer traceable to it.
pub fn stop_tree(child: &mut Child, grace: Duration) {
    let mut tree = process_table().map_or_else(
        || BTreeSet::from([child.id()]),
        |table| descendants(&table, child.id()),
    );
    signal(&tree, "-TERM");
    thread::sleep(grace);
    if let Some(table) = process_table() {
        if matches!(child.try_wait(), Ok(None)) {
            tree.extend(descendants(&table, child.id()));
        }
        let running: BTreeSet<u32> = table.iter().map(|&(pid, _)| pid).collect();
        tree.retain(|pid| running.contains(pid));
    }
    signal(&tree, "-KILL");
    let _ = child.wait();
}

/// Every `(pid, parent pid)` pair, or `None` when `ps` cannot say.
fn process_table() -> Option<Vec<(u32, u32)>> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(parse_process_table(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_process_table(listing: &str) -> Vec<(u32, u32)> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect()
}

/// `root` and every process below it in `table`.
fn descendants(table: &[(u32, u32)], root: u32) -> BTreeSet<u32> {
    let mut tree = BTreeSet::from([root]);
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, _) in table.iter().filter(|&&(_, ppid)| ppid == parent) {
            if tree.insert(pid) {
                frontier.push(pid);
            }
        }
    }
    tree
}

/// Sends one signal to a set of processes, ignoring those already gone.
fn signal(pids: &BTreeSet<u32>, signal: &str) {
    if pids.is_empty() {
        return;
    }
    let _ = Command::new("/bin/kill")
        .arg(signal)
        .args(pids.iter().map(u32::to_string))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn combine_check_results(rust: Result<()>, js: Result<()>) -> Result<()> {
    match (rust, js) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(rust), Err(js)) => {
            bail!("Rust check failed: {rust:#}\nJavaScript check failed: {js:#}")
        }
    }
}

/// Runs the one Js test that reads Rust sources. `smoke-pins.test.mjs` asserts
/// every literal the packaged smoke probes still exists verbatim in the file
/// that carries it, and three of those files are Rust — so a Rust-only gate
/// that skipped it could rename a wire id and only find out thirteen minutes
/// into the packaged smoke. It costs about a second.
fn run_smoke_pins(root: &Path) -> Result<()> {
    println!("check: rust-only run, so also running node --test scripts/smoke-pins.test.mjs");
    run_command_os(
        env::var_os("NODE").unwrap_or_else(|| "node".into()),
        [
            OsStr::new("--test"),
            OsStr::new("scripts/smoke-pins.test.mjs"),
        ],
        &root.join("electron"),
    )
}

/// Whether any doc comment opens a code fence, the only way a doc test exists.
/// An error answers yes, so doc tests are never skipped on a failed search.
fn workspace_has_doctests(root: &Path) -> bool {
    std::process::Command::new("git")
        .current_dir(root)
        .args(["grep", "-qE", r"^\s*(///|//!).*```", "--", "*.rs"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_or(true, |status| status.code() != Some(1))
}

/// The names a failing run reported, from either libtest output format: the
/// `test … FAILED` lines of the default one, and the summary block the terse
/// format prints instead. Without the second a `--quiet` executable would name
/// nothing, and the retry would refuse a known flake it can't see.
pub(crate) fn failed_test_names(output: &str) -> Vec<String> {
    let mut names = crate::gate::failed_tests(output);
    names.extend(terse_failure_names(output));
    names.sort();
    names.dedup();
    names
}

/// libtest ends a failing run with `failures:` and then the failed names, one
/// per line, indented four spaces. It prints that header twice: the first
/// introduces the captured output, whose entries are `---- name stdout ----`
/// and a blank line, so only the real list contributes names. Anything else
/// that happens to look like the block yields a name nobody knows as flaky,
/// which keeps the run fatal — the safe direction.
fn terse_failure_names(output: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut listing = false;
    for line in output.lines() {
        if line.trim_end() == "failures:" {
            listing = true;
            continue;
        }
        if !listing {
            continue;
        }
        match line.strip_prefix("    ").map(str::trim_end) {
            Some(name) if !name.is_empty() && !name.starts_with("---- ") => {
                names.push(name.to_owned());
            }
            _ => listing = false,
        }
    }
    names
}

fn run_cargo<const N: usize>(root: &Path, arguments: [&str; N]) -> Result<()> {
    run_cargo_args(root, &arguments)
}

fn run_cargo_args(root: &Path, arguments: &[&str]) -> Result<()> {
    run_command_os(
        env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        arguments.iter().map(OsStr::new),
        root,
    )
}

/// Runs a command with its output echoed as it arrives and kept for the
/// caller. Only the two retry decisions need the text; everything else keeps
/// streaming straight through `run_command_os`.
pub(crate) fn run_tee<I, S>(
    program: &OsStr,
    arguments: I,
    root: &Path,
) -> Result<(ExitStatus, String)>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = Command::new(program)
        .current_dir(root)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch {}", program.to_string_lossy()))?;
    let out = child.stdout.take().context("child stdout is unavailable")?;
    let errors = child.stderr.take().context("child stderr is unavailable")?;
    // Both pipes are drained concurrently: a child that fills one while this
    // process reads the other would deadlock.
    let reader = thread::spawn(move || {
        let mut captured = String::new();
        tee(errors, false, "", |line| captured.push_str(line)).map(|()| captured)
    });
    let mut captured = String::new();
    let out = tee(out, true, "", |line| captured.push_str(line));
    let Ok(errors) = reader.join() else {
        bail!("the child output reader panicked");
    };
    let status = child.wait().context("could not await the child")?;
    out?;
    Ok((status, format!("{captured}{}", errors?)))
}

/// Copies one child stream onto this process's own, a line at a time under
/// `prefix`, and hands `keep` a lossy copy of each line.
fn tee<R: Read>(
    stream: R,
    to_stdout: bool,
    prefix: &str,
    mut keep: impl FnMut(&str),
) -> Result<()> {
    let mut reader = io::BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        line.extend_from_slice(prefix.as_bytes());
        if reader
            .read_until(b'\n', &mut line)
            .context("could not read child output")?
            == 0
        {
            return Ok(());
        }
        // One write per line, so a line is never split by another writer.
        if to_stdout {
            let mut stdout = io::stdout().lock();
            stdout.write_all(&line).ok();
            stdout.flush().ok();
        } else {
            io::stderr().write_all(&line).ok();
        }
        keep(&String::from_utf8_lossy(&line[prefix.len()..]));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_fixtures::executable;

    #[test]
    fn gate_steps_print_their_own_time_passed_or_failed() {
        assert_eq!(
            step_line("cargo clippy", true, Duration::from_millis(12_540)),
            "cargo clippy 12.5 s"
        );
        assert_eq!(
            step_line("Rust half", false, Duration::from_millis(3_100)),
            "Rust half failed after 3.1 s"
        );
    }

    #[test]
    fn the_renderer_pool_widens_when_the_electron_half_runs_alone() {
        assert_eq!(renderer_pool(false), "5");
        assert_eq!(renderer_pool(true), "4");
    }

    #[test]
    fn every_failed_step_is_reported() {
        assert!(all_of(vec![Ok(()), Ok(())]).is_ok());
        let error = all_of(vec![
            Err(anyhow::anyhow!("cargo test failed")),
            Ok(()),
            Err(anyhow::anyhow!("cargo clippy failed")),
        ])
        .unwrap_err()
        .to_string();
        assert_eq!(error, "cargo test failed\ncargo clippy failed");
    }

    #[test]
    fn gate_reports_both_failures() {
        let result = combine_check_results(
            Err(anyhow::anyhow!("cargo fixture")),
            Err(anyhow::anyhow!("npm fixture")),
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("cargo fixture") && message.contains("npm fixture"));
        assert!(combine_check_results(Ok(()), Ok(())).is_ok());
        assert!(combine_check_results(Ok(()), Err(anyhow::anyhow!("npm"))).is_err());
    }

    #[test]
    fn descendants_follow_the_whole_process_tree() {
        let table = parse_process_table("   10     1\n 11 10\n12 11\n13 1\n14 12\nnot a row\n");
        assert_eq!(descendants(&table, 10), BTreeSet::from([10, 11, 12, 14]));
        assert_eq!(descendants(&table, 99), BTreeSet::from([99]));
    }

    #[test]
    fn the_npm_half_reports_its_own_exit() {
        let temp = tempfile::tempdir().expect("temp dir");
        let passing = temp.path().join("passing-npm");
        // It also proves the variables the gate adds reach npm.
        executable(
            &passing,
            b"#!/bin/sh\n[ \"$INTERFACE_RENDERER_POOL\" = 5 ] || exit 4\necho passed\necho warned >&2\n",
        );
        run_npm(
            passing.as_os_str(),
            &[],
            temp.path(),
            Duration::from_secs(120),
            NPM_STOP_GRACE,
            &[(RENDERER_POOL_VARIABLE, renderer_pool(false))],
        )
        .expect("a passing suite passes");
        let failing = temp.path().join("failing-npm");
        executable(&failing, b"#!/bin/sh\necho failed >&2\nexit 3\n");
        let error = run_npm(
            failing.as_os_str(),
            &[],
            temp.path(),
            Duration::from_secs(120),
            NPM_STOP_GRACE,
            &[],
        )
        .unwrap_err()
        .to_string();
        assert!(error.starts_with("npm failed with"), "{error}");
    }

    #[test]
    fn a_stalled_npm_half_is_stopped_with_its_last_lines() {
        let temp = tempfile::tempdir().expect("temp dir");
        let recorded = temp.path().join("grandchild.pid");
        let stalled = temp.path().join("stalled-npm");
        executable(
            &stalled,
            format!(
                "#!/bin/sh\necho first line\necho on stderr >&2\nsleep 600 &\necho $! > '{}'\n\
                 wait\n",
                recorded.display()
            )
            .as_bytes(),
        );
        let started = Instant::now();
        // A short grace: the tree's TERM-then-KILL is what is under test, not
        // the gate's five seconds.
        let error = run_npm(
            stalled.as_os_str(),
            &[],
            temp.path(),
            Duration::from_secs(2),
            Duration::from_millis(200),
            &[],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("did not finish within 2s"), "{error}");
        assert!(error.contains(NPM_CHECK_TIMEOUT_VARIABLE), "{error}");
        assert!(
            error.contains("first line") && error.contains("on stderr"),
            "{error}"
        );
        assert!(started.elapsed() < Duration::from_secs(60));
        // What npm started goes with it, not only npm.
        let grandchild = fs::read_to_string(&recorded).expect("the script recorded its child");
        let grandchild = grandchild.trim();
        let alive = || {
            Command::new("/bin/kill")
                .args(["-0", grandchild])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        assert!(!alive(), "pid {grandchild} outlived the stopped suite");
    }

    /// The gate runs `cargo test … -- --quiet`, whose terse format never prints
    /// a `test … FAILED` line — so the flaky-retry decision has to read the
    /// summary block instead, and must not read the captured-output block that
    /// carries the same header.
    #[test]
    fn terse_failures_are_named_from_the_summary_block() {
        // Joined rather than written as one literal: a `\`-continued string
        // eats the leading whitespace that is the whole point of these lines.
        let output = [
            "running 4 tests",
            "..F.",
            "failures:",
            "",
            "---- suite::beta stdout ----",
            "thread 'suite::beta' panicked at crates/x/src/lib.rs:9:5:",
            "failures:",
            "    not_a_test_name_from_captured_stdout",
            "",
            "failures:",
            "    suite::beta",
            "    src/lib.rs - alpha::doc (line 12)",
            "",
            "test result: FAILED. 3 passed; 1 failed; 0 ignored",
        ]
        .join("\n");
        let output = output.as_str();
        assert_eq!(
            failed_test_names(output),
            vec![
                "not_a_test_name_from_captured_stdout".to_owned(),
                "src/lib.rs - alpha::doc (line 12)".to_owned(),
                "suite::beta".to_owned(),
            ],
            "a name only the panic text mentions still counts, because an \
             unrecognized name keeps the run fatal"
        );
        assert!(failed_test_names("running 4 tests\n....\ntest result: ok.\n").is_empty());
        // The default format still parses, so a run without `--quiet` behaves.
        assert_eq!(
            failed_test_names("test suite::beta ... FAILED\nfailures:\n    suite::beta\n"),
            vec!["suite::beta".to_owned()]
        );
    }
}
