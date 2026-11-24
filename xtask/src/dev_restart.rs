//! `cargo xtask dev-restart`: the AGENTS.md Dev-app restart, as one command.
//!
//! The recipe it replaces is four shell lines — pull the fixed checkout,
//! `npm ci` when the lockfile moved, quit the app, relaunch the supervisor
//! detached — and every one of them was re-judged by a permission classifier
//! and typed again by the next agent. Two failures it now rules out: the
//! relaunch inherited a login `PATH` without `~/.cargo/bin`, which killed a
//! restart on a bare `spawn cargo ENOENT`, and a `nohup … &` that "succeeded"
//! while the supervisor died seconds later off-screen. This command puts
//! `~/.cargo/bin` on the child's `PATH` and then reads the log it wrote to
//! decide whether the app actually came up.
//!
//! Only `Interface Dev` is ever touched. The production install is never
//! quit, launched, or named here.
//!
//! A refresh never takes the screen from the owner. Its window opens hidden
//! and appears the first time the owner switches to Interface Dev, unless Dev
//! was the app in front, which this command reports as `INTERFACE_DEV_QUIET=0`
//! (see `electron/src/main/dev-readiness.ts`, `devLaunchStaysHidden`).

use std::{
    env,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{ErrorKind, Read as _, Seek as _, SeekFrom},
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

/// The fixed from-source checkout, deliberately outside the main repository,
/// relative to the home directory. `worktree-prune` protects it by this name.
pub(crate) const DEV_CHECKOUT: &str = "Developer/.interface-dev";
/// Where the detached supervisor's output is appended.
const DEV_LOG: &str = "Library/Logs/interface-dev.log";
/// A log past this size is set aside (as `<log>.1`) before the next launch, so
/// the pair stays bounded instead of growing for as long as Dev is used.
const DEV_LOG_ROTATE_BYTES: u64 = 32 * 1024 * 1024;
/// The toolchain directory a login `PATH` is missing.
const CARGO_BIN: &str = ".cargo/bin";
/// How the supervisor pattern reads in `ps`, and so to `pgrep -f`.
const SUPERVISOR_PATTERN: &str = "node scripts/dev.mjs";
/// How long the whole build-and-launch may take before this reports a timeout.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the log is re-read while waiting.
const POLL: Duration = Duration::from_millis(500);
/// How long a signalled supervisor gets to exit before the new one starts.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the quit Dev app and its Rust host get to finish their own
/// shutdown. Electron gives the host ten seconds, then SIGTERM, then SIGKILL;
/// a host still alive after this is stuck in its shutdown and holding the
/// profile's locks, which is exactly what makes the next launch die.
const QUIT_TIMEOUT: Duration = Duration::from_secs(20);
/// How the Dev app's Electron reads in `ps`, and so to `pgrep -f`. Its
/// arguments name the checkout; the executable itself is the shared bundle.
const DEV_APP_PATTERN: &str = "Interface Dev.app/Contents/MacOS/Electron";
/// The Rust host, relative to the checkout. Matched on the EXECUTABLE, not the
/// command line: every Codex and Claude pane the host launches carries the host
/// path in its own arguments (the notify hook), and a substring match over
/// `pgrep -f` counted those CLIs as hosts — and would have killed them.
const DEV_HOST_BINARY: &str = "target/debug/interface-host";
/// The Dev bundle's identifier (`DEV_APP_BUNDLE_ID` in `electron/scripts/dev-app.mjs`).
const DEV_BUNDLE_ID: &str = "com.andreromero.interface.dev";

/// What the supervisor's log says about this launch, once it says anything.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// The Dev app was handed to Electron.
    Ready(String),
    /// The supervisor gave up; the line is the one that said so.
    Failed(String),
}

const READY_MARKER: &str = "INTERFACE_DEV_READY ";
#[derive(serde::Deserialize)]
struct Ready {
    launch: String,
    revision: String,
    pid: u32,
}
const FAILURE_MARKERS: [&str; 8] = [
    "spawn cargo ENOENT",
    "failed to start Rust host build",
    "Rust host build failed",
    "is not executable anywhere on PATH",
    "error: could not compile",
    "failed to start Electron",
    "INTERFACE_DEV_FAILED",
    "startup failed",
];

/// Only an end-to-end response belonging to this launch settles readiness.
/// A failure anywhere in the new log tail takes precedence over success.
pub fn log_verdict(text: &str, launch: &str, revision: &str) -> Option<Verdict> {
    if let Some(line) = text.lines().find(|line| {
        FAILURE_MARKERS.iter().any(|marker| line.contains(marker))
            || line.trim_start().starts_with("error[E")
    }) {
        return Some(Verdict::Failed(line.trim().to_owned()));
    }
    text.lines().rev().find_map(|line| {
        let ready: Ready = serde_json::from_str(line.strip_prefix(READY_MARKER)?).ok()?;
        (ready.launch == launch && ready.revision == revision && ready.pid > 0)
            .then(|| Verdict::Ready(line.to_owned()))
    })
}

/// Updates the fixed checkout and relaunches the Dev app on it.
pub fn dev_restart(expected: Option<&str>) -> Result<()> {
    let checkout = crate::pipeline_state::home()?.join(DEV_CHECKOUT);
    let state = crate::pipeline_state::directory(&checkout)?;
    crate::release::with_pipeline_lock(&state, ".dev-refresh.lock", || restart_locked(expected))
}

fn restart_locked(expected: Option<&str>) -> Result<()> {
    let home = crate::pipeline_state::home()?;
    let checkout = home.join(DEV_CHECKOUT);
    ensure!(
        checkout.join(".git").exists(),
        "{} is not a checkout; the owner recreates it with `git -C ~/Developer/\
         Interface-Rust worktree add ~/Developer/.interface-dev dev`",
        checkout.display()
    );
    let electron = checkout.join("electron");
    ensure!(
        crate::gate::tree_is_clean(&checkout)?,
        "Dev checkout has local changes; refusing to refresh"
    );
    crate::gate::git_capture(&checkout, &["fetch", "origin", "dev"])?;
    let revision = crate::gate::rev_parse(&checkout, "origin/dev")?;
    if let Some(expected) = expected {
        ensure!(
            crate::gate::is_ancestor(&checkout, expected, &revision)?,
            "Dev tip does not contain the requested landing {expected}"
        );
    }
    crate::gate::git_capture(&checkout, &["merge", "--ff-only", &revision])?;
    crate::dependencies::ensure_installed(&checkout)?;

    // Read before the quit: once Dev is gone, another app is in front.
    let quiet = !dev_app_is_frontmost();
    // Quit, then stop the supervisor (its SIGTERM is a graceful quit too now),
    // then WAIT for the app and its host to be gone. Starting the next
    // supervisor while the old host still held the profile's locks made the
    // new host exit 1 and the window close on "startup failed".
    quit_dev_app();
    stop_supervisors(&checkout)?;
    wait_for_dev_app_exit(&checkout)?;

    let log = home.join(DEV_LOG);
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    rotate_log(&log, DEV_LOG_ROTATE_BYTES)?;
    let offset = fs::metadata(&log).map_or(0, |metadata| metadata.len());
    let launch = format!("{}-{}", std::process::id(), crate::pipeline_state::now());
    let mut child = spawn_supervisor(&home, &electron, &log, &launch, &revision, quiet)?;
    println!(
        "dev-restart: supervisor starting (npm pid {}); waiting up to {}s for the Dev app",
        child.id(),
        START_TIMEOUT.as_secs()
    );
    if quiet {
        println!(
            "dev-restart: Interface Dev was not in front; its window stays hidden until you switch to it"
        );
    }

    match wait_for_launch(&log, offset, &mut child, &launch, &revision)? {
        Verdict::Ready(line) => {
            let pid = supervisor_pids(&checkout)?
                .first()
                .copied()
                .unwrap_or_else(|| child.id());
            ensure!(
                crate::gate::rev_parse(&checkout, "HEAD")? == revision
                    && crate::gate::tree_is_clean(&checkout)?,
                "Dev sources changed during startup; readiness cannot be attributed to {revision}"
            );
            let ready: Ready = serde_json::from_str(
                line.strip_prefix(READY_MARKER)
                    .context("missing readiness marker")?,
            )?;
            let state = crate::pipeline_state::directory(&checkout)?.join("dev.json");
            crate::pipeline_state::write(
                &state,
                &serde_json::json!({
                    "revision": revision, "pid": ready.pid, "launch": launch,
                    "verifiedAt": crate::pipeline_state::now(), "checkout": checkout,
                }),
            )?;
            println!(
                "dev-restart: verified renderer → preload → Rust host on {}",
                crate::gate::short(&revision)
            );
            println!("dev-restart: supervisor pid {pid}, log {}", log.display());
            Ok(())
        }
        Verdict::Failed(line) => {
            bail!(
                "dev-restart: the supervisor failed: {line} (see {})",
                log.display()
            )
        }
    }
}

/// Polls the log this launch appended to until it settles or the deadline
/// passes. A supervisor that exits before saying anything is a failure too.
fn wait_for_launch(
    log: &Path,
    offset: u64,
    child: &mut Child,
    launch: &str,
    revision: &str,
) -> Result<Verdict> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let tail = read_from(log, offset)?;
        if let Some(verdict) = log_verdict(&tail, launch, revision) {
            return Ok(verdict);
        }
        if let Some(status) = child.try_wait().context("could not poll npm run dev")? {
            return Ok(Verdict::Failed(format!(
                "npm run dev exited with {status} before the Dev app launched"
            )));
        }
        if Instant::now() >= deadline {
            bail!(
                "dev-restart: the Dev app did not launch within {}s; the supervisor is still \
                 running, see {}",
                START_TIMEOUT.as_secs(),
                log.display()
            );
        }
        thread::sleep(POLL);
    }
}

/// Starts `npm run dev` in its own process group so it outlives this command,
/// with the toolchain on `PATH` and its output appended to the shared log.
fn spawn_supervisor(
    home: &Path,
    electron: &Path,
    log: &Path,
    launch: &str,
    revision: &str,
    quiet: bool,
) -> Result<Child> {
    let out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("could not open {}", log.display()))?;
    let err = out
        .try_clone()
        .context("could not duplicate the log handle")?;
    Command::new("npm")
        .arg("--prefix")
        .arg(electron)
        .args(["run", "dev"])
        .current_dir(electron)
        .env("PATH", dev_path(home))
        .env("INTERFACE_DEV_LAUNCH", launch)
        .env("INTERFACE_DEV_REVISION", revision)
        .env("INTERFACE_DEV_QUIET", if quiet { "1" } else { "0" })
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .process_group(0)
        .spawn()
        .context("failed to launch npm run dev")
}

/// `~/.cargo/bin` ahead of the inherited `PATH`. A login shell's `PATH` does
/// not carry it, and the supervisor shells out to cargo.
fn dev_path(home: &Path) -> OsString {
    let mut path = OsString::from(home.join(CARGO_BIN));
    if let Some(inherited) = env::var_os("PATH") {
        path.push(":");
        path.push(inherited);
    }
    path
}

/// Waits for the quit Dev app and its Rust host to exit, and kills whatever is
/// still there once the deadline passes — a host that outlives Electron's own
/// SIGKILL fallback is stuck in its shutdown, and it is holding the locks the
/// next host needs. Only this checkout's processes are considered.
fn wait_for_dev_app_exit(checkout: &Path) -> Result<()> {
    let deadline = Instant::now() + QUIT_TIMEOUT;
    loop {
        let electron = dev_processes(checkout, DEV_APP_PATTERN)?;
        let hosts = dev_hosts(checkout)?;
        if electron.is_empty() && hosts.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            for pid in electron.iter().chain(&hosts) {
                println!(
                    "dev-restart: pid {pid} is still running {}s after quit; killing it",
                    QUIT_TIMEOUT.as_secs()
                );
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .stdin(Stdio::null())
                    .status();
            }
            return Ok(());
        }
        thread::sleep(POLL);
    }
}

/// Pids of this checkout's processes whose command line contains `pattern`:
/// `pgrep -fl` lists every match with its arguments, and only lines naming the
/// checkout count, so the installed Interface and any other checkout's Dev app
/// are never touched.
fn dev_processes(checkout: &Path, pattern: &str) -> Result<Vec<u32>> {
    let output = Command::new("pgrep")
        .args(["-fl", pattern])
        .stdin(Stdio::null())
        .output()
        .context("failed to launch pgrep")?;
    Ok(dev_process_pids(
        &String::from_utf8_lossy(&output.stdout),
        &checkout.to_string_lossy(),
    ))
}

/// The pids in `pgrep -fl` output whose command line names `checkout`.
fn dev_process_pids(listing: &str, checkout: &str) -> Vec<u32> {
    listing
        .lines()
        .filter(|line| line.contains(checkout))
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect()
}

/// Pids whose executable IS this checkout's Rust host. `ps -o comm` is the
/// executable path alone, so a CLI whose arguments mention the host does not
/// match.
fn dev_hosts(checkout: &Path) -> Result<Vec<u32>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,comm="])
        .stdin(Stdio::null())
        .output()
        .context("failed to launch ps")?;
    let binary = checkout.join(DEV_HOST_BINARY);
    Ok(host_pids(
        &String::from_utf8_lossy(&output.stdout),
        &binary.to_string_lossy(),
    ))
}

/// The pids in `ps -axo pid=,comm=` output whose executable is exactly `binary`.
fn host_pids(listing: &str, binary: &str) -> Vec<u32> {
    listing
        .lines()
        .filter_map(|line| {
            let (pid, comm) = line.trim_start().split_once(char::is_whitespace)?;
            (comm.trim() == binary).then(|| pid.parse().ok()).flatten()
        })
        .collect()
}

/// Quits the Dev app. "Not running" is the expected case, never an error.
/// Whether Interface Dev is the app the owner is using right now. Anything
/// unreadable counts as not in front, so the relaunch stays out of sight.
fn dev_app_is_frontmost() -> bool {
    let Ok(front) = Command::new("lsappinfo").arg("front").output() else {
        return false;
    };
    let asn = String::from_utf8_lossy(&front.stdout).trim().to_owned();
    if !front.status.success() || asn.is_empty() {
        return false;
    }
    Command::new("lsappinfo")
        .args(["info", "-only", "bundleid", &asn])
        .output()
        .is_ok_and(|info| {
            info.status.success() && names_dev_bundle(&String::from_utf8_lossy(&info.stdout))
        })
}

/// `lsappinfo info -only bundleid` names the Dev bundle, not production's
/// `com.andreromero.interface`, which is a prefix of it.
fn names_dev_bundle(info: &str) -> bool {
    info.contains(&format!("\"{DEV_BUNDLE_ID}\""))
}

fn quit_dev_app() {
    let quit = Command::new("osascript")
        .args(["-e", "quit app \"Interface Dev\""])
        .stdin(Stdio::null())
        .output();
    match quit {
        Ok(output) if output.status.success() => println!("dev-restart: quit Interface Dev"),
        Ok(output) => println!(
            "dev-restart: Interface Dev was not quit ({})",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => println!("dev-restart: could not run osascript ({error})"),
    }
}

/// SIGTERMs every supervisor still working inside the fixed checkout, so the
/// new one does not race an old one for the dev port and the dev profile.
fn stop_supervisors(checkout: &Path) -> Result<()> {
    let pids = supervisor_pids(checkout)?;
    if pids.is_empty() {
        return Ok(());
    }
    for pid in &pids {
        println!("dev-restart: stopping supervisor pid {pid}");
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdin(Stdio::null())
            .status();
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if supervisor_pids(checkout)?.is_empty() {
            return Ok(());
        }
        thread::sleep(POLL);
    }
    println!("dev-restart: a previous supervisor is still shutting down; continuing");
    Ok(())
}

/// The `node scripts/dev.mjs` processes whose working directory is inside the
/// fixed checkout. Matching on the working directory keeps a supervisor
/// running from some other checkout out of this command's way.
fn supervisor_pids(checkout: &Path) -> Result<Vec<u32>> {
    let output = Command::new("pgrep")
        .args(["-f", SUPERVISOR_PATTERN])
        .stdin(Stdio::null())
        .output()
        .context("failed to launch pgrep")?;
    // pgrep exits 1 when nothing matches, which is not a failure here.
    let mut pids = Vec::new();
    for pid in String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
    {
        if process_cwd(pid).is_some_and(|cwd| cwd.starts_with(checkout)) {
            pids.push(pid);
        }
    }
    Ok(pids)
}

/// One process's working directory, or `None` when lsof cannot report it.
fn process_cwd(pid: u32) -> Option<PathBuf> {
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-F", "n"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(|path| PathBuf::from(path.trim()))
}

/// Renames `log` to `<log>.1`, replacing an older one, once it is larger than
/// `limit`. Called with every earlier supervisor stopped, so nothing appends to
/// the renamed file; a reader following it keeps the old inode.
fn rotate_log(log: &Path, limit: u64) -> Result<()> {
    let size = match fs::metadata(log) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", log.display()));
        }
    };
    if size <= limit {
        return Ok(());
    }
    let mut previous = log.as_os_str().to_owned();
    previous.push(".1");
    let previous = PathBuf::from(previous);
    fs::rename(log, &previous)
        .with_context(|| format!("could not move {} to {}", log.display(), previous.display()))
}

/// Everything appended to `log` since `offset`, treating a log that does not
/// exist yet as empty.
fn read_from(log: &Path, offset: u64) -> Result<String> {
    let mut file = match fs::File::open(log) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", log.display()));
        }
    };
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("could not seek {}", log.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("could not read {}", log.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_process_pids_keep_only_this_checkouts_processes() {
        let listing = "97216 /Users/a/Desktop/Projects/.interface-dev/target/debug/interface-host\n\
                       33387 /Applications/Interface.app/Contents/Resources/interface-host\n\
                       76458 /Applications/Interface Dev.app/Contents/MacOS/Electron \
                       /Users/a/Desktop/Projects/.interface-dev/electron --user-data-dir=x\n\
                       12 /private/tmp/other/target/debug/interface-host\n";
        assert_eq!(
            dev_process_pids(listing, "/Users/a/Desktop/Projects/.interface-dev"),
            vec![97216, 76458]
        );
        assert_eq!(
            dev_process_pids("", "/Users/a/Desktop/Projects/.interface-dev"),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn host_pids_match_the_executable_and_not_a_cli_that_names_it() {
        // `ps -axo pid=,comm=`: the executable only. A Codex pane's command line
        // carries the host path in its notify-hook arguments, which is why the
        // host is not found through `pgrep -f`.
        let listing = "  67214 /Users/a/Desktop/Projects/.interface-dev/target/debug/interface-host\n\
                       67380 /Users/a/.codex/bin/codex\n\
                       33387 /Applications/Interface.app/Contents/Resources/interface-host\n\
                       12 /private/tmp/other/target/debug/interface-host\n";
        assert_eq!(
            host_pids(
                listing,
                "/Users/a/Desktop/Projects/.interface-dev/target/debug/interface-host"
            ),
            vec![67214]
        );
        assert_eq!(host_pids("", "/x/interface-host"), Vec::<u32>::new());
    }

    #[test]
    fn readiness_requires_the_current_launch_and_checks_failure_first() {
        let ready = "INTERFACE_DEV_READY {\"launch\":\"one\",\"revision\":\"abc\",\"pid\":42}\n";
        assert!(matches!(
            log_verdict(ready, "one", "abc"),
            Some(Verdict::Ready(_))
        ));
        assert_eq!(log_verdict(ready, "old", "abc"), None);
        assert_eq!(log_verdict(ready, "one", "other"), None);
        assert_eq!(
            log_verdict("dev.mjs: launching Interface Dev", "one", "abc"),
            None
        );
        assert!(matches!(
            log_verdict(
                &format!("{ready}dev.mjs: failed to start Electron"),
                "one",
                "abc"
            ),
            Some(Verdict::Failed(_))
        ));
    }

    #[test]
    fn front_app_matches_only_the_dev_bundle() {
        let dev = "\"Interface Dev\" ASN:0x0-0x318e18b: (in front)\n    bundleID=\"com.andreromero.interface.dev\"\n";
        let production =
            "[ NULL ]  ASN:0x0-0x2eddedb: (in front)\n    bundleID=\"com.andreromero.interface\"\n";
        assert!(names_dev_bundle(dev));
        assert!(!names_dev_bundle(production));
        assert!(!names_dev_bundle(""));
    }

    #[test]
    fn rotate_log_sets_aside_only_a_log_over_the_limit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let log = dir.path().join("interface-dev.log");
        let previous = dir.path().join("interface-dev.log.1");

        rotate_log(&log, 8).unwrap();
        assert!(!log.exists() && !previous.exists());

        fs::write(&log, b"12345678").unwrap();
        rotate_log(&log, 8).unwrap();
        assert_eq!(fs::read(&log).unwrap(), b"12345678");
        assert!(!previous.exists());

        fs::write(&previous, b"oldest").unwrap();
        fs::write(&log, b"123456789").unwrap();
        rotate_log(&log, 8).unwrap();
        assert!(!log.exists());
        assert_eq!(fs::read(&previous).unwrap(), b"123456789");
    }

    #[test]
    fn dev_path_puts_the_toolchain_first() {
        let path = dev_path(Path::new("/Users/a"));
        let path = path.to_string_lossy();
        assert!(path.starts_with("/Users/a/.cargo/bin"), "{path}");
        if let Some(inherited) = env::var_os("PATH") {
            assert!(path.ends_with(&*inherited.to_string_lossy()), "{path}");
        }
    }
}
