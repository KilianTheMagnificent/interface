use std::{
    collections::HashSet,
    env::{JoinPathsError, join_paths, split_paths},
    ffi::{OsStr, OsString},
    fs::Metadata,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use tokio::{process::Command, sync::watch, time::timeout};

use crate::{EngineError, EngineErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableSource {
    ExplicitOverride,
    LoginShellPath,
    KnownLocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedExecutable {
    pub path: PathBuf,
    pub source: ExecutableSource,
}

/// Ordered, already-sanitized inputs to executable discovery.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutableResolution {
    pub explicit_override: Option<PathBuf>,
    pub login_shell_path: Option<PathBuf>,
    pub known_locations: Vec<PathBuf>,
    /// Provider executables in packaged builds should resolve to absolute paths.
    pub require_absolute: bool,
}

/// Resolve in legacy order: valid override, login-shell result, known paths.
pub fn resolve_executable(
    resolution: &ExecutableResolution,
    exists: impl Fn(&Path) -> bool,
) -> Option<ResolvedExecutable> {
    let candidates = resolution
        .explicit_override
        .iter()
        .map(|path| (path, ExecutableSource::ExplicitOverride))
        .chain(
            resolution
                .login_shell_path
                .iter()
                .map(|path| (path, ExecutableSource::LoginShellPath)),
        )
        .chain(
            resolution
                .known_locations
                .iter()
                .map(|path| (path, ExecutableSource::KnownLocation)),
        );
    for (path, source) in candidates {
        if (!resolution.require_absolute || path.is_absolute()) && exists(path) {
            return Some(ResolvedExecutable {
                path: path.clone(),
                source,
            });
        }
    }
    None
}

/// Prepend provider-relevant directories while retaining the inherited PATH.
///
/// # Errors
///
/// Returns `JoinPathsError` when a supplied path contains the platform PATH separator.
pub fn augment_search_path(
    current: Option<&OsStr>,
    prepend: impl IntoIterator<Item = PathBuf>,
) -> Result<OsString, JoinPathsError> {
    let mut seen = HashSet::<PathBuf>::new();
    let mut ordered = Vec::new();
    for path in prepend
        .into_iter()
        .chain(current.into_iter().flat_map(split_paths))
    {
        if path.as_os_str().is_empty() || !seen.insert(path.clone()) {
            continue;
        }
        ordered.push(path);
    }
    join_paths(ordered)
}

/// The PATH a provider child inherits: the resolved executable's own directory
/// (which an explicit configuration override already decided), then the known
/// install locations, then the inherited PATH. A packaged launch cannot rely on
/// the PATH the desktop shell happened to inherit.
///
/// engine-acp's `builtin_search_path` deliberately keeps its own copy: it
/// interleaves provider entries both before and after `$HOME/.local/bin`, and a
/// single shared order would silently change executable resolution.
///
/// # Errors
///
/// Returns `JoinPathsError` if any supplied directory contains a PATH separator.
pub fn provider_search_path(
    executable: &Path,
    current: Option<&OsStr>,
    home: Option<&Path>,
) -> Result<OsString, JoinPathsError> {
    let mut prepend = Vec::<PathBuf>::new();
    if let Some(directory) = executable.parent() {
        prepend.push(directory.to_path_buf());
    }
    if let Some(home) = home.filter(|home| !home.as_os_str().is_empty()) {
        prepend.push(home.join(".local/bin"));
    }
    prepend.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    augment_search_path(current, prepend)
}

/// An argv-only child-process description. It never passes through a shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

impl ProcessSpec {
    #[must_use]
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }
}

/// Which version of a file a path currently resolves to (symlinks followed).
/// An atomic replace changes the inode, an in-place write the size or the
/// nanosecond times, and an update that retargets an install's symlink chain
/// resolves to a different file altogether.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileVersion {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl FileVersion {
    #[must_use]
    pub fn from_metadata(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }

    /// `None` when the path cannot be stat'ed.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| Self::from_metadata(&metadata))
    }
}

/// The last successful `--version` answer for one executable path, kept only
/// while the path still resolves to the same file. The answer is a function of
/// that file alone, so asking again could not change it; an upgrade or a
/// downgrade changes the file and is asked again. A failed probe is never kept,
/// and a path that cannot be stat'ed is asked every time. A kept answer says
/// nothing about whether the file still runs: a caller that saw it fail to
/// start calls `forget` and asks again, so the version probe fails as it would
/// have without the cache.
#[derive(Debug)]
pub struct VersionCache<T = String>(Mutex<Option<(FileVersion, T)>>);

impl<T> Default for VersionCache<T> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<T: Clone> VersionCache<T> {
    /// Returns the kept answer while `executable` is unchanged; otherwise
    /// awaits `probe` and keeps its answer if it succeeded.
    ///
    /// # Errors
    ///
    /// Returns the probe's own error, which is not kept.
    pub async fn get_or_probe<E>(
        &self,
        executable: &Path,
        probe: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let version = FileVersion::of(executable);
        if let Some(version) = version
            && let Some((kept, answer)) = self.lock().as_ref()
            && *kept == version
        {
            return Ok(answer.clone());
        }
        let answer = probe.await?;
        if let Some(version) = version {
            *self.lock() = Some((version, answer.clone()));
        }
        Ok(answer)
    }

    /// The kept answer, while `executable` still resolves to the file it came
    /// from.
    #[must_use]
    pub fn get(&self, executable: &Path) -> Option<T> {
        let version = FileVersion::of(executable)?;
        self.lock()
            .as_ref()
            .filter(|(kept, _)| *kept == version)
            .map(|(_, answer)| answer.clone())
    }

    /// Drops the kept answer, so the next probe asks the executable again.
    pub fn forget(&self) {
        *self.lock() = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(FileVersion, T)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The most of a version answer kept; the rest is read and dropped.
const VERSION_OUTPUT_LIMIT: usize = 64 * 1024;

/// Runs `<executable> --version` without a shell and returns its trimmed
/// stdout. `provider` names the CLI in the error prose ("Claude version probe
/// timed out"); `no_version` is the provider's message for an empty answer.
/// Every failure is definitely-unsent: the probe sends nothing to a provider.
///
/// # Errors
///
/// Timeout, a spawn failure or a non-zero exit (retryable), or an empty answer.
pub async fn cli_version(
    executable: &Path,
    provider: &str,
    no_version: &'static str,
    limit: Duration,
) -> Result<String, EngineError> {
    let not_started = || {
        EngineError::definitely_unsent(
            EngineErrorKind::Unavailable,
            format!("{provider} executable could not be started"),
            true,
        )
    };
    let mut command = Command::new(executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| not_started())?;
    let mut stdout = child.stdout.take().ok_or_else(not_started)?;
    let mut answer = Vec::new();
    let keep = |bytes: &[u8]| {
        let room = VERSION_OUTPUT_LIMIT.saturating_sub(answer.len());
        answer.extend_from_slice(&bytes[..room.min(bytes.len())]);
        Ok(())
    };
    // The probe ends when the CLI exits, not when every copy of its stdout
    // closes: something the CLI left behind, or a child another thread
    // spawned as the pipe was made, may hold it open.
    let (tell, exited) = watch::channel(false);
    let (status, read) = timeout(limit, async {
        tokio::join!(
            crate::child_output::wait_and_tell(&mut child, tell),
            crate::child_output::read_until_done(&mut stdout, exited, None, keep),
        )
    })
    .await
    .map_err(|_| {
        EngineError::definitely_unsent(
            EngineErrorKind::Timeout,
            format!("{provider} version probe timed out"),
            true,
        )
    })?;
    let status = status.map_err(|_| not_started())?;
    read.map_err(|_| not_started())?;
    if !status.success() {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::Unavailable,
            format!("{provider} version probe exited unsuccessfully"),
            true,
        ));
    }
    let version = String::from_utf8_lossy(&answer).trim().to_owned();
    if version.is_empty() {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::Protocol,
            no_version,
            false,
        ));
    }
    Ok(version)
}

/// The path an adapter launches, shared with the host that resolved it, so
/// Models & providers → Refresh can point the adapter at a CLI installed
/// while Interface runs. Each launch reads it once; sessions already running
/// keep the process they started with.
#[derive(Clone, Debug)]
pub struct ExecutablePath(std::sync::Arc<std::sync::RwLock<PathBuf>>);

impl ExecutablePath {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(std::sync::Arc::new(std::sync::RwLock::new(path)))
    }

    /// The path to launch now.
    #[must_use]
    pub fn get(&self) -> PathBuf {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Points later launches at `path`. Answers whether it changed.
    pub fn set(&self, path: PathBuf) -> bool {
        let mut current = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if *current == path {
            return false;
        }
        *current = path;
        true
    }
}

/// What a CLI's own status command printed and whether it exited cleanly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliAnswer {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `<executable> <args…>` without a shell or stdin, for at most `limit`,
/// keeping up to 64 KiB of each output stream. For read-only status commands
/// (`claude auth status`, `codex login status`); `None` when it could not be
/// started or did not finish in time.
pub async fn cli_query(executable: &Path, args: &[&str], limit: Duration) -> Option<CliAnswer> {
    cli_query_with_environment(executable, args, &[], limit).await
}

/// [`cli_query`] with `environment` set on the child over what it inherits:
/// an added provider account's directory variable. Empty sets nothing.
pub async fn cli_query_with_environment(
    executable: &Path,
    args: &[&str],
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    limit: Duration,
) -> Option<CliAnswer> {
    let mut command = Command::new(executable);
    command.envs(environment.iter().map(|(name, value)| (name, value)));
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let keep = |into: &mut Vec<u8>, bytes: &[u8]| {
        let room = VERSION_OUTPUT_LIMIT.saturating_sub(into.len());
        into.extend_from_slice(&bytes[..room.min(bytes.len())]);
    };
    let (tell, exited) = watch::channel(false);
    let exited_too = exited.clone();
    let (status, read_out, read_err) = timeout(limit, async {
        tokio::join!(
            crate::child_output::wait_and_tell(&mut child, tell),
            crate::child_output::read_until_done(&mut stdout, exited, None, |bytes: &[u8]| {
                keep(&mut out, bytes);
                Ok(())
            }),
            crate::child_output::read_until_done(
                &mut stderr,
                exited_too,
                None,
                |bytes: &[u8]| {
                    keep(&mut err, bytes);
                    Ok(())
                }
            ),
        )
    })
    .await
    .ok()?;
    let status = status.ok()?;
    read_out.ok()?;
    read_err.ok()?;
    Some(CliAnswer {
        success: status.success(),
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_candidates_fall_through_in_the_documented_order() {
        let resolution = ExecutableResolution {
            explicit_override: Some("/stale/codex".into()),
            login_shell_path: Some("/login/codex".into()),
            known_locations: vec!["/known/codex".into()],
            require_absolute: true,
        };
        let resolved = resolve_executable(&resolution, |path| path == Path::new("/login/codex"));
        assert_eq!(
            resolved,
            Some(ResolvedExecutable {
                path: "/login/codex".into(),
                source: ExecutableSource::LoginShellPath,
            })
        );
    }

    #[tokio::test]
    async fn version_cache_keeps_only_a_successful_answer_for_an_unchanged_file() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("cli");
        std::fs::write(&file, "one").unwrap();
        let cache = VersionCache::<u32>::default();
        let asked = AtomicUsize::new(0);
        let ask = |answer: Result<u32, ()>| {
            let asked = &asked;
            async move {
                asked.fetch_add(1, Ordering::SeqCst);
                answer
            }
        };

        assert_eq!(cache.get_or_probe(&file, ask(Err(()))).await, Err(()));
        assert_eq!(cache.get_or_probe(&file, ask(Ok(1))).await, Ok(1));
        assert_eq!(cache.get_or_probe(&file, ask(Ok(2))).await, Ok(1));
        assert_eq!(asked.load(Ordering::SeqCst), 2, "a failure is not kept");
        assert_eq!(cache.get(&file), Some(1));
        cache.forget();
        assert_eq!(cache.get(&file), None);
        assert_eq!(cache.get_or_probe(&file, ask(Ok(1))).await, Ok(1));
        assert_eq!(
            asked.load(Ordering::SeqCst),
            3,
            "a forgotten answer is asked again"
        );

        std::fs::write(&file, "rewritten in place").unwrap();
        assert_eq!(
            cache.get(&file),
            None,
            "a rewritten file has no kept answer"
        );
        assert_eq!(cache.get_or_probe(&file, ask(Ok(3))).await, Ok(3));
        let missing = directory.path().join("missing");
        assert_eq!(cache.get_or_probe(&missing, ask(Ok(4))).await, Ok(4));
        assert_eq!(cache.get_or_probe(&missing, ask(Ok(5))).await, Ok(5));
        assert_eq!(
            asked.load(Ordering::SeqCst),
            6,
            "an unstat-able path is always asked"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_version_probe_ends_when_the_cli_exits_though_its_pipe_stays_open() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let cli = directory.path().join("cli");
        let lingering = directory.path().join("lingering.pid");
        // The CLI leaves a process holding its stdout's write end, as a child
        // that inherited the pipe would.
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\n/bin/sleep 30 &\necho $! > '{}'\necho 1.2.3\n",
                lingering.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let version = cli_version(&cli, "Test", "no version", Duration::from_secs(5)).await;
        let elapsed = started.elapsed();
        if let Ok(pid) = std::fs::read_to_string(&lingering) {
            let _ = std::process::Command::new("/bin/kill")
                .arg(pid.trim())
                .status();
        }
        assert_eq!(version.unwrap(), "1.2.3");
        assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");

        // A CLI that never exits still times out.
        std::fs::write(&cli, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        let error = cli_version(&cli, "Test", "no version", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(error.message, "Test version probe timed out");
    }

    #[test]
    fn path_augmentation_deduplicates_without_reordering() {
        let result = augment_search_path(
            Some(OsStr::new("/usr/bin:/custom")),
            [PathBuf::from("/custom"), PathBuf::from("/opt/homebrew/bin")],
        )
        .unwrap();
        assert_eq!(result, OsString::from("/custom:/opt/homebrew/bin:/usr/bin"));
    }
}
