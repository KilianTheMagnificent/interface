use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use interface_common::sync::lock;
use interface_contracts::ipc::TerminalApp;
use interface_terminal::{
    ApprovedTerminalRoots, ExternalTerminalBackend, SystemExternalTerminalBackend,
    TerminalBackendError, TerminalError, open_external_terminal,
};
use tempfile::TempDir;

fn temp_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("interface-external-terminal-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory")
}

#[derive(Default)]
struct FakeExternal {
    ghostty: bool,
    launches: Mutex<Vec<(PathBuf, Vec<String>)>>,
}

impl ExternalTerminalBackend for FakeExternal {
    fn ghostty_installed(&self) -> bool {
        self.ghostty
    }

    fn spawn_detached(
        &self,
        executable: &Path,
        args: &[String],
    ) -> Result<(), TerminalBackendError> {
        lock(&self.launches).push((executable.to_path_buf(), args.to_vec()));
        Ok(())
    }
}

#[test]
fn terminal_app_and_ghostty_use_the_exact_legacy_open_argv() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let terminal = FakeExternal::default();
    let opened = open_external_terminal(directory.path(), &approved, &terminal).expect("open");
    assert_eq!(opened.app, TerminalApp::Terminal);
    assert_eq!(
        lock(&terminal.launches).as_slice(),
        [(
            PathBuf::from("/usr/bin/open"),
            vec![
                "-a".to_owned(),
                "Terminal".to_owned(),
                directory.path().to_string_lossy().into_owned()
            ]
        )]
    );

    let ghostty = FakeExternal {
        ghostty: true,
        ..FakeExternal::default()
    };
    let opened = open_external_terminal(directory.path(), &approved, &ghostty).expect("open");
    assert_eq!(opened.app, TerminalApp::Ghostty);
    assert_eq!(lock(&ghostty.launches)[0].0, PathBuf::from("/usr/bin/open"));
    assert_eq!(
        lock(&ghostty.launches)[0].1,
        [
            "-na",
            "Ghostty",
            "--args",
            &format!("--working-directory={}", directory.path().display())
        ]
    );
}

#[test]
fn external_terminal_refuses_subdirectories_without_launching() {
    let directory = temp_dir();
    let child = directory.path().join("child");
    std::fs::create_dir(&child).expect("child");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = FakeExternal::default();

    assert_eq!(
        open_external_terminal(&child, &approved, &backend),
        Err(TerminalError::UnauthorizedCwd)
    );
    assert!(lock(&backend.launches).is_empty());
}

#[test]
fn external_terminal_refuses_an_empty_target_without_launching() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = FakeExternal::default();

    assert_eq!(
        open_external_terminal(Path::new(""), &approved, &backend),
        Err(TerminalError::UnsafeCwd)
    );
    assert!(lock(&backend.launches).is_empty());
}

#[cfg(unix)]
#[test]
fn external_terminal_refuses_a_non_utf8_root_without_lossy_retargeting() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = temp_dir();
    let root = directory.path().join(OsString::from_vec(vec![b'r', 0xff]));
    if std::fs::create_dir(&root).is_err() {
        // Some macOS volumes reject non-UTF-8 names at the filesystem layer,
        // which is already the desired fail-closed result.
        return;
    }
    let approved = ApprovedTerminalRoots::new([root.clone()]).expect("roots");
    let backend = FakeExternal::default();

    assert_eq!(
        open_external_terminal(&root, &approved, &backend),
        Err(TerminalError::UnsafeCwd)
    );
    assert!(lock(&backend.launches).is_empty());
}

#[test]
fn production_external_backend_type_is_available_without_launching_it() {
    let backend: Arc<dyn ExternalTerminalBackend> = Arc::new(SystemExternalTerminalBackend);
    let _ = backend.ghostty_installed();
}
