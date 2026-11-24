//! `cargo xtask prove --pin <path>::<test name> [--base <rev>]`: shows that a
//! fix is a fix. It runs the one pinned test on the base revision (default
//! `origin/dev`) and then on the candidate (this checkout, uncommitted
//! changes included), prints one line per side, and exits 0 only when the
//! base did not pass and the candidate did.
//!
//! The base runs in a kept detached worktree at
//! `~/.interface-pipeline/prove/worktree` with its own target directory beside
//! it (incremental compilation off, to keep it small); later runs reuse and
//! reset it. Never an agent worktree, never the main checkout. Prove writes
//! no gate receipt and never moves a branch or remote ref (it does not fetch;
//! the base is whatever the local ref names).
//!
//! The pure parts (pin to command, test listing and result parsing, the
//! outcome table) are unit tested below; the wrappers run git, cargo and
//! vitest.

use std::{
    env,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::{
    gate,
    pins::{self, Pin},
    pipeline_state, release,
};

/// Under `~`: the kept base checkout and its target directory.
const PROVE_DIRECTORY: &str = ".interface-pipeline/prove";
/// The kept base checkout, under [`PROVE_DIRECTORY`].
pub const PROVE_WORKTREE: &str = ".interface-pipeline/prove/worktree";

/// How the pinned test runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Runner {
    /// `cargo test -p <package> <target> <filter> -- …` in the checkout.
    Rust {
        package: String,
        target: Vec<String>,
        test: String,
    },
    /// `npx vitest run <file> -t <name>` in the checkout's `electron/`.
    Vitest { file: String, test: String },
}

/// What one side of the proof did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Passed,
    Failed,
    /// The test does not exist on that side.
    Absent,
    /// It could not be built or run, or the pin names more than one test.
    CouldNotRun(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Passed => formatter.write_str("passed"),
            Self::Failed => formatter.write_str("failed"),
            Self::Absent => formatter.write_str("absent"),
            Self::CouldNotRun(why) => write!(formatter, "could not run ({why})"),
        }
    }
}

/// The proof's verdict and its exit code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// The base did not pass and the candidate passed.
    Proven,
    CandidateFailed,
    BaseAlreadyPasses,
    CouldNotRun,
}

impl Verdict {
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Proven => 0,
            Self::CandidateFailed => 1,
            Self::BaseAlreadyPasses => 2,
            Self::CouldNotRun => 3,
        }
    }

    const fn summary(self) -> &'static str {
        match self {
            Self::Proven => {
                "proven: the test does not pass on the base and passes on the candidate"
            }
            Self::CandidateFailed => "not proven: the test does not pass on the candidate",
            Self::BaseAlreadyPasses => {
                "not proven: the test already passes on the base, so it does not show the fix"
            }
            Self::CouldNotRun => "not proven: one side could not run the test",
        }
    }
}

/// The outcome table.
#[must_use]
pub fn verdict(base: &Outcome, candidate: &Outcome) -> Verdict {
    match (base, candidate) {
        (Outcome::CouldNotRun(_), _) | (_, Outcome::CouldNotRun(_) | Outcome::Absent) => {
            Verdict::CouldNotRun
        }
        (_, Outcome::Failed) => Verdict::CandidateFailed,
        (Outcome::Passed, Outcome::Passed) => Verdict::BaseAlreadyPasses,
        (Outcome::Failed | Outcome::Absent, Outcome::Passed) => Verdict::Proven,
    }
}

/// How a pin runs in a checkout. `manifest` reads a repository-relative
/// `Cargo.toml` of that checkout (`None` when absent); `exists` says whether
/// a repository-relative file exists there.
///
/// # Errors
/// The pin names neither a Rust file inside a crate nor an Electron test.
pub fn runner_for(
    pin: &Pin,
    manifest: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&str) -> bool,
) -> Result<Runner, String> {
    let path = pin.path.as_str();
    let has_extension = |extensions: &[&str]| {
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
    };
    if has_extension(&["rs"]) {
        let (directory, package) = Path::new(path)
            .ancestors()
            .skip(1)
            .map(|directory| directory.to_string_lossy().into_owned())
            .find_map(|directory| {
                let file = if directory.is_empty() {
                    "Cargo.toml".to_owned()
                } else {
                    format!("{directory}/Cargo.toml")
                };
                manifest(&file)
                    .and_then(|text| package_name(&text))
                    .map(|package| (directory, package))
            })
            .ok_or_else(|| format!("no crate owns {path}"))?;
        let inside = path
            .strip_prefix(&directory)
            .unwrap_or(path)
            .trim_start_matches('/');
        let crate_file = |file: &str| {
            if directory.is_empty() {
                file.to_owned()
            } else {
                format!("{directory}/{file}")
            }
        };
        let target = if let Some(test) = inside.strip_prefix("tests/") {
            let name = test.split('/').next().unwrap_or(test);
            vec!["--test".to_owned(), name.trim_end_matches(".rs").to_owned()]
        } else if let Some(binary) = inside.strip_prefix("src/bin/") {
            let name = binary.split('/').next().unwrap_or(binary);
            vec!["--bin".to_owned(), name.trim_end_matches(".rs").to_owned()]
        } else if inside.starts_with("src/") && exists(&crate_file("src/lib.rs")) {
            vec!["--lib".to_owned()]
        } else if inside.starts_with("src/") {
            vec!["--bins".to_owned()]
        } else {
            return Err(format!(
                "{path} is not under its crate's src/ or tests/, so no test target owns it"
            ));
        };
        return Ok(Runner::Rust {
            package,
            target,
            test: pin.test.clone(),
        });
    }
    if let Some(file) = path.strip_prefix("electron/")
        && has_extension(&["ts", "tsx", "mts", "js", "mjs"])
    {
        return Ok(Runner::Vitest {
            file: file.to_owned(),
            test: pin.test.clone(),
        });
    }
    Err(format!(
        "{path} is neither a Rust source in a crate nor an Electron test file"
    ))
}

/// `[package] name` of a `Cargo.toml`, or `None` for a manifest with no
/// package (the workspace root).
#[must_use]
pub fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(value) = line.strip_prefix("name")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

/// The cargo arguments that list the tests whose name contains `test`.
#[must_use]
pub fn rust_list_arguments(package: &str, target: &[String], test: &str) -> Vec<String> {
    let mut arguments = vec!["test".to_owned(), "-p".to_owned(), package.to_owned()];
    arguments.extend(target.iter().cloned());
    arguments.extend([test.to_owned(), "--".to_owned(), "--list".to_owned()]);
    arguments
}

/// The cargo arguments that run exactly the test `full_name`.
#[must_use]
pub fn rust_run_arguments(package: &str, target: &[String], full_name: &str) -> Vec<String> {
    let mut arguments = vec!["test".to_owned(), "-p".to_owned(), package.to_owned()];
    arguments.extend(target.iter().cloned());
    arguments.extend([full_name.to_owned(), "--".to_owned(), "--exact".to_owned()]);
    arguments
}

/// The one listed test whose last path segment is `test`: `Ok(None)` when
/// none is, an error naming them when several are.
///
/// # Errors
/// More than one listed test has that name.
pub fn listed_test(listing: &str, test: &str) -> Result<Option<String>, String> {
    let matches: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .filter(|name| name.rsplit("::").next() == Some(test))
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some((*one).to_owned())),
        several => Err(format!(
            "{test} names {} tests: {}",
            several.len(),
            several.join(", ")
        )),
    }
}

/// What a `cargo test … --exact` run of one test says: its `test result:`
/// lines summed.
#[must_use]
pub fn rust_outcome(output: &str) -> Outcome {
    let (mut passed, mut failed, mut seen) = (0_u32, 0_u32, false);
    for line in output.lines() {
        let Some(rest) = line.trim().strip_prefix("test result: ") else {
            continue;
        };
        seen = true;
        for part in rest.split(';') {
            let mut words = part.split_whitespace().rev();
            let (Some(label), Some(count)) = (words.next(), words.next()) else {
                continue;
            };
            let count: u32 = count.parse().unwrap_or(0);
            match label {
                "passed" => passed += count,
                "failed" => failed += count,
                _ => {}
            }
        }
    }
    match (seen, passed, failed) {
        (false, _, _) => Outcome::CouldNotRun("it did not build or run".to_owned()),
        (true, 1, 0) => Outcome::Passed,
        (true, 0, 1) => Outcome::Failed,
        (true, passed, failed) => Outcome::CouldNotRun(format!(
            "{passed} passed and {failed} failed where exactly one test was to run"
        )),
    }
}

/// Vitest's `-t` is a pattern: the title, every special character escaped.
#[must_use]
pub fn vitest_pattern(test: &str) -> String {
    let mut pattern = String::with_capacity(test.len());
    for character in test.chars() {
        if r"\^$.|?*+()[]{}/".contains(character) {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

/// What vitest's JSON report says about the test titled exactly `test`.
#[must_use]
pub fn vitest_outcome(report: &Value, test: &str) -> Outcome {
    let results: Vec<&str> = report["testResults"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|file| file["assertionResults"].as_array().into_iter().flatten())
        .filter(|result| result["title"].as_str() == Some(test))
        .filter_map(|result| result["status"].as_str())
        .filter(|status| matches!(*status, "passed" | "failed"))
        .collect();
    match results.as_slice() {
        [] => Outcome::Absent,
        ["passed"] => Outcome::Passed,
        ["failed"] => Outcome::Failed,
        several => Outcome::CouldNotRun(format!("{} tests are titled {test:?}", several.len())),
    }
}

/// `cargo xtask prove`.
///
/// # Errors
/// The pin is malformed, the base revision is unknown, or the base
/// checkout could not be prepared.
pub fn prove(root: &Path, pin: &str, base: &str) -> Result<i32> {
    let pin = pins::parse_pin(pin).map_err(anyhow::Error::msg)?;
    let base_commit = gate::rev_parse(root, &format!("{base}^{{commit}}"))
        .with_context(|| format!("{base} is not a revision here"))?;
    let directory = pipeline_state::home()?.join(PROVE_DIRECTORY);
    pipeline_state::private_directory(directory.parent().context("prove's parent")?)?;
    release::with_pipeline_lock(&directory, ".prove.lock", || {
        let worktree = prepare_base(root, &directory, &base_commit)?;
        let target = directory.join("target");
        let base_outcome = run_side(&worktree, &pin, Some(&target));
        println!(
            "base {base} ({}): {base_outcome}",
            gate::short(&base_commit)
        );
        let candidate_outcome = run_side(root, &pin, None);
        println!("candidate (this checkout): {candidate_outcome}");
        let verdict = verdict(&base_outcome, &candidate_outcome);
        println!("{}", verdict.summary());
        Ok(verdict.exit_code())
    })
}

/// The kept base checkout, created or reset to `commit`.
fn prepare_base(root: &Path, directory: &Path, commit: &str) -> Result<PathBuf> {
    let worktree = directory.join("worktree");
    let canonical_root = root.canonicalize().context("the checkout")?;
    if worktree.starts_with(&canonical_root) || canonical_root.starts_with(&worktree) {
        bail!("prove's base checkout must lie outside this checkout");
    }
    if worktree.exists() {
        let own = gate::git_capture(root, &["rev-parse", "--git-common-dir"])?;
        let theirs = gate::git_capture(&worktree, &["rev-parse", "--git-common-dir"])
            .with_context(|| {
                format!(
                    "{} exists but is not a checkout of this repository; move it away",
                    worktree.display()
                )
            })?;
        let same =
            root.join(own.trim()).canonicalize()? == worktree.join(theirs.trim()).canonicalize()?;
        if !same {
            bail!(
                "{} is a checkout of another repository; move it away",
                worktree.display()
            );
        }
        gate::git_capture(
            &worktree,
            &["checkout", "--quiet", "--detach", "--force", commit],
        )?;
        gate::git_capture(&worktree, &["clean", "-ffdq"])?;
    } else {
        let path = worktree.to_string_lossy().into_owned();
        gate::git_capture(
            root,
            &["worktree", "add", "--quiet", "--detach", &path, commit],
        )?;
    }
    Ok(worktree)
}

/// Runs the pin in `checkout`; `target` is its own cargo target directory.
fn run_side(checkout: &Path, pin: &Pin, target: Option<&Path>) -> Outcome {
    let runner = runner_for(
        pin,
        |file| std::fs::read_to_string(checkout.join(file)).ok(),
        |file| checkout.join(file).is_file(),
    );
    match runner {
        Err(why) => {
            if checkout.join(&pin.path).exists() {
                Outcome::CouldNotRun(why)
            } else {
                Outcome::Absent
            }
        }
        Ok(Runner::Rust {
            package,
            target: selector,
            test,
        }) => run_rust(checkout, &package, &selector, &test, target),
        Ok(Runner::Vitest { file, test }) => run_vitest(checkout, &file, &test),
    }
}

fn cargo(checkout: &Path, arguments: &[String], target: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    command
        .current_dir(checkout)
        .args(arguments)
        .stdin(Stdio::null());
    if let Some(target) = target {
        command
            .env("CARGO_TARGET_DIR", target)
            .env("CARGO_INCREMENTAL", "0");
    }
    let output = command
        .output()
        .map_err(|error| format!("cargo did not start: {error}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

fn run_rust(
    checkout: &Path,
    package: &str,
    selector: &[String],
    test: &str,
    target: Option<&Path>,
) -> Outcome {
    let listing = match cargo(
        checkout,
        &rust_list_arguments(package, selector, test),
        target,
    ) {
        Ok(listing) => listing,
        Err(why) => return Outcome::CouldNotRun(why),
    };
    let full_name = match listed_test(&listing, test) {
        Ok(Some(name)) => name,
        Ok(None) if listing.contains(": test") || listing.contains("0 tests") => {
            return Outcome::Absent;
        }
        Ok(None) => return Outcome::CouldNotRun(tail(&listing)),
        Err(why) => return Outcome::CouldNotRun(why),
    };
    match cargo(
        checkout,
        &rust_run_arguments(package, selector, &full_name),
        target,
    ) {
        Ok(output) => match rust_outcome(&output) {
            Outcome::CouldNotRun(why) => Outcome::CouldNotRun(format!("{why}: {}", tail(&output))),
            outcome => outcome,
        },
        Err(why) => Outcome::CouldNotRun(why),
    }
}

fn run_vitest(checkout: &Path, file: &str, test: &str) -> Outcome {
    let electron = checkout.join("electron");
    if !electron.join(file).is_file() {
        return Outcome::Absent;
    }
    if !electron.join("node_modules").is_dir() {
        println!(
            "prove: installing Electron dependencies in {}",
            electron.display()
        );
        let installed = Command::new("npm")
            .current_dir(&electron)
            .args(["ci", "--no-audit", "--no-fund"])
            .stdin(Stdio::null())
            .status();
        if !installed.is_ok_and(|status| status.success()) {
            return Outcome::CouldNotRun("npm ci failed".to_owned());
        }
    }
    let report = match tempfile::NamedTempFile::new() {
        Ok(report) => report,
        Err(error) => return Outcome::CouldNotRun(error.to_string()),
    };
    let output_file = format!("--outputFile={}", report.path().display());
    let ran = Command::new("npx")
        .current_dir(&electron)
        .args(["vitest", "run", file, "-t", &vitest_pattern(test)])
        .args(["--reporter=json", &output_file])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let stderr = match ran {
        Ok(output) => String::from_utf8_lossy(&output.stderr).into_owned(),
        Err(error) => return Outcome::CouldNotRun(format!("npx did not start: {error}")),
    };
    match std::fs::read_to_string(report.path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    {
        Some(report) => vitest_outcome(&report, test),
        None => Outcome::CouldNotRun(format!("vitest wrote no report: {}", tail(&stderr))),
    }
}

/// The last lines of a command's output, for a could-not-run line.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" | ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn pin(path: &str, test: &str) -> Pin {
        Pin {
            path: path.to_owned(),
            test: test.to_owned(),
        }
    }

    fn manifests(file: &str) -> Option<String> {
        match file {
            "Cargo.toml" => Some("[workspace]\nmembers = []\n".to_owned()),
            "apps/interface-host/Cargo.toml" => {
                Some("[package]\nname = \"interface-host\"\n".to_owned())
            }
            "xtask/Cargo.toml" => Some("[package]\nname = \"xtask\"\npublish = false\n".to_owned()),
            "crates/provider-replay/Cargo.toml" => Some(
                "[package]\nname = \"provider-replay\"\n\n[[bin]]\nname = \"provider-replay\"\n"
                    .to_owned(),
            ),
            _ => None,
        }
    }

    fn exists(file: &str) -> bool {
        matches!(
            file,
            "apps/interface-host/src/lib.rs" | "crates/provider-replay/src/lib.rs"
        )
    }

    #[test]
    fn pins_map_to_the_one_test_target_that_owns_them() {
        let runner = |path: &str| runner_for(&pin(path, "t"), manifests, exists);
        assert_eq!(
            runner("apps/interface-host/tests/suite/provider_replay.rs"),
            Ok(Runner::Rust {
                package: "interface-host".into(),
                target: vec!["--test".into(), "suite".into()],
                test: "t".into(),
            })
        );
        assert_eq!(
            runner("crates/provider-replay/tests/replay_cli.rs"),
            Ok(Runner::Rust {
                package: "provider-replay".into(),
                target: vec!["--test".into(), "replay_cli".into()],
                test: "t".into(),
            })
        );
        assert_eq!(
            runner("apps/interface-host/src/threads/run_tap.rs"),
            Ok(Runner::Rust {
                package: "interface-host".into(),
                target: vec!["--lib".into()],
                test: "t".into(),
            })
        );
        assert_eq!(
            runner("xtask/src/pins.rs"),
            Ok(Runner::Rust {
                package: "xtask".into(),
                target: vec!["--bins".into()],
                test: "t".into(),
            })
        );
        assert_eq!(
            runner("crates/provider-replay/src/bin/provider-replay.rs"),
            Ok(Runner::Rust {
                package: "provider-replay".into(),
                target: vec!["--bin".into(), "provider-replay".into()],
                test: "t".into(),
            })
        );
        assert_eq!(
            runner("electron/src/renderer/browser-point.test.ts"),
            Ok(Runner::Vitest {
                file: "src/renderer/browser-point.test.ts".into(),
                test: "t".into(),
            })
        );
        assert!(runner("docs/TODO.md").is_err());
        assert!(runner("crates/unknown/src/lib.rs").is_err());
    }

    #[test]
    fn rust_commands_list_then_run_exactly_one_test() {
        let target = vec!["--test".to_owned(), "suite".to_owned()];
        assert_eq!(
            rust_list_arguments("interface-host", &target, "claude_simple").join(" "),
            "test -p interface-host --test suite claude_simple -- --list"
        );
        assert_eq!(
            rust_run_arguments("interface-host", &target, "provider_replay::claude_simple")
                .join(" "),
            "test -p interface-host --test suite provider_replay::claude_simple -- --exact"
        );
        let listing = "provider_replay::claude_simple: test\n\
                       provider_replay::claude_simple_more: test\n\
                       other::claude_simple: bench\n\n2 tests, 0 benchmarks\n";
        assert_eq!(
            listed_test(listing, "claude_simple"),
            Ok(Some("provider_replay::claude_simple".to_owned()))
        );
        assert_eq!(listed_test("0 tests, 0 benchmarks\n", "x"), Ok(None));
        let twice = "a::x: test\nb::x: test\n";
        assert!(
            listed_test(twice, "x")
                .unwrap_err()
                .contains("names 2 tests")
        );
    }

    #[test]
    fn a_run_is_judged_by_its_test_result_lines() {
        assert_eq!(
            rust_outcome(
                "running 1 test\ntest a::x ... ok\n\n\
                 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.3s\n"
            ),
            Outcome::Passed
        );
        assert_eq!(
            rust_outcome(
                "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n\
                 test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out\n"
            ),
            Outcome::Failed
        );
        assert!(matches!(
            rust_outcome("error[E0425]: cannot find value `x`\nerror: could not compile"),
            Outcome::CouldNotRun(_)
        ));
        assert!(matches!(
            rust_outcome("test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured"),
            Outcome::CouldNotRun(_)
        ));
    }

    #[test]
    fn vitest_titles_match_exactly_and_patterns_are_escaped() {
        assert_eq!(
            vitest_pattern("adds (a) note? [x] $1.5"),
            r"adds \(a\) note\? \[x\] \$1\.5"
        );
        let report = json!({"testResults": [{"assertionResults": [
            {"title": "adds a note", "status": "passed"},
            {"title": "adds a note twice", "status": "failed"},
            {"title": "other", "status": "skipped"}
        ]}]});
        assert_eq!(vitest_outcome(&report, "adds a note"), Outcome::Passed);
        assert_eq!(
            vitest_outcome(&report, "adds a note twice"),
            Outcome::Failed
        );
        assert_eq!(vitest_outcome(&report, "other"), Outcome::Absent);
        assert_eq!(vitest_outcome(&report, "missing"), Outcome::Absent);
    }

    #[test]
    fn the_outcome_table_proves_only_a_fix() {
        use Outcome::{Absent, Failed, Passed};
        let cannot = Outcome::CouldNotRun("x".into());
        let cases = [
            (Failed, Passed, Verdict::Proven, 0),
            (Absent, Passed, Verdict::Proven, 0),
            (Passed, Passed, Verdict::BaseAlreadyPasses, 2),
            (Failed, Failed, Verdict::CandidateFailed, 1),
            (Absent, Failed, Verdict::CandidateFailed, 1),
            (Passed, Failed, Verdict::CandidateFailed, 1),
            (Failed, Absent, Verdict::CouldNotRun, 3),
            (cannot.clone(), Passed, Verdict::CouldNotRun, 3),
            (Failed, cannot, Verdict::CouldNotRun, 3),
        ];
        for (base, candidate, expected, code) in cases {
            let found = verdict(&base, &candidate);
            assert_eq!(found, expected, "{base} / {candidate}");
            assert_eq!(found.exit_code(), code);
        }
        let codes: std::collections::BTreeSet<i32> = [
            Verdict::Proven,
            Verdict::CandidateFailed,
            Verdict::BaseAlreadyPasses,
            Verdict::CouldNotRun,
        ]
        .iter()
        .map(|verdict| verdict.exit_code())
        .collect();
        assert_eq!(codes.len(), 4, "every verdict has its own exit code");
    }

    #[test]
    fn package_names_come_from_the_package_table_only() {
        assert_eq!(
            package_name("[package]\nname = \"xtask\"\n[dependencies]\nname = \"no\"\n"),
            Some("xtask".to_owned())
        );
        assert_eq!(package_name("[workspace]\nmembers = [\"a\"]\n"), None);
    }
}
