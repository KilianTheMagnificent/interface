//! Checks are identified by source bytes, not by whether those bytes are committed.
//! Each suite has its own receipt, keyed by the inputs that suite reads plus the
//! tools that suite runs: a change to one side of the tree, or to one side's
//! tools, leaves the other side's receipt valid. What a suite reads comes from `gate::scope_of`, the same
//! classification `check --changed` selects suites by, widened by the two
//! reads that classification leaves to other rules (see [`source_identity`]).
//! The Swift helper's steps inside the Rust half have a receipt of their own,
//! keyed by the helper's package and the xtask code that builds and tests it,
//! so a Rust change that leaves those identical skips them.
use crate::{
    check_run, dependencies,
    gate::{self, Scope, Selection},
    pipeline_state,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub subject: String,
    pub suite: String,
    pub at: u64,
    pub elapsed_ms: u64,
    /// The suite's source digest and the tools it ran under, so a later miss
    /// on the same source can name the tool that changed. Receipts written
    /// before these were kept have neither.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub toolchain: Toolchain,
}

/// What each tool a suite runs answered when asked its version, by tool.
pub type Toolchain = BTreeMap<String, String>;

/// The tools only the Electron suite runs. Everything else (the Rust
/// compilers, Xcode's Swift and SDK, the build variables) belongs to the Rust
/// half and the Swift steps inside it. The Rust-only run's smoke-pin test is a
/// node run, but it only compares file contents, so node stays Electron's.
/// `installed` is the dependency tree in `electron/node_modules`, which the
/// lockfile in the source identity only describes (see [`installed`]).
const JS_TOOLS: [&str; 3] = ["node", "npm", "installed"];

/// Each suite's identity: the digest a receipt for that suite is keyed by.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subject {
    pub rust: String,
    pub js: String,
    /// The Swift helper's steps, which run within the Rust half.
    pub voice: String,
}

/// What the Swift helper's build and tests read: its package, and the xtask
/// sources that say how the gate builds and tests it.
const VOICE_INPUTS: [&str; 3] = [
    "apps/interface-voice/",
    "xtask/src/package.rs",
    "xtask/src/check_run.rs",
];

fn is_voice_input(name: &str) -> bool {
    VOICE_INPUTS.iter().any(|input| name.starts_with(input))
}

fn sensitive(path: &str) -> bool {
    path.split('/')
        .any(|part| interface_common::paths::is_sensitive_name(&part.to_ascii_lowercase()))
}

/// One checked input: its name, scope, executable bits and content digest.
struct Input {
    name: String,
    scope: Scope,
    mode: u32,
    digest: [u8; 32],
}

impl Input {
    fn hash_into(&self, hash: &mut Sha256) {
        hash.update(self.name.as_bytes());
        hash.update(b"\0");
        hash.update(self.mode.to_le_bytes());
        hash.update(self.digest);
    }
}

/// One gate input's executable bits and the bytes it is identified by, or
/// `None` when the listed file is gone from the working tree.
fn read_input(root: &Path, name: &str) -> Result<Option<(u32, Vec<u8>)>> {
    ensure!(
        !sensitive(name),
        "sensitive input prevents caching; no contents read"
    );
    let path = root.join(name);
    for parent in path
        .ancestors()
        .skip(1)
        .take_while(|parent| *parent != root)
    {
        if let Ok(meta) = fs::symlink_metadata(parent) {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "nonregular source parent prevents caching"
            );
        }
    }
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.is_file() && meta.nlink() == 1,
        "nonregular gate input prevents caching"
    );
    let mut bytes = Vec::new();
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?
        .read_to_end(&mut bytes)?;
    if gate::VERSION_FILES.contains(&name) {
        bytes =
            crate::dependencies::without_app_version(&bytes, name.ends_with("package-lock.json"))?;
    }
    Ok(Some((meta.mode() & 0o111, bytes)))
}

/// Each suite's source identity. The Rust suite hashes the Rust and shared
/// files, plus every file outside `electron/` an Electron script names by a
/// relative literal: those are what `gate`'s cross-language test (a Rust
/// test) reads from Electron sources. The Electron suite hashes the Electron
/// and shared files, plus every Rust file the smoke-pin sources name, which
/// `smoke-pins.test.mjs` (an Electron test) reads. No credential contents,
/// symlink targets or generated build output are read.
pub fn source_identity(root: &Path) -> Result<Subject> {
    let output = gate::git(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    ensure!(output.status.success(), "could not enumerate gate inputs");
    let names: BTreeSet<_> = std::str::from_utf8(&output.stdout)?
        .split('\0')
        .filter(|name| !name.is_empty())
        .collect();
    let mut inputs = Vec::new();
    let mut script_reads = BTreeSet::new();
    let mut pinned_names = BTreeSet::new();
    for name in names {
        let scope = gate::scope_of(name);
        if scope == Scope::None {
            continue;
        }
        let Some((mode, bytes)) = read_input(root, name)? else {
            continue;
        };
        if gate::is_electron_script(name)
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            for target in gate::script_reads_outside_electron(name, text) {
                script_reads.insert((name.to_owned(), target));
            }
            if gate::SMOKE_PIN_SOURCES.contains(&name) {
                pinned_names.extend(
                    gate::quoted_literals(text, &gate::SCRIPT_QUOTES)
                        .into_iter()
                        .map(str::to_owned),
                );
            }
        }
        inputs.push(Input {
            name: name.to_owned(),
            scope,
            mode,
            digest: Sha256::digest(&bytes).into(),
        });
    }
    let mut rust = Sha256::new();
    rust.update(b"interface-gate-source-v3\0rust\0");
    let mut js = Sha256::new();
    js.update(b"interface-gate-source-v3\0js\0");
    let mut voice = Sha256::new();
    voice.update(b"interface-gate-source-v3\0voice\0");
    for input in &inputs {
        if is_voice_input(&input.name) {
            input.hash_into(&mut voice);
        }
        if matches!(input.scope, Scope::Rust | Scope::Both) {
            input.hash_into(&mut rust);
        }
        let pinned = input.scope == Scope::Rust && pinned_names.contains(&input.name);
        if matches!(input.scope, Scope::Js | Scope::Both) || pinned {
            input.hash_into(&mut js);
        }
    }
    rust.update(b"\0script reads\0");
    for (script, target) in &script_reads {
        rust.update(script.as_bytes());
        rust.update(b"\0");
        rust.update(target.as_bytes());
        rust.update(b"\0");
    }
    Ok(Subject {
        rust: format!("{:x}", rust.finalize()),
        js: format!("{:x}", js.finalize()),
        voice: format!("{:x}", voice.finalize()),
    })
}

/// The compilers, runtimes and build settings the suites run under, as each
/// tool reports itself through this process's `PATH`.
fn toolchain(root: &Path) -> Result<Toolchain> {
    let mut tools = Toolchain::new();
    for (tool, program, args) in [
        ("rustc", "rustc", vec!["-Vv"]),
        ("cargo", "cargo", vec!["--version"]),
        ("node", "node", vec!["--version"]),
        ("npm", "npm", vec!["--version"]),
        ("swift", "xcrun", vec!["swift", "--version"]),
        (
            "sdk",
            "xcrun",
            vec!["--sdk", "macosx", "--show-sdk-version"],
        ),
    ] {
        let output = Command::new(program)
            .args(args)
            .current_dir(root)
            .output()
            .with_context(|| format!("could not identify {program}"))?;
        ensure!(output.status.success(), "could not identify {program}");
        tools.insert(
            tool.to_owned(),
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        );
    }
    for name in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        tools.insert(name.to_owned(), std::env::var(name).unwrap_or_default());
    }
    tools.insert("installed".to_owned(), installed(root)?);
    Ok(tools)
}

/// What `electron/node_modules` holds, as npm recorded it: the digest of
/// `node_modules/.package-lock.json` without the app's own version (as
/// [`dependencies::fingerprint`] reads the lockfile), or `none`. The source
/// identity carries only `package-lock.json`, so without this a pass under
/// an install from an older lockfile would be saved for the newer one. Two
/// `npm ci` runs from one lockfile write identical bytes here.
fn installed(root: &Path) -> Result<String> {
    let record = root.join("electron/node_modules/.package-lock.json");
    match fs::read(&record) {
        Ok(bytes) => dependencies::fingerprint(&bytes)
            .with_context(|| format!("could not read {}", record.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("none".to_owned()),
        Err(error) => Err(error).with_context(|| format!("could not read {}", record.display())),
    }
}

/// The part of `tools` one suite runs: the Electron tools, or all the rest.
fn suite_tools(tools: &Toolchain, js: bool) -> Toolchain {
    tools
        .iter()
        .filter(|(tool, _)| JS_TOOLS.contains(&tool.as_str()) == js)
        .map(|(tool, text)| (tool.clone(), text.clone()))
        .collect()
}

/// A suite's receipt key: its source digest and the tools it runs.
fn keyed(source: &str, tools: &Toolchain) -> String {
    let mut hash = Sha256::new();
    hash.update(b"interface-gate-subject-v2\0");
    hash.update(source);
    for (tool, text) in tools {
        hash.update(b"\0");
        hash.update(tool);
        hash.update(b"\0");
        hash.update(text);
    }
    format!("{:x}", hash.finalize())
}

/// Each suite's receipt key, from its source digest and the tools it runs.
/// The Swift steps run inside the Rust half, under its tools.
fn keyed_subject(source: &Subject, tools: &Toolchain) -> Subject {
    let rust_tools = suite_tools(tools, false);
    Subject {
        rust: keyed(&source.rust, &rust_tools),
        js: keyed(&source.js, &suite_tools(tools, true)),
        voice: keyed(&source.voice, &rust_tools),
    }
}

pub fn subject(root: &Path) -> Result<Subject> {
    Ok(keyed_subject(&source_identity(root)?, &toolchain(root)?))
}

/// Why `suite` missed although its newest receipt for this same `source` is
/// on disk: the tools whose answers changed since, first line each. `None`
/// when no receipt of this suite has this source, the ordinary miss after a
/// source change.
fn toolchain_miss(root: &Path, suite: &str, source: &str, tools: &Toolchain) -> Option<String> {
    let directory = pipeline_state::location(root).ok()?.join("checks");
    let suffix = format!("-{suite}.json");
    let newest = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
        .filter_map(|entry| {
            pipeline_state::read::<Receipt>(&entry.path())
                .ok()
                .flatten()
        })
        .filter(|receipt| receipt.suite == suite && receipt.source == source)
        .max_by_key(|receipt| receipt.at)?;
    let first_line = |text: Option<&String>| {
        text.and_then(|text| text.lines().next())
            .unwrap_or("(none)")
            .to_owned()
    };
    let changed = newest
        .toolchain
        .keys()
        .chain(tools.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|tool| newest.toolchain.get(*tool) != tools.get(*tool))
        .map(|tool| {
            format!(
                "{tool} {} -> {}",
                first_line(newest.toolchain.get(tool)),
                first_line(tools.get(tool))
            )
        })
        .collect::<Vec<_>>();
    (!changed.is_empty()).then(|| changed.join("; "))
}

fn receipt_path(root: &Path, subject: &str, suite: &str) -> Result<PathBuf> {
    let dir = pipeline_state::location(root)?.join("checks");
    Ok(dir.join(format!("{subject}-{suite}.json")))
}

/// Whether a receipt for `suite` matches `subject` exactly.
fn has_receipt(root: &Path, subject: &str, suite: &str) -> Result<bool> {
    let path = receipt_path(root, subject, suite)?;
    let record = pipeline_state::read::<Receipt>(&path).unwrap_or_else(|error| {
        eprintln!("check: unusable {suite} receipt ({error}); rerunning that suite");
        None
    });
    Ok(record.is_some_and(|record| record.subject == subject && record.suite == suite))
}

pub fn covered(root: &Path, subject: &Subject) -> Result<Selection> {
    Ok(Selection {
        rust: has_receipt(root, &subject.rust, "rust")?,
        js: has_receipt(root, &subject.js, "js")?,
    })
}

/// Runs the requested suites not already covered by receipts and returns the
/// ones that ran and passed in this checkout.
pub fn run(root: &Path, requested: Selection, force: bool) -> Result<Selection> {
    let identity = source_identity(root)
        .and_then(|source| Ok((source, toolchain(root)?)))
        .inspect_err(|error| println!("check: cache unavailable ({error}); running checks"))
        .ok();
    let before = identity
        .as_ref()
        .map(|(source, tools)| keyed_subject(source, tools));
    let saved = before
        .as_ref()
        .map(|subject| covered(root, subject))
        .transpose()?
        .unwrap_or_default();
    let needed = Selection {
        rust: requested.rust && (force || !saved.rust),
        js: requested.js && (force || !saved.js),
    };
    for (suite, key, js, wants, runs) in [
        ("Rust", "rust", false, requested.rust, needed.rust),
        ("Electron", "js", true, requested.js, needed.js),
    ] {
        if wants && !runs {
            println!("check: {suite} reused — identical {suite} inputs and toolchain");
        }
        if let Some((source, tools)) = identity.as_ref().filter(|_| runs && !force) {
            let source = if js { &source.js } else { &source.rust };
            if let Some(changed) = toolchain_miss(root, key, source, &suite_tools(tools, js)) {
                println!(
                    "check: {suite} inputs unchanged since their receipt; its tools changed: {changed}"
                );
            }
        }
    }
    if !needed.rust && !needed.js {
        return Ok(needed);
    }
    // The Swift steps run with the Rust half unless identical helper inputs
    // already passed them; an explicit full run always repeats them.
    let voice = force
        || match &before {
            Some(subject) => !has_receipt(root, &subject.voice, "voice")?,
            None => true,
        };
    let started = Instant::now();
    let checked = check_run::run_check_results(root, needed.rust, needed.js, voice);
    // Without an identity from before the run there is nothing to key a
    // receipt by: the verdict stands, and nothing is saved.
    if let Some(before) = before {
        ensure!(
            subject(root)? == before,
            "source or toolchain changed during checks; rerun on a stable tree"
        );
        let mut saved = false;
        // Each receipt records its own suite's time, so the half that binds the
        // gate is visible afterwards.
        let passed = |ran: bool, half: &check_run::HalfOutcome| {
            (ran && half.result.is_ok()).then_some(half.elapsed)
        };
        let (source, tools) = identity.as_ref().context("identity without a subject")?;
        for (suite, elapsed, subject, source, js) in [
            (
                "rust",
                passed(needed.rust, &checked.rust),
                &before.rust,
                &source.rust,
                false,
            ),
            (
                "js",
                passed(needed.js, &checked.js),
                &before.js,
                &source.js,
                true,
            ),
            ("voice", checked.voice, &before.voice, &source.voice, false),
        ] {
            if let Some(elapsed) = elapsed {
                let directory = pipeline_state::directory(root)?.join("checks");
                pipeline_state::private_directory(&directory)?;
                pipeline_state::write(
                    &receipt_path(root, subject, suite)?,
                    &Receipt {
                        subject: subject.clone(),
                        suite: suite.to_owned(),
                        at: pipeline_state::now(),
                        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                        source: source.clone(),
                        toolchain: suite_tools(tools, js),
                    },
                )?;
                saved = true;
            }
        }
        if saved {
            println!(
                "check: successful suite receipts saved ({:.1?})",
                started.elapsed()
            );
        }
    }
    check_run::combine_check_results(checked.rust.result, checked.js.result).map(|()| needed)
}

pub fn ensure_full(root: &Path) -> Result<Selection> {
    run(
        root,
        Selection {
            rust: true,
            js: true,
        },
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{git_in as git, init_repo};
    #[test]
    fn unusable_receipts_are_cache_misses_and_suites_are_independent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_repo(root);
        let checks = pipeline_state::directory(root).unwrap().join("checks");
        pipeline_state::private_directory(&checks).unwrap();
        let subject = Subject {
            rust: "a".repeat(64),
            js: "c".repeat(64),
            voice: "d".repeat(64),
        };
        fs::write(
            receipt_path(root, &subject.rust, "rust").unwrap(),
            "interrupted old receipt",
        )
        .unwrap();
        assert!(!covered(root, &subject).unwrap().rust);
        pipeline_state::write(
            &receipt_path(root, &subject.rust, "rust").unwrap(),
            &Receipt {
                subject: subject.rust.clone(),
                suite: "rust".into(),
                at: 1,
                elapsed_ms: 1,
                source: String::new(),
                toolchain: Toolchain::new(),
            },
        )
        .unwrap();
        let saved = covered(root, &subject).unwrap();
        assert!(saved.rust);
        assert!(!saved.js);
        let other = Subject {
            rust: "b".repeat(64),
            js: subject.js.clone(),
            voice: subject.voice.clone(),
        };
        assert!(!covered(root, &other).unwrap().rust);
        // A receipt keyed by the Rust identity never covers the Electron suite.
        let crossed = Subject {
            rust: subject.js.clone(),
            js: subject.rust.clone(),
            voice: subject.rust.clone(),
        };
        assert!(!covered(root, &crossed).unwrap().js);
        assert!(
            !has_receipt(root, &crossed.voice, "voice").unwrap(),
            "nor the Swift steps"
        );
    }
    #[test]
    fn each_suite_is_keyed_by_only_the_tools_it_runs() {
        let source = Subject {
            rust: "r".repeat(64),
            js: "j".repeat(64),
            voice: "v".repeat(64),
        };
        let tools = |node: &str, swift: &str| {
            Toolchain::from([
                ("rustc".to_owned(), "rustc 1.88.0".to_owned()),
                ("node".to_owned(), node.to_owned()),
                ("npm".to_owned(), "11.19.0".to_owned()),
                ("swift".to_owned(), swift.to_owned()),
                ("sdk".to_owned(), "27.0".to_owned()),
            ])
        };
        let base = keyed_subject(&source, &tools("v26.6.0", "Swift 6.3"));
        // Another node on PATH: only the Electron receipt misses.
        let node = keyed_subject(&source, &tools("v26.7.0", "Swift 6.3"));
        assert_eq!((node.rust == base.rust, node.js == base.js), (true, false));
        assert_eq!(node.voice, base.voice);
        // An Xcode update: the Rust half and its Swift steps miss, Electron not.
        let xcode = keyed_subject(&source, &tools("v26.6.0", "Swift 6.4"));
        assert_eq!(
            (xcode.rust == base.rust, xcode.js == base.js),
            (false, true)
        );
        assert_ne!(xcode.voice, base.voice);
    }

    #[test]
    fn the_electron_receipt_follows_what_npm_installed() {
        let temp = tempfile::tempdir().unwrap();
        let tree = |name: &str, installed_record: Option<&str>| {
            let root = temp.path().join(name);
            init_repo(&root);
            fs::create_dir_all(root.join("electron/node_modules")).unwrap();
            fs::write(root.join(".gitignore"), "node_modules\n").unwrap();
            fs::write(
                root.join("electron/package-lock.json"),
                r#"{"version":"2","packages":{"":{"version":"2"},"node_modules/x":{"version":"2"}}}"#,
            )
            .unwrap();
            fs::write(root.join("electron/app.ts"), "export const app = 1;").unwrap();
            if let Some(text) = installed_record {
                fs::write(root.join("electron/node_modules/.package-lock.json"), text).unwrap();
            }
            let tools = Toolchain::from([
                ("rustc".to_owned(), "rustc 1.88.0".to_owned()),
                ("node".to_owned(), "v26.6.0".to_owned()),
                ("installed".to_owned(), installed(&root).unwrap()),
            ]);
            (
                source_identity(&root).unwrap(),
                keyed_subject(&source_identity(&root).unwrap(), &tools),
            )
        };
        let (source, current) = tree(
            "current",
            Some(r#"{"version":"2","packages":{"node_modules/x":{"version":"2"}}}"#),
        );
        // npm ci from the old lockfile, never rerun after the bump.
        let (stale_source, stale) = tree(
            "stale",
            Some(r#"{"version":"1","packages":{"node_modules/x":{"version":"1"}}}"#),
        );
        assert_eq!(stale_source, source, "only node_modules differs");
        assert_ne!(stale.js, current.js);
        assert_eq!(
            (stale.rust, stale.voice),
            (current.rust.clone(), current.voice.clone())
        );
        // An app-version bump since the install leaves the install's key.
        let (_, bumped) = tree(
            "bumped",
            Some(r#"{"version":"1","packages":{"node_modules/x":{"version":"2"}}}"#),
        );
        assert_eq!(bumped.js, current.js);
        let (_, missing) = tree("missing", None);
        assert_ne!(missing.js, current.js);
        assert_eq!(missing.rust, current.rust);
        assert_eq!(installed(&temp.path().join("missing")).unwrap(), "none");
    }

    #[test]
    fn a_miss_on_unchanged_inputs_names_the_tool_that_changed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_repo(root);
        let checks = pipeline_state::directory(root).unwrap().join("checks");
        pipeline_state::private_directory(&checks).unwrap();
        let tools = |swift: &str| {
            Toolchain::from([
                ("rustc".to_owned(), "rustc 1.88.0\nhost: aarch64".to_owned()),
                ("swift".to_owned(), format!("{swift}\nTarget: arm64")),
            ])
        };
        let source = "s".repeat(64);
        for (at, swift) in [(1, "Apple Swift 6.2"), (2, "Apple Swift 6.3")] {
            let then = tools(swift);
            let subject = keyed(&source, &then);
            pipeline_state::write(
                &receipt_path(root, &subject, "rust").unwrap(),
                &Receipt {
                    subject,
                    suite: "rust".into(),
                    at,
                    elapsed_ms: 1,
                    source: source.clone(),
                    toolchain: then,
                },
            )
            .unwrap();
        }
        // Compared with the newest receipt for this source, first lines only.
        assert_eq!(
            toolchain_miss(root, "rust", &source, &tools("Apple Swift 6.4")).as_deref(),
            Some("swift Apple Swift 6.3 -> Apple Swift 6.4")
        );
        assert_eq!(
            toolchain_miss(root, "rust", &"t".repeat(64), &tools("Apple Swift 6.4")),
            None,
            "a source change is the ordinary miss"
        );
        assert_eq!(
            toolchain_miss(root, "js", &source, &tools("Apple Swift 6.4")),
            None
        );
    }

    #[test]
    fn dirty_bytes_keep_their_identity_after_commit_and_ignore_only_app_version() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_repo(root);
        fs::create_dir(root.join("electron")).unwrap();
        fs::write(
            root.join("electron/package.json"),
            r#"{"version":"1","scripts":{"check":"test"}}"#,
        )
        .unwrap();
        let before = source_identity(root).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "test"]);
        assert_eq!(source_identity(root).unwrap(), before);
        fs::write(
            root.join("electron/package.json"),
            r#"{"version":"2","scripts":{"check":"test"}}"#,
        )
        .unwrap();
        assert_eq!(source_identity(root).unwrap(), before);
        fs::write(root.join("electron/new.ts"), "new source").unwrap();
        assert_ne!(source_identity(root).unwrap().js, before.js);
    }

    #[test]
    fn each_suite_identity_follows_only_what_that_suite_reads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_repo(root);
        for (name, text) in [
            ("crates/a/src/lib.rs", "pub fn a() {}"),
            ("crates/a/src/pinned.rs", "pub fn pinned() {}"),
            ("electron/src/app.ts", "export const app = 1;"),
            ("electron/src/fixtures/f.json", "{}"),
            (
                "electron/scripts/smoke-pins.test.mjs",
                "read(\"crates/a/src/pinned.rs\");",
            ),
            ("xtask/src/main.rs", "fn main() {}"),
            ("xtask/src/package.rs", "fn package() { }"),
            ("docs/notes.md", "notes"),
        ] {
            fs::create_dir_all(root.join(name).parent().unwrap()).unwrap();
            fs::write(root.join(name), text).unwrap();
        }
        let mut last = source_identity(root).unwrap();
        let mut change = |name: &str, text: &str| {
            fs::write(root.join(name), text).unwrap();
            let next = source_identity(root).unwrap();
            let moved = (next.rust != last.rust, next.js != last.js);
            last = next;
            moved
        };
        // (Rust identity moved, Electron identity moved)
        assert_eq!(
            change("crates/a/src/lib.rs", "pub fn b() {}"),
            (true, false)
        );
        assert_eq!(
            change("electron/src/app.ts", "export const app = 2;"),
            (false, true)
        );
        assert_eq!(change("docs/notes.md", "more"), (false, false));
        // xtask is Rust; the one file an Electron test reads is both.
        assert_eq!(change("xtask/src/main.rs", "fn main() { }"), (true, false));
        assert_eq!(
            change("xtask/src/package.rs", "fn package() {}"),
            (true, true)
        );
        // Shared, and read across: both suites.
        assert_eq!(change("electron/src/fixtures/f.json", "[]"), (true, true));
        // A Rust file the smoke pins name is read by the Electron suite too.
        assert_eq!(
            change("crates/a/src/pinned.rs", "pub fn pinned2() {}"),
            (true, true)
        );
        // A script's relative read outside `electron/` is what the Rust
        // suite's cross-language test checks; one inside it is not.
        assert_eq!(
            change("electron/src/app.ts", "import { x } from \"../shared/x\";"),
            (false, true)
        );
        assert_eq!(
            change(
                "electron/src/app.ts",
                "const y = new URL(\"../../crates/a/src/lib.rs\", import.meta.url);"
            ),
            (true, true)
        );
    }
    #[test]
    fn the_voice_helper_identity_follows_only_its_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_repo(root);
        for (name, text) in [
            ("crates/a/src/lib.rs", "pub fn a() {}"),
            ("apps/interface-voice/Package.swift", "// package"),
            ("apps/interface-voice/Sources/v/main.swift", "print(1)"),
            ("xtask/src/package.rs", "fn build() {}"),
            ("xtask/src/gate.rs", "fn gate() {}"),
            ("electron/src/app.ts", "export const app = 1;"),
        ] {
            fs::create_dir_all(root.join(name).parent().unwrap()).unwrap();
            fs::write(root.join(name), text).unwrap();
        }
        let mut last = source_identity(root).unwrap();
        let mut change = |name: &str, text: &str| {
            fs::write(root.join(name), text).unwrap();
            let next = source_identity(root).unwrap();
            let moved = (next.rust != last.rust, next.voice != last.voice);
            last = next;
            moved
        };
        // (Rust identity moved, Swift steps' identity moved)
        assert_eq!(
            change("crates/a/src/lib.rs", "pub fn b() {}"),
            (true, false),
            "a Rust change alone reuses the Swift steps"
        );
        assert_eq!(change("xtask/src/gate.rs", "fn gate2() {}"), (true, false));
        assert_eq!(
            change("electron/src/app.ts", "export const app = 2;"),
            (false, false)
        );
        assert_eq!(
            change("apps/interface-voice/Sources/v/main.swift", "print(2)"),
            (true, true)
        );
        assert_eq!(
            change("xtask/src/package.rs", "fn build() { }"),
            (true, true),
            "how xtask builds the helper is one of its inputs"
        );
        fs::write(root.join("apps/interface-voice/Sources/v/new.swift"), "").unwrap();
        assert_ne!(source_identity(root).unwrap().voice, last.voice);
    }

    #[test]
    fn scratch_directories_are_not_gate_inputs() {
        // The owner's policy sends agents' scratch output to `work/`; a gate
        // transcript growing there must not move either identity mid-run, and
        // must not dirty the tree closeout requires clean.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root");
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_repo(root);
        fs::copy(workspace.join(".gitignore"), root.join(".gitignore")).unwrap();
        fs::create_dir_all(root.join("crates/a/src")).unwrap();
        fs::write(root.join("crates/a/src/lib.rs"), "pub fn a() {}").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "source"]);
        let before = source_identity(root).unwrap();
        for scratch in ["work/gate.txt", "output/shot.png"] {
            fs::create_dir_all(root.join(scratch).parent().unwrap()).unwrap();
            fs::write(root.join(scratch), "scratch").unwrap();
        }
        assert_eq!(source_identity(root).unwrap(), before);
        assert!(gate::tree_is_clean(root).unwrap());
    }

    #[test]
    fn secrets_and_links_cannot_be_cached() {
        assert!(sensitive(".env"));
        assert!(sensitive("a/service-account.json"));
        assert!(!sensitive(".env.example"));
        // The names navigation already refused, and the key stores and SSH
        // keys neither list caught before they shared one predicate.
        for secret in [
            "android/release.keystore",
            "keys/app.jks",
            "keys/putty.PPK",
            "home/id_ed25519",
            "home/ID_RSA.pub",
        ] {
            assert!(sensitive(secret), "{secret}");
        }
        assert!(!sensitive("crates/interface-common/src/paths.rs"));
        let temp = tempfile::tempdir().unwrap();
        init_repo(temp.path());
        std::os::unix::fs::symlink("/not/read", temp.path().join("link.rs")).unwrap();
        assert!(source_identity(temp.path()).is_err());
    }
}
