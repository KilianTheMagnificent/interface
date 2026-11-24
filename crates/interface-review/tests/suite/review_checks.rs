use std::fs;
use std::path::Path;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crate::temp_dir;
use interface_contracts::ipc::{ReviewCheckDiscoveryError, ReviewCheckName, ReviewCheckRunStatus};
use interface_review::{
    REVIEW_ACTION_LIMITS, ReviewCheckError, ReviewCheckInvocation, ReviewCheckRunOptions,
    ReviewCheckRunner, ReviewCheckRunnerResult, discover_review_checks, run_review_checks,
};

#[derive(Default)]
struct FakeRunner {
    invocations: Mutex<Vec<ReviewCheckInvocation>>,
}

struct RootSwappingRunner {
    backup: std::path::PathBuf,
    swapped: AtomicBool,
}

impl ReviewCheckRunner for RootSwappingRunner {
    fn run(&self, invocation: &ReviewCheckInvocation) -> ReviewCheckRunnerResult {
        if !self.swapped.swap(true, Ordering::SeqCst) {
            fs::rename(&invocation.cwd, &self.backup).expect("move authorized root");
            fs::create_dir(&invocation.cwd).expect("replacement root");
        }
        ReviewCheckRunnerResult {
            exit_code: Some(0),
            stdout: "output that must be discarded".to_owned(),
            ..ReviewCheckRunnerResult::default()
        }
    }
}

impl ReviewCheckRunner for FakeRunner {
    fn run(&self, invocation: &ReviewCheckInvocation) -> ReviewCheckRunnerResult {
        self.invocations
            .lock()
            .expect("invocations lock")
            .push(invocation.clone());
        let name = invocation.args.last().map(String::as_str);
        match name {
            Some("check") => ReviewCheckRunnerResult {
                exit_code: Some(0),
                stdout: "ééé".to_owned(),
                ..ReviewCheckRunnerResult::default()
            },
            Some("test") => ReviewCheckRunnerResult {
                exit_code: Some(2),
                stderr: "failed".to_owned(),
                ..ReviewCheckRunnerResult::default()
            },
            _ => ReviewCheckRunnerResult::default(),
        }
    }
}

fn write_package(root: &Path) {
    fs::write(
        root.join("package.json"),
        r#"{"scripts":{"deploy":"ignored","check":"node check.js","test":"node test.js","typecheck":"   ","lint":7,"build":"node build.js"}}"#,
    )
    .expect("package fixture");
}

#[test]
fn discovery_uses_fixed_order_and_does_not_rewrite_package() {
    let directory = temp_dir();
    write_package(directory.path());
    let package = directory.path().join("package.json");
    let before = fs::read(&package).expect("package before");
    let before_metadata = fs::metadata(&package).expect("metadata before");

    let discovery = discover_review_checks(directory.path()).expect("discovery");

    assert!(discovery.package_present);
    assert_eq!(
        discovery.checks,
        vec![
            ReviewCheckName::Check,
            ReviewCheckName::Test,
            ReviewCheckName::Build
        ]
    );
    assert_eq!(fs::read(&package).expect("package after"), before);
    let after_metadata = fs::metadata(&package).expect("metadata after");
    assert_eq!(after_metadata.len(), before_metadata.len());
    assert_eq!(
        after_metadata.modified().ok(),
        before_metadata.modified().ok()
    );
}

#[test]
fn selected_checks_run_sequentially_with_exact_shell_free_npm_argv_and_caps() {
    let directory = temp_dir();
    write_package(directory.path());
    let runner = FakeRunner::default();
    let options = ReviewCheckRunOptions {
        runner: Some(&runner),
        deadline: Some(Duration::from_secs(2)),
        max_output_bytes: Some(3),
    };

    let results = run_review_checks(
        directory.path(),
        &["check".to_owned(), "test".to_owned()],
        &options,
    )
    .expect("check run");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].name, ReviewCheckName::Check);
    assert_eq!(results[0].status, ReviewCheckRunStatus::OutputLimit);
    assert_eq!(results[0].stdout, "é");
    assert!(results[0].output_truncated);
    assert_eq!(results[1].status, ReviewCheckRunStatus::OutputLimit);
    assert_eq!(results[1].stderr, "fai");
    let invocations = runner.invocations.lock().expect("invocations lock");
    assert_eq!(invocations.len(), 2);
    assert_eq!(invocations[0].args, ["run", "--silent", "check"]);
    assert_eq!(invocations[1].args, ["run", "--silent", "test"]);
    assert!(invocations.iter().all(|invocation| {
        invocation.executable == if cfg!(windows) { "npm.cmd" } else { "npm" }
            && invocation.cwd == directory.path()
            && invocation.deadline == Duration::from_secs(2)
            && invocation.max_output_bytes == 3
    }));
}

#[test]
fn arbitrary_duplicate_and_undiscovered_checks_are_rejected_before_execution() {
    let directory = temp_dir();
    write_package(directory.path());
    let runner = FakeRunner::default();
    let options = ReviewCheckRunOptions {
        runner: Some(&runner),
        ..ReviewCheckRunOptions::default()
    };

    assert_eq!(
        run_review_checks(directory.path(), &["deploy".to_owned()], &options),
        Err(ReviewCheckError::CheckUnavailable("deploy".to_owned()))
    );
    assert_eq!(
        run_review_checks(
            directory.path(),
            &["check".to_owned(), "check".to_owned()],
            &options
        ),
        Err(ReviewCheckError::DuplicateSelection)
    );
    assert_eq!(
        run_review_checks(directory.path(), &["lint".to_owned()], &options),
        Err(ReviewCheckError::CheckUnavailable("lint".to_owned()))
    );
    assert!(
        runner
            .invocations
            .lock()
            .expect("invocations lock")
            .is_empty()
    );
}

#[test]
fn malformed_and_non_text_packages_report_invalid_package() {
    let malformed = temp_dir();
    fs::write(malformed.path().join("package.json"), "{not-json")
        .expect("malformed package fixture");
    assert_eq!(
        discover_review_checks(malformed.path())
            .expect("malformed discovery")
            .error,
        Some(ReviewCheckDiscoveryError::InvalidPackage)
    );

    let non_text = temp_dir();
    fs::write(non_text.path().join("package.json"), [b'{', 0, b'}'])
        .expect("non-text package fixture");
    assert_eq!(
        discover_review_checks(non_text.path())
            .expect("non-text discovery")
            .error,
        Some(ReviewCheckDiscoveryError::InvalidPackage)
    );
}

#[test]
fn package_absence_and_script_shapes_keep_their_discovery() {
    let missing = temp_dir();
    let discovery = discover_review_checks(missing.path()).expect("missing discovery");
    assert!(!discovery.package_present);
    assert!(discovery.checks.is_empty());
    assert_eq!(discovery.error, None);
    assert_eq!(discovery.root, missing.path().to_string_lossy());

    for (source, error) in [
        (&b"[]"[..], None),
        (b"{}", None),
        (
            br#"{"scripts":5}"#,
            Some(ReviewCheckDiscoveryError::InvalidPackage),
        ),
        (b"\xff{}", Some(ReviewCheckDiscoveryError::InvalidPackage)),
    ] {
        let directory = temp_dir();
        fs::write(directory.path().join("package.json"), source).expect("package fixture");
        let discovery = discover_review_checks(directory.path()).expect("discovery");
        assert!(discovery.package_present);
        assert!(discovery.checks.is_empty());
        assert_eq!(discovery.error, error);
    }
}

#[cfg(unix)]
#[test]
fn symlink_hardlink_and_oversized_packages_are_refused() {
    use std::os::unix::fs::symlink;

    let base = temp_dir();
    let source = base.path().join("source.json");
    fs::write(&source, "{}").expect("source fixture");

    let symlink_root = base.path().join("symlink-root");
    fs::create_dir(&symlink_root).expect("symlink root");
    symlink(&source, symlink_root.join("package.json")).expect("package symlink");
    assert_eq!(
        discover_review_checks(&symlink_root)
            .expect("symlink discovery")
            .error,
        Some(ReviewCheckDiscoveryError::UnsafePackage)
    );

    let hardlink_root = base.path().join("hardlink-root");
    fs::create_dir(&hardlink_root).expect("hardlink root");
    fs::hard_link(&source, hardlink_root.join("package.json")).expect("package hardlink");
    assert_eq!(
        discover_review_checks(&hardlink_root)
            .expect("hardlink discovery")
            .error,
        Some(ReviewCheckDiscoveryError::UnsafePackage)
    );

    let large_root = base.path().join("large-root");
    fs::create_dir(&large_root).expect("large root");
    let large = fs::File::create(large_root.join("package.json")).expect("large package");
    large
        .set_len(u64::try_from(REVIEW_ACTION_LIMITS.max_package_bytes + 1).expect("size"))
        .expect("grow package");
    assert_eq!(
        discover_review_checks(&large_root)
            .expect("large discovery")
            .error,
        Some(ReviewCheckDiscoveryError::PackageTooLarge)
    );
}

#[test]
fn check_root_swap_discards_process_output() {
    let directory = temp_dir();
    write_package(directory.path());
    let root = directory.path().to_path_buf();
    let backup = root.with_extension("authorized-backup");
    let runner = RootSwappingRunner {
        backup: backup.clone(),
        swapped: AtomicBool::new(false),
    };
    let options = ReviewCheckRunOptions {
        runner: Some(&runner),
        ..ReviewCheckRunOptions::default()
    };

    let result = run_review_checks(&root, &["check".to_owned()], &options);

    assert_eq!(result, Err(ReviewCheckError::UnsafeRoot));
    fs::remove_dir(&root).expect("remove replacement root");
    fs::rename(&backup, &root).expect("restore fixture root");
}
