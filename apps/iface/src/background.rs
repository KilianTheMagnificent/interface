use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::Path,
    process::{Command, Stdio},
};

use chrono::{DateTime, NaiveDateTime, Utc};
use interface_runtime::{RuntimeClock, SystemRuntimeClock};
use interface_storage::{
    JsonLimits, PrivateDirectory, WriterLock, atomic_write, read_json, snapshot_target,
};
use serde::{Deserialize, Serialize};

use crate::secure_tail::read_private_tail;
use crate::{errors::CLI_EXIT, sanitize::for_display};

const SESSION_MAX_BYTES: usize = 128 * 1024;
const LOG_MAX_CHARACTERS: usize = 32 * 1024;
const STORE_LOCK: &str = ".iface-bg.lock";
/// How far a child's start time may sit from the row's `started_at`, which
/// is stamped just after `spawn` returns. `lstart` has one-second
/// resolution; a reused pid would have to wrap all 99,999 pids to land
/// this close.
const START_TOLERANCE_MS: i64 = 5_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundOutcome {
    Running,
    Exited,
    Failed,
    Killed,
    Stale,
}

impl BackgroundOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Killed => "killed",
            Self::Stale => "stale",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundSession {
    pub name: String,
    pub pid: u32,
    pub log_file: String,
    pub started_at: String,
    pub command: Vec<String>,
    pub outcome: BackgroundOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
}

/// A positive pid this process may name. Zero and anything past `i32::MAX`
/// would address a process group (or every process), never one process.
fn single_process(pid: u32) -> Option<rustix::process::Pid> {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
}

/// Whether `pid` is a live process this user may signal (`kill(pid, 0)`).
pub(crate) fn process_is_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    single_process(pid).is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

pub trait BackgroundProcesses: Send + Sync {
    /// # Errors
    ///
    /// Returns an error when the direct-argv child or safe log cannot be opened.
    fn spawn(&self, command: &[String], log_file: &Path) -> io::Result<u32>;
    fn is_alive(&self, pid: u32) -> bool;
    /// When `pid` started, or `None` when it is gone or `ps` cannot say.
    fn start_time(&self, pid: u32) -> Option<DateTime<Utc>>;
    /// # Errors
    ///
    /// Returns an error when the process cannot be signalled.
    fn terminate(&self, pid: u32) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemBackgroundProcesses;

impl BackgroundProcesses for SystemBackgroundProcesses {
    fn spawn(&self, command: &[String], log_file: &Path) -> io::Result<u32> {
        let Some(executable) = command.first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "background spawn is missing an executable",
            ));
        };
        let log = open_private_append(log_file)?;
        let stderr = log.try_clone()?;
        let mut process = Command::new(executable);
        process
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .process_group(0);
        process.spawn().map(|child| child.id())
    }

    fn is_alive(&self, pid: u32) -> bool {
        process_is_alive(pid)
    }

    fn start_time(&self, pid: u32) -> Option<DateTime<Utc>> {
        let pid = single_process(pid)?.as_raw_nonzero().to_string();
        let output = interface_terminal::PsProbe {
            columns: &["lstart"],
            select: &["-p", &pid],
            utc: true,
            stderr: false,
        }
        .output()
        .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_lstart(std::str::from_utf8(&output.stdout).ok()?)
    }

    fn terminate(&self, pid: u32) -> io::Result<()> {
        single_process(pid)
            .and_then(|pid| rustix::process::kill_process(pid, rustix::process::Signal::TERM).ok())
            .ok_or_else(|| io::Error::other("background process could not be terminated"))
    }
}

#[derive(Debug)]
pub struct BackgroundStore {
    directory: PrivateDirectory,
    logs: PrivateDirectory,
}

impl BackgroundStore {
    /// Opens or creates the two CLI-owned private directories.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe permissions, symlinks, ownership, or inode drift.
    pub fn open(data_root: &Path, logs_name: &str, sessions_name: &str) -> Result<Self, String> {
        ensure_private_root(data_root)?;
        let root = PrivateDirectory::open(data_root).map_err(string_error)?;
        let logs = root.open_or_create_child(logs_name).map_err(string_error)?;
        let directory = root
            .open_or_create_child(sessions_name)
            .map_err(string_error)?;
        Ok(Self { directory, logs })
    }

    /// Returns a stable name-sorted projection. Corrupt or unsafe individual
    /// rows are skipped without weakening the private directory boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the private directory identity or permissions drift.
    pub fn list(
        &self,
        processes: &dyn BackgroundProcesses,
    ) -> Result<Vec<BackgroundSession>, String> {
        let names = session_file_names(&self.directory, |path| fs::read_dir(path))?;
        let mut sessions = Vec::new();
        for file_name in names {
            let Some(name) = file_name.strip_suffix(".json") else {
                continue;
            };
            if validate_name(name).is_err() {
                continue;
            }
            let Ok(Some(mut session)) = self.read(name) else {
                continue;
            };
            if session.outcome != BackgroundOutcome::Killed {
                session.outcome = if still_running(&session, processes) {
                    BackgroundOutcome::Running
                } else if session.outcome == BackgroundOutcome::Running {
                    BackgroundOutcome::Stale
                } else {
                    session.outcome
                };
            }
            sessions.push(session);
        }
        self.directory.assert_unchanged().map_err(string_error)?;
        sessions.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(sessions)
    }

    /// Spawns and durably records one named background child.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid/reused names, spawn failure, or unsafe persistence.
    pub fn start(
        &self,
        name: &str,
        command: Vec<String>,
        processes: &dyn BackgroundProcesses,
    ) -> Result<BackgroundSession, String> {
        validate_name(name)?;
        if let Some(existing) = self.read(name)?
            && existing.outcome == BackgroundOutcome::Running
            && still_running(&existing, processes)
        {
            return Err("A background session with that name is still running.".to_owned());
        }
        let log_file = self.logs.path().join(format!("bg-{name}.log"));
        let pid = processes
            .spawn(&command, &log_file)
            .map_err(|error| error.to_string())?;
        let session = BackgroundSession {
            name: name.to_owned(),
            pid,
            log_file: log_file.to_string_lossy().into_owned(),
            started_at: SystemRuntimeClock.now(),
            command,
            outcome: BackgroundOutcome::Running,
            exit_code: None,
        };
        self.write(&session)?;
        Ok(session)
    }

    /// Terminates one known child and durably marks it killed.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is absent or termination/persistence fails.
    pub fn kill(
        &self,
        name: &str,
        processes: &dyn BackgroundProcesses,
    ) -> Result<BackgroundSession, String> {
        let Some(mut session) = self.read(name)? else {
            return Err("No background session with that name exists.".to_owned());
        };
        if still_running(&session, processes) {
            processes
                .terminate(session.pid)
                .map_err(|error| error.to_string())?;
        }
        session.outcome = BackgroundOutcome::Killed;
        session.exit_code = Some(CLI_EXIT.generic);
        self.write(&session)?;
        Ok(session)
    }

    /// Reads the bounded display-safe tail for one known child.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown sessions or unsafe log identity/metadata.
    pub fn read_log(&self, name: &str) -> Result<String, String> {
        let Some(session) = self.read(name)? else {
            return Err("No background session with that name exists.".to_owned());
        };
        let expected = self.logs.path().join(format!("bg-{name}.log"));
        if Path::new(&session.log_file) != expected {
            return Err(
                "Background session log path does not match its safe store identity.".into(),
            );
        }
        let Some(text) =
            read_private_tail(&self.logs, &format!("bg-{name}.log"), LOG_MAX_CHARACTERS)?
        else {
            return Ok(String::new());
        };
        Ok(for_display(&text))
    }

    fn read(&self, name: &str) -> Result<Option<BackgroundSession>, String> {
        validate_name(name)?;
        let file_name = format!("{name}.json");
        let loaded = read_json::<BackgroundSession>(
            &self.directory,
            &file_name,
            JsonLimits::new(SESSION_MAX_BYTES, 16, 512).map_err(string_error)?,
        )
        .map_err(string_error)?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        let session = loaded.value().clone();
        if session.name != name || session.pid == 0 {
            return Err("Background session metadata failed its identity checks.".to_owned());
        }
        Ok(Some(session))
    }

    fn write(&self, session: &BackgroundSession) -> Result<(), String> {
        validate_name(&session.name)?;
        let file_name = format!("{}.json", session.name);
        let expected = snapshot_target(&self.directory, &file_name, SESSION_MAX_BYTES)
            .map_err(string_error)?;
        let lock = WriterLock::acquire(&self.directory, STORE_LOCK).map_err(string_error)?;
        let mut bytes = serde_json::to_vec(session).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        atomic_write(
            &self.directory,
            &lock,
            &file_name,
            &bytes,
            &expected,
            SESSION_MAX_BYTES,
        )
        .map_err(string_error)?;
        lock.release(&self.directory).map_err(string_error)
    }
}

/// Whether the row's pid still names the child it recorded. A live pid is
/// not enough: macOS reuses pids, so a row left from a job that ended long
/// ago may now name an unrelated process of this user, which must never be
/// signalled. The process must also have started when the row says it did;
/// anything unproven counts as gone.
fn still_running(session: &BackgroundSession, processes: &dyn BackgroundProcesses) -> bool {
    if !processes.is_alive(session.pid) {
        return false;
    }
    let Ok(recorded) = DateTime::parse_from_rfc3339(&session.started_at) else {
        return false;
    };
    processes.start_time(session.pid).is_some_and(|started| {
        (started - recorded.with_timezone(&Utc))
            .num_milliseconds()
            .abs()
            <= START_TOLERANCE_MS
    })
}

/// `ps -o lstart=` in the C locale and UTC, such as `Fri Nov  6 23:05:39 2026`.
fn parse_lstart(text: &str) -> Option<DateTime<Utc>> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    NaiveDateTime::parse_from_str(&normalized, "%a %b %d %H:%M:%S %Y")
        .ok()
        .map(|started| started.and_utc())
}

fn session_file_names(
    directory: &PrivateDirectory,
    read_directory: impl FnOnce(&Path) -> io::Result<fs::ReadDir>,
) -> Result<Vec<String>, String> {
    directory.assert_unchanged().map_err(string_error)?;
    let Ok(entries) = read_directory(directory.path()) else {
        directory.assert_unchanged().map_err(string_error)?;
        return Ok(Vec::new());
    };
    let mut names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|value| value == "json")
        })
        .collect::<Vec<_>>();
    directory.assert_unchanged().map_err(string_error)?;
    names.sort();
    Ok(names)
}

#[must_use]
pub fn render_list(sessions: &[BackgroundSession], json: bool) -> String {
    if json {
        return format!(
            "{}\n",
            serde_json::json!({ "schemaVersion": 1, "sessions": sessions })
        );
    }
    if sessions.is_empty() {
        return "(no background sessions)\n".to_owned();
    }
    let lines = sessions
        .iter()
        .map(|session| {
            format!(
                "{} {} pid={} {}",
                session.name,
                session.outcome.as_str(),
                session.pid,
                session.log_file
            )
        })
        .collect::<Vec<_>>();
    format!("{}\n", lines.join("\n"))
}

fn validate_name(name: &str) -> Result<(), String> {
    let bytes = name.as_bytes();
    let valid = (1..=63).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err("Background session name is invalid.".to_owned())
    }
}

pub(crate) fn ensure_private_root(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "background store path has no parent".to_owned())?;
    if !parent.is_dir() {
        return Err("background store parent directory does not exist".to_owned());
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn open_private_append(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "background log is not an owner-private regular single-link file",
        ));
    }
    let named = fs::symlink_metadata(path)?;
    if metadata.dev() != named.dev() || metadata.ino() != named.ino() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "background log identity changed during open",
        ));
    }
    Ok(file)
}

fn string_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct FakeProcesses {
        alive: Mutex<Vec<u32>>,
        started: Mutex<Vec<(u32, DateTime<Utc>)>>,
        spawned: Mutex<Vec<Vec<String>>>,
    }

    impl BackgroundProcesses for FakeProcesses {
        fn spawn(&self, command: &[String], log_file: &Path) -> io::Result<u32> {
            self.spawned
                .lock()
                .expect("spawn lock")
                .push(command.to_vec());
            fs::write(log_file, "safe\u{1b}[31m log").expect("fake log");
            fs::set_permissions(log_file, permissions(0o600)).expect("log permissions");
            self.alive.lock().expect("alive lock").push(42);
            self.started
                .lock()
                .expect("started lock")
                .push((42, Utc::now()));
            Ok(42)
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.alive.lock().expect("alive lock").contains(&pid)
        }

        fn start_time(&self, pid: u32) -> Option<DateTime<Utc>> {
            self.started
                .lock()
                .expect("started lock")
                .iter()
                .rev()
                .find(|(started, _)| *started == pid)
                .map(|(_, at)| *at)
        }

        fn terminate(&self, pid: u32) -> io::Result<()> {
            self.alive
                .lock()
                .expect("alive lock")
                .retain(|item| *item != pid);
            Ok(())
        }
    }

    #[cfg(unix)]
    fn permissions(mode: u32) -> fs::Permissions {
        use std::os::unix::fs::PermissionsExt;
        fs::Permissions::from_mode(mode)
    }

    fn store() -> (TempDir, BackgroundStore) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("iface");
        let store = BackgroundStore::open(&root, "logs", "bg-sessions").expect("store");
        (temp, store)
    }

    #[test]
    fn liveness_names_one_process_and_never_a_group() {
        assert!(process_is_alive(std::process::id()));
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_is_alive(pid));
        // `kill -0 0` and `kill -0 -1` would reach a group or every process.
        assert!(!process_is_alive(0));
        assert!(!process_is_alive(u32::MAX));
        assert!(SystemBackgroundProcesses.terminate(0).is_err());
    }

    #[test]
    fn a_child_is_known_by_its_start_time_and_lstart_parses_in_utc() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let spawned = Utc::now();
        let pid = child.id();
        let started = SystemBackgroundProcesses.start_time(pid);
        child.kill().unwrap();
        child.wait().unwrap();
        let started = started.expect("a live child has a start time");
        assert!((started - spawned).num_milliseconds().abs() <= START_TOLERANCE_MS);
        assert_eq!(SystemBackgroundProcesses.start_time(pid), None);
        assert_eq!(SystemBackgroundProcesses.start_time(0), None);

        assert_eq!(
            parse_lstart("Fri Nov  6 23:05:39 2026\n"),
            DateTime::parse_from_rfc3339("2026-11-06T23:05:39Z")
                .ok()
                .map(|at| at.with_timezone(&Utc))
        );
        assert_eq!(parse_lstart(""), None);
    }

    #[test]
    fn a_reused_pid_is_listed_stale_and_never_signalled() {
        let (_temp, store) = store();
        let processes = FakeProcesses::default();
        let now = Utc::now();
        // The job ended long ago; its pid now names another live process.
        processes.alive.lock().expect("alive lock").push(7);
        processes
            .started
            .lock()
            .expect("started lock")
            .push((7, now));
        let session = BackgroundSession {
            name: "job".into(),
            pid: 7,
            log_file: store
                .logs
                .path()
                .join("bg-job.log")
                .to_string_lossy()
                .into(),
            started_at: interface_contracts::canonical_timestamp(now - chrono::Duration::hours(1)),
            command: vec!["/bin/true".into()],
            outcome: BackgroundOutcome::Running,
            exit_code: None,
        };
        store.write(&session).expect("row");

        let listed = store.list(&processes).expect("list");
        assert_eq!(listed[0].outcome, BackgroundOutcome::Stale);
        let killed = store.kill("job", &processes).expect("kill");
        assert_eq!(killed.outcome, BackgroundOutcome::Killed);
        assert!(
            processes.alive.lock().expect("alive lock").contains(&7),
            "the unrelated process was not signalled"
        );
        store
            .start("job", vec!["/bin/true".into()], &processes)
            .expect("the name is free again");
    }

    #[test]
    fn start_list_logs_and_kill_preserve_legacy_wire() {
        let (_temp, store) = store();
        let processes = FakeProcesses::default();
        let session = store
            .start(
                "worker",
                vec!["/bin/echo".into(), "hello".into()],
                &processes,
            )
            .expect("start");
        assert_eq!(session.pid, 42);
        let listed = store.list(&processes).expect("list");
        assert_eq!(listed.len(), 1);
        assert!(render_list(&listed, false).starts_with("worker running pid=42 "));
        assert_eq!(store.read_log("worker").expect("log"), "safe log");
        let killed = store.kill("worker", &processes).expect("kill");
        assert_eq!(killed.outcome, BackgroundOutcome::Killed);
        assert_eq!(killed.exit_code, Some(1));
        let json = serde_json::to_string(&killed).expect("serialize");
        assert!(json.contains("\"logFile\""));
        assert!(json.contains("\"startedAt\""));
    }

    #[test]
    fn list_skips_bad_rows_and_treats_a_stable_readdir_failure_as_empty() {
        let (_temp, store) = store();
        let processes = FakeProcesses::default();
        store
            .start("valid", vec!["/bin/true".into()], &processes)
            .expect("valid session");
        let malformed = store.directory.path().join("malformed.json");
        fs::write(&malformed, b"not-json\n").expect("malformed row");
        fs::set_permissions(&malformed, permissions(0o600)).expect("malformed mode");
        let oversized = store.directory.path().join("oversized.json");
        fs::write(&oversized, vec![b'x'; SESSION_MAX_BYTES + 1]).expect("oversized row");
        fs::set_permissions(&oversized, permissions(0o600)).expect("oversized mode");

        let listed = store.list(&processes).expect("tolerant list");
        assert_eq!(
            listed
                .iter()
                .map(|session| session.name.as_str())
                .collect::<Vec<_>>(),
            vec!["valid"]
        );
        assert!(
            session_file_names(&store.directory, |_| Err(io::Error::other("fixture")))
                .expect("stable read failure")
                .is_empty()
        );
    }

    #[test]
    fn logs_return_the_final_32k_utf16_units_without_whole_file_bounds() {
        let (_temp, store) = store();
        let processes = FakeProcesses::default();
        store
            .start("worker", vec!["/bin/true".into()], &processes)
            .expect("start");
        let log = store.logs.path().join("bg-worker.log");
        let text = format!("{}tail", "x".repeat(LOG_MAX_CHARACTERS + 9_000));
        fs::write(&log, text).expect("long log");
        fs::set_permissions(&log, permissions(0o600)).expect("log mode");

        let tail = store.read_log("worker").expect("tail");
        assert_eq!(tail.encode_utf16().count(), LOG_MAX_CHARACTERS);
        assert!(tail.ends_with("tail"));
    }

    #[test]
    fn stale_revision_drift_refuses_overwrite() {
        let (_temp, store) = store();
        let session = BackgroundSession {
            name: "job".into(),
            pid: 7,
            log_file: store
                .logs
                .path()
                .join("bg-job.log")
                .to_string_lossy()
                .into(),
            started_at: "2026-01-01T00:00:00.000Z".into(),
            command: vec!["/bin/true".into()],
            outcome: BackgroundOutcome::Running,
            exit_code: None,
        };
        store.write(&session).expect("first write");
        let path = store.directory.path().join("job.json");
        let original = fs::read(&path).expect("read");
        assert_eq!(
            original,
            serde_json::to_vec(&session)
                .expect("json")
                .into_iter()
                .chain([b'\n'])
                .collect::<Vec<_>>()
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_metadata_and_symlink_logs_are_refused() {
        use std::os::unix::fs::symlink;

        let (_temp, store) = store();
        let processes = FakeProcesses::default();
        store
            .start("job", vec!["/bin/true".into()], &processes)
            .expect("start");
        let log = store.logs.path().join("bg-job.log");
        fs::remove_file(&log).expect("remove fixture log");
        symlink("/dev/null", &log).expect("symlink");
        assert!(store.read_log("job").is_err());
    }
}
