//! Every integration test in this crate, linked as one binary.

mod repository_programs;
mod repository_review;
mod review_checks;

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A temporary directory outside the source tree, named by its canonical
/// path: Review refuses a root reached through a symbolic link, and the
/// system temporary directory sits behind one on macOS.
struct TestDir {
    path: PathBuf,
    _directory: TempDir,
}

impl TestDir {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn temp_dir() -> TestDir {
    let directory = tempfile::Builder::new()
        .prefix("interface-review-")
        .tempdir()
        .expect("temporary directory");
    let path = directory
        .path()
        .canonicalize()
        .expect("canonical temporary directory");
    TestDir {
        path,
        _directory: directory,
    }
}
