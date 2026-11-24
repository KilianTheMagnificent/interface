//! Pure gate decisions: which half of `cargo xtask check` a change requires.
//!
//! Scoping only ever skips work this module can prove redundant, so anything
//! unrecognized classifies as [`Scope::Both`] and the full gate runs. The
//! ordering of the rules is deliberate too: a Markdown file inside `electron/`
//! or `crates/` keeps that side's checks rather than being treated as docs.

use std::{
    collections::BTreeSet,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

/// Which half of the gate a changed path can affect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// Nothing the gate can check.
    None,
    /// The Electron/renderer check only.
    Js,
    /// The Rust steps only.
    Rust,
    /// Both halves.
    Both,
}

/// The halves of the gate one set of changed paths requires.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    /// Run formatting, Clippy, the voice helper build, and `cargo test`.
    pub rust: bool,
    /// Run `npm run check`.
    pub js: bool,
}

/// Paths that decide both halves. `electron/electron-builder.yml` is one of
/// them because a Rust test reads it: `package::tests::
/// every_expected_binary_is_a_packaged_resource` asserts every binary the
/// packager expects is still an `extraResources` entry.
const BOTH_FILES: [&str; 4] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "electron/electron-builder.yml",
];
/// Directories whose contents cross the language boundary.
const BOTH_PREFIXES: [&str; 2] = ["electron/src/shared/", "crates/interface-contracts/"];
/// The only files a release version bump may touch.
pub const VERSION_FILES: [&str; 2] = ["electron/package.json", "electron/package-lock.json"];
/// Repository documentation the gate cannot check.
const NONE_FILES: [&str; 2] = ["README.md", "AGENTS.md"];
/// Documentation and CI directories the gate cannot check.
const NONE_PREFIXES: [&str; 3] = [".github/", "docs/", "_AI/"];
/// Files a suite reads from outside its side of the tree, by path prefix and
/// suffix, with that reading suite: a change to one alone must still run the
/// test that asserts it. The fixtures are contracts both sides decode;
/// `protocol-docs.test.mjs` reads the host's protocol constants and
/// `PROTOCOL_FACTS.md`; Electron's tests execute the transports kept as `.mjs`
/// in Rust crates; an `engine-claude` test reads `rust-host-client.ts`, and
/// `xtask`'s routing test the navigation map; `package-parity.test.mjs` reads
/// how xtask packages the app, the only xtask file an Electron test reads;
/// `draft-store.test.ts` checks the renderer's collaboration engines against
/// the collaboration state's list; `contract-parity.test.mjs` reads every
/// contract source (also under `BOTH_PREFIXES`) and `protocol.rs`'s push list.
/// The host suite's outside agents test runs the hands-free kit's fake
/// providers (the `.sh` that starts the fake Claude `.mjs`).
/// `theme-look.test.ts` reads the example looks and their scene sources.
/// `smoke-packaged.mjs` and its test read the release channel the host
/// compiles in, to know whether a packaged updater must come up enabled.
/// `cross_language_inputs_select_the_suite_that_reads_them` keeps this whole.
const CROSS_LANGUAGE: [(&str, &str, Scope); 16] = [
    ("docs/fixtures/", "", Scope::Both),
    ("docs/theme-examples/", "", Scope::Js),
    ("docs/PROTOCOL_FACTS.md", "", Scope::Js),
    ("docs/agent-map.json", "", Scope::Rust),
    ("electron/src/fixtures/", "", Scope::Rust),
    ("electron/src/main/rust-host-client.ts", "", Scope::Rust),
    ("apps/interface-host/src/protocol.rs", "", Scope::Js),
    ("apps/interface-host/src/dispatcher.rs", "", Scope::Js),
    (
        "apps/interface-host/src/terminal_transport.rs",
        "",
        Scope::Js,
    ),
    ("crates/", ".mjs", Scope::Js),
    ("xtask/src/package.rs", "", Scope::Js),
    (
        "crates/interface-collaboration/src/state/mod.rs",
        "",
        Scope::Js,
    ),
    ("crates/interface-contracts/src/", ".rs", Scope::Js),
    (
        "crates/interface-settings/src/custom_themes.rs",
        "",
        Scope::Js,
    ),
    ("electron/scripts/handsfree-fake-", "", Scope::Rust),
    (
        "crates/interface-updater/release-channel.json",
        "",
        Scope::Js,
    ),
];

/// Classifies one repository-relative path: where it lives, widened by any
/// suite that reads it from the other side.
pub fn scope_of(path: &str) -> Scope {
    let path = path.trim_start_matches("./");
    CROSS_LANGUAGE
        .iter()
        .filter(|(prefix, suffix, _)| path.starts_with(prefix) && path.ends_with(suffix))
        .fold(location_scope(path), |scope, &(_, _, reader)| {
            widen(scope, reader)
        })
}

/// The quotes an Electron script's string literals use.
pub const SCRIPT_QUOTES: [char; 3] = ['"', '\'', '`'];

/// Whether the cross-language scan reads `path` as an Electron script.
pub fn is_electron_script(path: &str) -> bool {
    path.starts_with("electron/")
        && [".ts", ".tsx", ".mjs", ".cjs", ".js"]
            .iter()
            .any(|extension| path.ends_with(extension))
}

/// Every string between one kind of `quotes` on one line of `text`.
pub fn quoted_literals<'a>(text: &'a str, quotes: &[char]) -> Vec<&'a str> {
    let mut found = Vec::new();
    for line in text.lines() {
        for &quote in quotes {
            let mut rest = line;
            while let Some(at) = rest.find(quote) {
                let after = &rest[at + quote.len_utf8()..];
                let Some(end) = after.find(quote) else {
                    break;
                };
                found.push(&after[..end]);
                rest = &after[end + quote.len_utf8()..];
            }
        }
    }
    found
}

/// The literals in `text` that name a path relative to their file or crate:
/// `"../…"` or `"/../…"` between one kind of quote on one line.
pub fn relative_literals<'a>(text: &'a str, quotes: &[char]) -> Vec<&'a str> {
    quoted_literals(text, quotes)
        .into_iter()
        .filter(|literal| literal.starts_with("../") || literal.starts_with("/../"))
        .collect()
}

/// `literal` resolved against the repository directory `base`, or `None`
/// when it leaves the repository.
pub fn resolve(base: &str, literal: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in literal.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The files outside `electron/` the Electron script `path` names by a
/// relative literal. Anything it names inside `electron/` selects the
/// Electron suite already, so only these can be a read the cross-language
/// test rejects: the Rust suite's identity carries them.
pub fn script_reads_outside_electron(path: &str, text: &str) -> Vec<String> {
    let directory = Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    relative_literals(text, &SCRIPT_QUOTES)
        .into_iter()
        .filter_map(|literal| resolve(&directory, literal))
        .filter(|target| !target.starts_with("electron/"))
        .collect()
}

/// Where `smoke-pins.test.mjs` takes the repository files it reads from. It
/// is an Electron test that reads Rust sources named here by literal, so each
/// Rust file these name joins the Electron suite's receipt identity.
pub const SMOKE_PIN_SOURCES: [&str; 2] = [
    "electron/scripts/smoke-packaged.mjs",
    "electron/scripts/smoke-pins.test.mjs",
];

/// Whether a path is documentation by where it lives, whichever suite reads
/// it: a landing of only these is a record and needs no Dev refresh.
pub fn is_documentation(path: &str) -> bool {
    location_scope(path.trim_start_matches("./")) == Scope::None
}

/// The scope that runs everything either scope needs.
const fn widen(first: Scope, second: Scope) -> Scope {
    match (first, second) {
        (Scope::None, other) | (other, Scope::None) => other,
        (Scope::Js, Scope::Js) => Scope::Js,
        (Scope::Rust, Scope::Rust) => Scope::Rust,
        _ => Scope::Both,
    }
}

/// The scope where a path lives.
fn location_scope(path: &str) -> Scope {
    if BOTH_FILES.contains(&path) || BOTH_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
        return Scope::Both;
    }
    if path.starts_with("electron/") {
        return Scope::Js;
    }
    if path.starts_with("crates/") || path.starts_with("apps/") || path.starts_with("xtask/") {
        return Scope::Rust;
    }
    if NONE_FILES.contains(&path)
        || NONE_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
        || Path::new(path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        return Scope::None;
    }
    // An unclassified path is never a reason to skip a check.
    Scope::Both
}

/// The first path that forces both halves, so a `--changed` run that gates
/// everything always says which path made it do so — an untracked `.grok/` is
/// otherwise indistinguishable from the scoping having failed.
pub fn first_both<S: AsRef<str>>(paths: &[S]) -> Option<&str> {
    paths
        .iter()
        .map(AsRef::as_ref)
        .find(|path| scope_of(path) == Scope::Both)
}

/// Whether a gate running these halves must also run the renderer pin test.
/// `smoke-pins.test.mjs` belongs to the Js half but reads Rust sources, so a
/// Rust-only run is the one that would otherwise drop it.
pub const fn runs_smoke_pins(rust: bool, js: bool) -> bool {
    rust && !js
}

/// Aggregates the halves a whole change requires.
pub fn select<S: AsRef<str>>(paths: &[S]) -> Selection {
    let mut selection = Selection::default();
    for path in paths {
        match scope_of(path.as_ref()) {
            Scope::None => {}
            Scope::Js => selection.js = true,
            Scope::Rust => selection.rust = true,
            Scope::Both => {
                selection.rust = true;
                selection.js = true;
            }
        }
    }
    selection
}

/// The paths one `git status --porcelain` line reports. A rename yields both
/// its source and its destination: either side can decide the scope.
pub fn porcelain_paths(line: &str) -> Vec<String> {
    let Some(rest) = line.get(3..) else {
        return Vec::new();
    };
    rest.split(" -> ")
        .map(|path| unquote(path.trim()))
        .filter(|path| !path.is_empty())
        .collect()
}

/// Strips the quoting git applies to paths with unusual bytes. The result is
/// only ever prefix-matched, so an unresolved escape degrades to [`Scope::Both`].
fn unquote(path: &str) -> String {
    path.strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(path)
        .to_owned()
}

/// Runs the gate the flags select, printing why anything was skipped.
pub fn run_gate(root: &Path, changed: bool, base: &str, full: bool) -> Result<()> {
    if full || !changed {
        return run_full_gate(root);
    }
    crate::navigation::check(root)?;
    let Some(merge_base) = merge_base(root, base)? else {
        println!("check --changed: no merge base with {base}; running the full gate");
        return run_full_gate(root);
    };
    let paths = changed_paths(root, &merge_base)?;
    let selection = select(&paths);
    println!(
        "check --changed: base {}, {} paths → rust={} js={}",
        short(&merge_base),
        paths.len(),
        yes_no(selection.rust),
        yes_no(selection.js)
    );
    if let Some(path) = first_both(&paths) {
        println!("check --changed: {path} needs both halves");
    }
    if !selection.rust && !selection.js {
        println!("check --changed: navigation checked; no Rust/Electron suites required");
        return Ok(());
    }
    crate::gate_cache::run(root, selection, false).map(drop)
}

/// Explicit full checks always run both halves and leave source receipts.
pub fn run_full_gate(root: &Path) -> Result<()> {
    crate::navigation::check(root)?;
    crate::gate_cache::run(
        root,
        Selection {
            rust: true,
            js: true,
        },
        true,
    )
    .map(drop)
}

/// Reuse both suites only for a clean, pinned tree whose source and toolchain
/// identities match their receipts. Legacy tree-only stamps are not trusted.
pub fn gate_skip_reason(root: &Path, head: &str) -> Result<Option<String>> {
    if !tree_is_clean(root)? || rev_parse(root, "HEAD")? != head {
        return Ok(None);
    }
    let Ok(subject) = crate::gate_cache::subject(root) else {
        return Ok(None);
    };
    let covered = crate::gate_cache::covered(root, &subject)?;
    Ok((covered.rust && covered.js)
        .then(|| "source and toolchain receipts cover both suites".to_owned()))
}

/// The paths one commit range touches.
pub fn changed_names(root: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    comparable_names(root, from, to)?.with_context(|| format!("git cannot compare {from} and {to}"))
}

/// The same, but `None` when git cannot compare the two revisions at all.
fn comparable_names(root: &Path, from: &str, to: &str) -> Result<Option<Vec<String>>> {
    let output = git(root, &["diff", "--no-color", "--name-only", from, to])?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(output.stdout)
            .context("git printed non-UTF-8 path names")?
            .lines()
            .map(|name| unquote(name.trim()))
            .filter(|name| !name.is_empty())
            .collect(),
    ))
}

/// Whether two commits differ only by the Electron release version. Colour is
/// pinned off: a `color.ui = always` configuration would wrap every changed
/// line in escapes and the line filter would reject a real version bump.
pub fn version_only_diff(root: &Path, from: &str, to: &str) -> Result<bool> {
    // A revision git can no longer resolve — a stamped commit lost to `gc`, an
    // amended or rebased branch — proves nothing about this tree. That is "the
    // stamp does not cover it", which runs the gate; it is never a reason to
    // abort the run that was about to do the safe thing anyway.
    let Some(names) = comparable_names(root, from, to)? else {
        return Ok(false);
    };
    for name in &names {
        if !VERSION_FILES.contains(&name.as_str()) && name != crate::update_log::UPDATES_DOC {
            return Ok(false);
        }
    }
    if names.is_empty() {
        return Ok(true);
    }
    for name in names {
        if name == crate::update_log::UPDATES_DOC {
            continue;
        }
        let from_text = git_capture(root, &["show", &format!("{from}:{name}")])?;
        let to_text = git_capture(root, &["show", &format!("{to}:{name}")])?;
        let lock = name.ends_with("package-lock.json");
        if crate::dependencies::without_app_version(from_text.as_bytes(), lock)?
            != crate::dependencies::without_app_version(to_text.as_bytes(), lock)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Tests whose failure reruns their own test executable once instead of
/// failing the gate.
///
/// A name belongs here only with a known, understood failure mode that is
/// timing and not correctness. Everything else stays fatal on the first run.
/// Empty for now: the Claude kept-version probe test now bounds `--version`
/// by its own generous limit, and the child-host tests keep reading the
/// host's stdout until it exits, as the app does, instead of closing it.
pub const KNOWN_FLAKY: &[&str] = &[];

/// The test names one `cargo test` run reported as failures.
pub fn failed_tests(output: &str) -> Vec<String> {
    let mut names: Vec<String> = output
        .lines()
        .filter_map(|line| {
            let name = line
                .trim()
                .strip_prefix("test ")?
                .strip_suffix(" ... FAILED")?;
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether a failing test executable is retryable: at least one named
/// failure, and every one of them in `known` (the gate passes
/// [`KNOWN_FLAKY`]).
pub fn only_listed(known: &[&str], failed: &[String]) -> bool {
    !failed.is_empty() && failed.iter().all(|name| known.contains(&name.as_str()))
}

/// What a packaged smoke may fail with once before the failure is believed.
/// The smoke drives a real signed app: a loaded machine loses these races, a
/// broken build does not. Both are the exact wording `smoke-packaged.mjs`
/// fails with.
const SMOKE_RETRYABLE: [&str; 2] = [
    // Every `waitFor` bound in the probe, including the redraw waits, which
    // report a wait they could not finish in these words rather than as a
    // wrong value.
    "timed out",
    // The workspace round trip, whose three attempts each drive six real PTYs
    // through a workspace switch: attempt 3's redraw markers arrived after the
    // wait bound on a loaded machine and failed a promotion that was correct.
    "did not survive card-flip and workspace transitions",
];

/// Whether a failed packaged smoke is one of the known timing failures.
pub fn smoke_retryable(output: &str) -> bool {
    SMOKE_RETRYABLE
        .iter()
        .any(|signature| output.contains(signature))
}

/// Whether the working tree has no modified, staged, or untracked paths.
/// `status.showUntrackedFiles` is overridden: a repository configured to hide
/// untracked files must not be able to hide them from the gate.
pub fn tree_is_clean(root: &Path) -> Result<bool> {
    Ok(status_porcelain(root)?.trim().is_empty())
}

/// `git status --porcelain` with the untracked-file policy pinned.
pub fn status_porcelain(root: &Path) -> Result<String> {
    git_capture(root, &["status", "--porcelain", "--untracked-files=normal"])
}

/// Every path the working tree or the commits since `merge_base` touch.
fn changed_paths(root: &Path, merge_base: &str) -> Result<Vec<String>> {
    let mut paths: BTreeSet<String> = changed_names(root, merge_base, "HEAD")?
        .into_iter()
        .collect();
    for line in status_porcelain(root)?.lines() {
        paths.extend(porcelain_paths(line));
    }
    Ok(paths.into_iter().collect())
}

/// The merge base of `base` and `HEAD`, or `None` when git cannot find one.
fn merge_base(root: &Path, base: &str) -> Result<Option<String>> {
    let output = git(root, &["merge-base", base, "HEAD"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let base = String::from_utf8(output.stdout)
        .context("git printed a non-UTF-8 merge base")?
        .trim()
        .to_owned();
    Ok((!base.is_empty()).then_some(base))
}

/// Resolves one revision to its full object name.
pub fn rev_parse(root: &Path, revision: &str) -> Result<String> {
    git_capture(root, &["rev-parse", revision]).map(|value| value.trim().to_owned())
}

/// The seven-character prefix commit messages and logs use.
pub fn short(revision: &str) -> &str {
    revision.get(..7).unwrap_or(revision)
}

/// Renders a decision the way the printed gate summary spells it.
fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Where a git child's standard error goes.
#[derive(Clone, Copy, Debug)]
pub enum GitStderr {
    /// This process's own stderr, so git's reason for a failure shows.
    Inherit,
    /// Captured with the output, for a caller that reads it.
    Capture,
    /// Discarded: the exit status is the whole answer.
    Null,
}

/// An HTTPS transfer slower than this many bytes a second...
const GIT_LOW_SPEED_LIMIT: &str = "1000";
/// ...for this many seconds fails rather than waiting forever.
const GIT_LOW_SPEED_TIME: &str = "60";

/// The one way xtask starts git: in `root`, with no stdin, and with an HTTPS
/// fetch or push that stalls failing after a minute instead of hanging.
/// Closeout, promote and dev-restart run those while holding their locks. A
/// push that fails this way is an ambiguous outcome, which closeout and promote
/// already settle by reading the refs back.
pub fn git_command(root: &Path, arguments: &[&str], stderr: GitStderr) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(match stderr {
            GitStderr::Inherit => Stdio::inherit(),
            GitStderr::Capture => Stdio::piped(),
            GitStderr::Null => Stdio::null(),
        })
        .env("GIT_HTTP_LOW_SPEED_LIMIT", GIT_LOW_SPEED_LIMIT)
        .env("GIT_HTTP_LOW_SPEED_TIME", GIT_LOW_SPEED_TIME);
    command
}

/// Runs one git command, returning its raw output whether or not it succeeded.
pub fn git(root: &Path, arguments: &[&str]) -> Result<std::process::Output> {
    git_command(root, arguments, GitStderr::Inherit)
        .output()
        .context("failed to launch git")
}

/// Whether `ancestor` is reachable from `descendant`. `merge-base
/// --is-ancestor` exits 1 for "no"; any other failure (a revision `gc`
/// removed, a corrupt object) also answers "no", the safe direction for every
/// caller, while git's inherited stderr still says why.
pub fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    Ok(
        git(root, &["merge-base", "--is-ancestor", ancestor, descendant])?
            .status
            .success(),
    )
}

/// Runs one git command and requires it to succeed.
pub fn git_capture(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = git(root, arguments)?;
    if !output.status.success() {
        bail!("git {} failed with {}", arguments.join(" "), output.status);
    }
    String::from_utf8(output.stdout).context("git printed non-UTF-8 output")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stalled fetch or push under closeout, promote or dev-restart fails
    /// instead of holding their lock forever.
    #[test]
    fn every_git_child_gets_a_transfer_deadline() {
        let command = git_command(Path::new("/"), &["fetch", "origin"], GitStderr::Null);
        let environment: Vec<_> = command.get_envs().collect();
        for (name, value) in [
            ("GIT_HTTP_LOW_SPEED_LIMIT", "1000"),
            ("GIT_HTTP_LOW_SPEED_TIME", "60"),
        ] {
            assert!(
                environment.contains(&(name.as_ref(), Some(value.as_ref()))),
                "{name}"
            );
        }
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["fetch", "origin"]);
    }

    #[test]
    fn scope_of_classifies_the_documented_paths() {
        assert_eq!(scope_of("electron/src/shared/ipc.ts"), Scope::Both);
        assert_eq!(
            scope_of("crates/interface-contracts/src/lib.rs"),
            Scope::Both
        );
        assert_eq!(scope_of("Cargo.toml"), Scope::Both);
        assert_eq!(scope_of("Cargo.lock"), Scope::Both);
        assert_eq!(scope_of("rust-toolchain.toml"), Scope::Both);
        // xtask is Rust, but an Electron test reads how it packages the app.
        assert_eq!(scope_of("xtask/src/gate.rs"), Scope::Rust);
        assert_eq!(scope_of("xtask/Cargo.toml"), Scope::Rust);
        assert_eq!(scope_of("xtask/src/package.rs"), Scope::Both);
        // A Rust test reads the packaging manifest, so it is not Js-only.
        assert_eq!(scope_of("electron/electron-builder.yml"), Scope::Both);
        assert_eq!(scope_of("./electron/electron-builder.yml"), Scope::Both);
        assert_eq!(scope_of("electron/src/renderer/App.tsx"), Scope::Js);
        assert_eq!(scope_of("electron/package.json"), Scope::Js);
        assert_eq!(scope_of("crates/interface-runtime/src/lib.rs"), Scope::Rust);
        assert_eq!(scope_of("apps/interface-host/src/main.rs"), Scope::Rust);
        assert_eq!(scope_of(".github/workflows/gate.yml"), Scope::None);
        assert_eq!(scope_of("docs/CODEMAP.md"), Scope::None);
        assert_eq!(scope_of("README.md"), Scope::None);
        assert_eq!(scope_of("AGENTS.md"), Scope::None);
        assert_eq!(scope_of("_AI/AI_CONTEXT.md"), Scope::None);
        assert_eq!(scope_of("NOTES.md"), Scope::None);
        // Unknown paths never skip a check.
        assert_eq!(scope_of("release/latest-mac.yml"), Scope::Both);
        assert_eq!(scope_of("package.json"), Scope::Both);
        // Documentation inside a checked directory keeps that side's checks.
        assert_eq!(scope_of("electron/AGENTS.md"), Scope::Js);
        assert_eq!(scope_of("crates/AGENTS.md"), Scope::Rust);
        // A Cargo manifest below the root belongs to its crate.
        assert_eq!(scope_of("apps/iface/Cargo.toml"), Scope::Rust);
        // Inputs one suite reads from the other side of the tree.
        assert_eq!(
            scope_of("docs/fixtures/pane-presentation-v1.json"),
            Scope::Both
        );
        assert_eq!(
            scope_of("electron/src/fixtures/session-kind-contract.json"),
            Scope::Both
        );
        assert_eq!(scope_of("docs/PROTOCOL_FACTS.md"), Scope::Js);
        assert_eq!(scope_of("docs/agent-map.json"), Scope::Rust);
        assert_eq!(
            scope_of("electron/src/main/rust-host-client.ts"),
            Scope::Both
        );
        assert_eq!(scope_of("apps/interface-host/src/protocol.rs"), Scope::Both);
        assert_eq!(
            scope_of("apps/interface-host/src/terminal_transport.rs"),
            Scope::Both
        );
        assert_eq!(
            scope_of("crates/interface-terminal/src/opencode_activity_transport.mjs"),
            Scope::Both
        );
        assert_eq!(
            scope_of("crates/engine-acp/src/opencode_usage_transport.mjs"),
            Scope::Both
        );
        // Their neighbours keep their own side.
        assert_eq!(scope_of("docs/areas/xtask.md"), Scope::None);
        assert_eq!(scope_of("electron/src/main/main.ts"), Scope::Js);
        assert_eq!(scope_of("apps/interface-host/src/server.rs"), Scope::Rust);
        // Documentation by location stays documentation for landings.
        assert!(is_documentation("docs/fixtures/pane-presentation-v1.json"));
        assert!(is_documentation("./docs/PROTOCOL_FACTS.md"));
        assert!(!is_documentation(
            "electron/src/fixtures/terminal-data-v2.json"
        ));
        assert!(!is_documentation("unknown/path"));
    }

    #[test]
    fn cross_language_inputs_select_the_suite_that_reads_them() {
        // A Rust source names a file by a path relative to itself
        // (`include_str!`) or to its crate (`CARGO_MANIFEST_DIR`); a script or
        // test by one relative to its own URL. A tracked file named that way
        // must select the suite whose source names it, or a change to that
        // file alone skips the test it breaks.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root");
        let listing = git_capture(root, &["ls-files", "-z"]).expect("git lists the tree");
        let tracked: BTreeSet<&str> = listing
            .split('\0')
            .filter(|name| !name.is_empty())
            .collect();
        let crate_of = |source: &str| {
            Path::new(source)
                .ancestors()
                .skip(1)
                .map(|directory| directory.to_string_lossy().into_owned())
                .find(|directory| {
                    let manifest = if directory.is_empty() {
                        "Cargo.toml".to_owned()
                    } else {
                        format!("{directory}/Cargo.toml")
                    };
                    tracked.contains(manifest.as_str())
                })
                .unwrap_or_default()
        };
        let mut unselected = BTreeSet::new();
        let mut checked = 0_usize;
        for &source in &tracked {
            let rust = Path::new(source)
                .extension()
                .is_some_and(|extension| extension == "rs");
            let script = is_electron_script(source);
            if !rust && !script {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(root.join(source)) else {
                continue;
            };
            let directory = Path::new(source)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (quotes, bases): (&[char], Vec<String>) = if rust {
                (&['"'], vec![directory, crate_of(source)])
            } else {
                (&SCRIPT_QUOTES, vec![directory])
            };
            for literal in relative_literals(&text, quotes) {
                for base in &bases {
                    let Some(target) = resolve(base, literal) else {
                        continue;
                    };
                    if !tracked.contains(target.as_str()) {
                        continue;
                    }
                    checked += 1;
                    let selection = select(&[target.as_str()]);
                    if !(if rust { selection.rust } else { selection.js }) {
                        unselected.insert(format!("{target}, read by {source}"));
                    }
                }
            }
        }
        assert!(checked > 50, "the scan found only {checked} relative reads");
        assert!(
            unselected.is_empty(),
            "add these to gate::CROSS_LANGUAGE: {unselected:#?}"
        );
    }

    #[test]
    fn relative_literals_and_resolve_read_repository_paths() {
        let rust = "let a = include_str!(\"../../../electron/src/fixtures/a.json\");\n\
                    let b = concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../docs/x.md\");\n\
                    let c = \"./not-relative\";";
        assert_eq!(
            relative_literals(rust, &['"']),
            vec!["../../../electron/src/fixtures/a.json", "/../../docs/x.md"]
        );
        // Paths no file has, so the repository scan above never reads these.
        let js = "readFileSync(new URL('../../docs/facts.md', import.meta.url));";
        assert_eq!(
            relative_literals(js, &['"', '\'', '`']),
            vec!["../../docs/facts.md"]
        );
        assert_eq!(
            resolve(
                "crates/engine-acp/src",
                "../../../electron/src/fixtures/a.json"
            )
            .as_deref(),
            Some("electron/src/fixtures/a.json")
        );
        assert_eq!(
            resolve("crates/x", "/../../docs/x.md").as_deref(),
            Some("docs/x.md")
        );
        assert_eq!(resolve("electron", "../../outside"), None);
    }

    #[test]
    fn script_reads_name_only_files_outside_electron() {
        let script = "import { a } from '../shared/ipc';\n\
                      const b = readFileSync(new URL(\"../../../crates/x/src/y.rs\", import.meta.url));\n\
                      const c = new URL(`../../../docs/fixtures/z.json`, import.meta.url);\n\
                      const d = '../../../../../outside';\n";
        assert_eq!(
            script_reads_outside_electron("electron/src/main/a.test.ts", script),
            vec![
                "crates/x/src/y.rs".to_owned(),
                "docs/fixtures/z.json".to_owned()
            ]
        );
        assert!(is_electron_script("electron/scripts/check.mjs"));
        assert!(!is_electron_script("electron/package.json"));
        assert!(!is_electron_script("crates/x/src/transport.mjs"));
    }

    #[test]
    fn smoke_pin_sources_name_their_rust_files_by_literal() {
        // `gate_cache::source_identity` finds the Rust files the pin test reads
        // as literals in these sources; a pin naming its file any other way
        // would leave that file out of the Electron suite's identity.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root");
        let mut named = BTreeSet::new();
        for source in SMOKE_PIN_SOURCES {
            let text = std::fs::read_to_string(root.join(source)).expect("a pin source");
            named.extend(
                quoted_literals(&text, &SCRIPT_QUOTES)
                    .into_iter()
                    .filter(|literal| scope_of(literal) == Scope::Rust)
                    .map(str::to_owned),
            );
            for line in text.lines().filter(|line| line.contains(" file:")) {
                let value = line.split(" file:").nth(1).unwrap_or_default().trim_start();
                assert!(
                    value.starts_with(['"', '\'', '`']) || value.starts_with("XTERM_LIB"),
                    "{source}: a pin names its file by literal: {line}"
                );
            }
        }
        for rust in [
            "crates/interface-terminal/src/launch.rs",
            "apps/interface-host/src/antigravity_usage.rs",
            "apps/interface-host/src/orchestration.rs",
        ] {
            assert!(named.contains(rust), "{rust} is not named: {named:#?}");
        }
    }

    #[test]
    fn select_aggregates_a_path_list() {
        assert_eq!(
            select::<String>(&[]),
            Selection {
                rust: false,
                js: false
            }
        );
        assert_eq!(
            select(&["docs/CODEMAP.md", "README.md"]),
            Selection {
                rust: false,
                js: false
            }
        );
        assert_eq!(
            select(&["electron/src/renderer/App.tsx", "docs/CODEMAP.md"]),
            Selection {
                rust: false,
                js: true
            }
        );
        assert_eq!(
            select(&["crates/interface-runtime/src/lib.rs", "AGENTS.md"]),
            Selection {
                rust: true,
                js: false
            }
        );
        assert_eq!(
            select(&[
                "crates/interface-runtime/src/lib.rs",
                "electron/src/renderer/App.tsx"
            ]),
            Selection {
                rust: true,
                js: true
            }
        );
        assert_eq!(
            select(&["electron/src/shared/ipc.ts"]),
            Selection {
                rust: true,
                js: true
            }
        );
        assert_eq!(
            select(&["something/unknown"]),
            Selection {
                rust: true,
                js: true
            }
        );
    }

    #[test]
    fn first_both_names_the_path_that_forced_a_full_run() {
        assert_eq!(
            first_both(&["docs/CODEMAP.md", ".grok/", "electron/src/renderer/App.tsx"]),
            Some(".grok/"),
            "an untracked directory is unrecognized, so it gates everything"
        );
        assert_eq!(
            first_both(&["electron/electron-builder.yml"]),
            Some("electron/electron-builder.yml")
        );
        assert_eq!(
            first_both(&[
                "crates/interface-runtime/src/lib.rs",
                "electron/src/renderer/App.tsx"
            ]),
            None,
            "two single-sided paths select both halves without either forcing it"
        );
        assert_eq!(first_both::<String>(&[]), None);
    }

    #[test]
    fn a_rust_only_gate_still_runs_the_renderer_pins() {
        // The pin test lives in the Js half but reads Rust sources: dropping it
        // is what turns a renamed constant into a thirteen-minute failure.
        assert!(runs_smoke_pins(true, false));
        assert!(!runs_smoke_pins(true, true), "the Js half already runs it");
        assert!(!runs_smoke_pins(false, true));
        assert!(!runs_smoke_pins(false, false));
    }

    #[test]
    fn version_only_diff_treats_an_unresolvable_revision_as_uncovered() {
        // A stamp whose commit was garbage-collected must run the gate, not
        // abort the promotion. git prints its own "bad object" line here.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root");
        assert!(
            !version_only_diff(root, &"0".repeat(40), "HEAD")
                .expect("a missing commit is not an error"),
            "an uncomparable revision covers nothing"
        );
    }

    #[test]
    fn porcelain_paths_reads_both_sides_of_a_rename() {
        assert_eq!(
            porcelain_paths(" M electron/src/main.ts"),
            vec!["electron/src/main.ts".to_owned()]
        );
        assert_eq!(
            porcelain_paths("?? xtask/src/gate.rs"),
            vec!["xtask/src/gate.rs".to_owned()]
        );
        assert_eq!(
            porcelain_paths("A  crates/a/src/lib.rs"),
            vec!["crates/a/src/lib.rs".to_owned()]
        );
        assert_eq!(
            porcelain_paths("R  docs/old.md -> docs/new.md"),
            vec!["docs/old.md".to_owned(), "docs/new.md".to_owned()]
        );
        assert_eq!(
            porcelain_paths("?? \"electron/src/a b.ts\""),
            vec!["electron/src/a b.ts".to_owned()]
        );
        assert!(porcelain_paths("").is_empty());
        assert!(porcelain_paths("   ").is_empty());
    }

    #[test]
    fn failed_tests_reads_the_libtest_failure_lines() {
        let output = "running 3 tests\n\
                      test alpha ... ok\n\
                      test beta ... FAILED\n\
                      test suite::gamma ... FAILED\n\
                      test beta ... FAILED\n\
                      test delta ... ignored\n\
                      test src/lib.rs - alpha::doc (line 12) ... FAILED\n\
                      test suite::epsilon - should panic ... FAILED\n\
                      failures:\n    beta\n\
                      test result: FAILED. 1 passed; 2 failed; 1 ignored\n";
        assert_eq!(
            failed_tests(output),
            vec![
                "beta".to_owned(),
                "src/lib.rs - alpha::doc (line 12)".to_owned(),
                "suite::epsilon - should panic".to_owned(),
                "suite::gamma".to_owned()
            ]
        );
        assert!(
            !only_listed(KNOWN_FLAKY, &failed_tests(output)),
            "a doc-test or a should-panic failure is nobody's known flake"
        );
        assert!(failed_tests("test result: ok. 3 passed; 0 failed\n").is_empty());
        assert!(failed_tests("").is_empty());
    }

    #[test]
    fn only_known_flaky_gates_the_retry() {
        let known = ["a_timing_test"];
        let flaky = known[0].to_owned();
        assert!(only_listed(&known, &[flaky.clone()]));
        assert!(
            !only_listed(&known, &[]),
            "a failure with no named test is never retried"
        );
        assert!(
            !only_listed(
                &known,
                &[flaky, "packaging_publishes_the_manifest_last".to_owned()]
            ),
            "one unknown failure keeps the whole run fatal"
        );
        assert!(!only_listed(&known, &["something_else".to_owned()]));
        assert!(!only_listed(KNOWN_FLAKY, &["something_else".to_owned()]));
    }

    #[test]
    fn smoke_retryable_accepts_only_the_known_timing_failures() {
        assert!(smoke_retryable("Packaged renderer probe timed out."));
        assert!(smoke_retryable("Error: Packaged host bridge timed out."));
        assert!(
            smoke_retryable(
                "Error: Packaged terminal output did not survive card-flip and workspace \
                 transitions: {\"workspaceRoundTripCount\":2}"
            ),
            "the redraw markers arriving after the wait bound is a loaded machine"
        );
        assert!(!smoke_retryable(
            "Packaged renderer is missing the terminal pane."
        ));
        assert!(!smoke_retryable(
            "Packaged truncated-agent redraw did not fence shrink output before the final frame."
        ));
        assert!(!smoke_retryable(""));
    }

    #[test]
    fn short_and_yes_no_render_the_summary_line() {
        assert_eq!(short("7ef25074deadbeef"), "7ef2507");
        assert_eq!(short("abc"), "abc");
        assert_eq!(yes_no(true), "yes");
        assert_eq!(yes_no(false), "no");
    }
}
