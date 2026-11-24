use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use interface_contracts::ipc::{
    ReviewCheckDiscovery, ReviewCheckDiscoveryError, ReviewCheckName, ReviewCheckRunResult,
    ReviewCheckRunStatus,
};
use serde_json::Value;
use thiserror::Error;

use crate::process::{cap_utf8_owned, run_bounded_process};
use crate::safety::{
    DirectorySnapshot, SafetyError, assert_directory, canonical_directory, read_verified_file,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewActionLimits {
    pub max_package_bytes: usize,
    pub max_script_chars: usize,
    pub check_deadline: Duration,
    pub max_check_output_bytes: usize,
}

pub const REVIEW_ACTION_LIMITS: ReviewActionLimits = ReviewActionLimits {
    max_package_bytes: 256 * 1024,
    max_script_chars: 8_192,
    check_deadline: Duration::from_secs(10 * 60),
    max_check_output_bytes: 512 * 1024,
};

const REVIEW_CHECK_NAMES: &[(ReviewCheckName, &str)] = &[
    (ReviewCheckName::Check, "check"),
    (ReviewCheckName::Test, "test"),
    (ReviewCheckName::Typecheck, "typecheck"),
    (ReviewCheckName::Lint, "lint"),
    (ReviewCheckName::Build, "build"),
];

/// One check run. The runner passes argv directly to `Command`, never a shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewCheckInvocation {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub deadline: Duration,
    pub max_output_bytes: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReviewCheckRunnerResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub output_truncated: bool,
    pub unavailable: bool,
}

pub trait ReviewCheckRunner: Send + Sync {
    fn run(&self, invocation: &ReviewCheckInvocation) -> ReviewCheckRunnerResult;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemReviewCheckRunner;

impl ReviewCheckRunner for SystemReviewCheckRunner {
    fn run(&self, invocation: &ReviewCheckInvocation) -> ReviewCheckRunnerResult {
        run_bounded_process(
            &invocation.executable,
            &invocation.args,
            &invocation.cwd,
            invocation.deadline,
            invocation.max_output_bytes,
        )
    }
}

#[derive(Default)]
pub struct ReviewCheckRunOptions<'a> {
    pub runner: Option<&'a dyn ReviewCheckRunner>,
    pub deadline: Option<Duration>,
    pub max_output_bytes: Option<usize>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ReviewCheckError {
    #[error("the authorized Review root changed or crossed a symbolic link")]
    UnsafeRoot,
    #[error("too many Review checks were selected")]
    TooManySelected,
    #[error("a Review check was selected more than once")]
    DuplicateSelection,
    #[error("Review check discovery failed: {0:?}")]
    DiscoveryFailed(ReviewCheckDiscoveryError),
    #[error("Review check is not available: {0}")]
    CheckUnavailable(String),
}

fn check_name(value: &str) -> Option<ReviewCheckName> {
    REVIEW_CHECK_NAMES
        .iter()
        .find_map(|(name, raw)| (*raw == value).then_some(*name))
}

fn check_name_str(value: ReviewCheckName) -> &'static str {
    REVIEW_CHECK_NAMES
        .iter()
        .find_map(|(name, raw)| (*name == value).then_some(*raw))
        .expect("every contract check has a wire name")
}

/// Discover bounded, allowlisted package scripts without modifying the project.
///
/// # Errors
///
/// Returns [`ReviewCheckError::UnsafeRoot`] when the supplied directory is not
/// a stable, canonical directory or changes during discovery.
pub fn discover_review_checks(root_path: &Path) -> Result<ReviewCheckDiscovery, ReviewCheckError> {
    let root = canonical_directory(root_path).map_err(|_| ReviewCheckError::UnsafeRoot)?;
    let discovery = |package_present, checks, error| ReviewCheckDiscovery {
        root: root.path.to_string_lossy().into_owned(),
        package_present,
        checks,
        error,
    };
    match package_checks(&root) {
        Ok(None) => Ok(discovery(false, Vec::new(), None)),
        Ok(Some(checks)) => {
            assert_directory(&root).map_err(|_| ReviewCheckError::UnsafeRoot)?;
            Ok(discovery(true, checks, None))
        }
        Err(error) => Ok(discovery(true, Vec::new(), Some(error))),
    }
}

/// The allowlisted scripts in `package.json`, in fixed order, or `None` when
/// there is no package. The symlink/type and size checks run on the entry
/// before it is opened; each refusal names its own error.
fn package_checks(
    root: &DirectorySnapshot,
) -> Result<Option<Vec<ReviewCheckName>>, ReviewCheckDiscoveryError> {
    let metadata = match fs::symlink_metadata(root.path.join("package.json")) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ReviewCheckDiscoveryError::UnsafePackage),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ReviewCheckDiscoveryError::UnsafePackage);
    }
    if metadata.len() > u64::try_from(REVIEW_ACTION_LIMITS.max_package_bytes).unwrap_or(u64::MAX) {
        return Err(ReviewCheckDiscoveryError::PackageTooLarge);
    }
    let opened = read_verified_file(
        root,
        Path::new("package.json"),
        REVIEW_ACTION_LIMITS.max_package_bytes,
        true,
        None,
        None,
    )
    .map_err(|error| match error {
        SafetyError::OutputLimit => ReviewCheckDiscoveryError::PackageTooLarge,
        _ => ReviewCheckDiscoveryError::UnsafePackage,
    })?;
    if opened.content.contains(&0) {
        return Err(ReviewCheckDiscoveryError::InvalidPackage);
    }
    let parsed = std::str::from_utf8(&opened.content)
        .ok()
        .and_then(|source| serde_json::from_str::<Value>(source).ok())
        .ok_or(ReviewCheckDiscoveryError::InvalidPackage)?;
    let scripts = match parsed.as_object().and_then(|object| object.get("scripts")) {
        None => return Ok(Some(Vec::new())),
        Some(scripts) => scripts
            .as_object()
            .ok_or(ReviewCheckDiscoveryError::InvalidPackage)?,
    };
    let checks = REVIEW_CHECK_NAMES
        .iter()
        .filter(|(_, raw_name)| {
            scripts
                .get(*raw_name)
                .and_then(Value::as_str)
                .is_some_and(|command| {
                    !command
                        .trim_matches(|character: char| {
                            character.is_whitespace() || character == '\u{feff}'
                        })
                        .is_empty()
                        && command.encode_utf16().count() <= REVIEW_ACTION_LIMITS.max_script_chars
                })
        })
        .map(|(name, _)| *name)
        .collect();
    Ok(Some(checks))
}

fn bounded_deadline(requested: Option<Duration>) -> Duration {
    requested
        .filter(|value| !value.is_zero())
        .map_or(REVIEW_ACTION_LIMITS.check_deadline, |value| {
            value.min(REVIEW_ACTION_LIMITS.check_deadline)
        })
}

fn bounded_output(requested: Option<usize>) -> usize {
    requested
        .filter(|value| *value > 0)
        .map_or(REVIEW_ACTION_LIMITS.max_check_output_bytes, |value| {
            value.min(REVIEW_ACTION_LIMITS.max_check_output_bytes)
        })
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Run selected discovered scripts sequentially through a direct `npm` argv.
///
/// # Errors
///
/// Returns an error for an unsafe root, unsafe package metadata, duplicate or
/// excessive selections, or any script outside the fixed discovery allowlist.
pub fn run_review_checks(
    root_path: &Path,
    selected: &[String],
    options: &ReviewCheckRunOptions<'_>,
) -> Result<Vec<ReviewCheckRunResult>, ReviewCheckError> {
    if selected.len() > REVIEW_CHECK_NAMES.len() {
        return Err(ReviewCheckError::TooManySelected);
    }
    let mut unique = HashSet::with_capacity(selected.len());
    if selected.iter().any(|name| !unique.insert(name.as_str())) {
        return Err(ReviewCheckError::DuplicateSelection);
    }

    let discovery = discover_review_checks(root_path)?;
    if let Some(error) = discovery.error {
        return Err(ReviewCheckError::DiscoveryFailed(error));
    }
    let mut names = Vec::with_capacity(selected.len());
    for raw in selected {
        let Some(name) = check_name(raw) else {
            return Err(ReviewCheckError::CheckUnavailable(raw.clone()));
        };
        if !discovery.checks.contains(&name) {
            return Err(ReviewCheckError::CheckUnavailable(raw.clone()));
        }
        names.push(name);
    }

    let root = canonical_directory(Path::new(&discovery.root))
        .map_err(|_| ReviewCheckError::UnsafeRoot)?;
    let system_runner = SystemReviewCheckRunner;
    let runner = options.runner.unwrap_or(&system_runner);
    let deadline = bounded_deadline(options.deadline);
    let max_output_bytes = bounded_output(options.max_output_bytes);
    let mut results = Vec::with_capacity(names.len());
    for name in names {
        assert_directory(&root).map_err(|_| ReviewCheckError::UnsafeRoot)?;
        let started = Instant::now();
        let raw_name = check_name_str(name);
        let raw = runner.run(&ReviewCheckInvocation {
            executable: "npm".to_owned(),
            args: vec!["run".to_owned(), "--silent".to_owned(), raw_name.to_owned()],
            cwd: root.path.clone(),
            deadline,
            max_output_bytes,
        });
        assert_directory(&root).map_err(|_| ReviewCheckError::UnsafeRoot)?;
        let (stdout, stdout_truncated) = cap_utf8_owned(raw.stdout, max_output_bytes);
        let (stderr, stderr_truncated) = cap_utf8_owned(raw.stderr, max_output_bytes);
        let capped = stdout_truncated || stderr_truncated;
        let output_truncated = raw.output_truncated || capped;
        let status = if raw.timed_out {
            ReviewCheckRunStatus::TimedOut
        } else if raw.unavailable {
            ReviewCheckRunStatus::Unavailable
        } else if output_truncated {
            ReviewCheckRunStatus::OutputLimit
        } else if raw.exit_code == Some(0) {
            ReviewCheckRunStatus::Passed
        } else {
            ReviewCheckRunStatus::Failed
        };
        results.push(ReviewCheckRunResult {
            name,
            status,
            exit_code: raw.exit_code,
            stdout,
            stderr,
            duration_ms: duration_millis(started.elapsed()),
            output_truncated,
        });
    }
    Ok(results)
}
