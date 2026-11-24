use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        Self { _temp: temp, root }
    }
    fn write(&self, path: &str) {
        fs::write(self.root.join(path), "original").unwrap();
        fs::set_permissions(self.root.join(path), fs::Permissions::from_mode(0o644)).unwrap();
    }
    fn change(path: &str) -> Change {
        Change {
            path: path.into(),
            before: Some("original".into()),
            before_sha256: Some(digest("original")),
            before_mode: Some(0o644),
            after: Some("proposed".into()),
            after_mode: Some(0o644),
            source_changed: false,
        }
    }
    fn unchanged(&self, path: &str) {
        assert_eq!(
            fs::read_to_string(self.root.join(path)).unwrap(),
            "original"
        );
        assert_eq!(
            fs::metadata(self.root.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
}

#[test]
fn symlink_files_and_ancestors_cannot_modify_outside_source() {
    let source = Fixture::new();
    let outside = Fixture::new();
    outside.write("file.txt");
    symlink(outside.root.join("file.txt"), source.root.join("link.txt")).unwrap();
    symlink(&outside.root, source.root.join("sub")).unwrap();
    for path in ["link.txt", "sub/file.txt"] {
        assert_eq!(
            apply_changes(&source.root, &[Fixture::change(path)])["state"],
            "conflict"
        );
        outside.unchanged("file.txt");
    }
    assert!(
        fs::symlink_metadata(source.root.join("link.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn invalid_and_duplicate_paths_refuse_the_whole_packet() {
    let source = Fixture::new();
    source.write("first.txt");
    for path in [
        "../escape.txt",
        "/tmp/escape.txt",
        "a/../first.txt",
        "first.txt",
        "./first.txt",
        "a//b.txt",
    ] {
        let result = apply_changes(
            &source.root,
            &[Fixture::change("first.txt"), Fixture::change(path)],
        );
        assert_eq!(result["state"], "conflict", "{path}: {result}");
        source.unchanged("first.txt");
    }
}

#[test]
fn stale_sha_or_mode_on_a_later_file_prevents_every_write() {
    let source = Fixture::new();
    source.write("first.txt");
    source.write("last.txt");
    let mut stale_sha = Fixture::change("last.txt");
    stale_sha.before_sha256 = Some(digest("stale"));
    let mut stale_mode = Fixture::change("last.txt");
    stale_mode.before_mode = Some(0o600);
    stale_mode.after_mode = Some(0o600);
    for stale in [stale_sha, stale_mode] {
        assert_eq!(
            apply_changes(&source.root, &[Fixture::change("first.txt"), stale])["state"],
            "conflict"
        );
        source.unchanged("first.txt");
        source.unchanged("last.txt");
    }
}

#[test]
fn deletion_permission_changes_and_executable_creation_are_deliberate_parent_actions() {
    let source = Fixture::new();
    source.write("keep.txt");
    let mut deletion = Fixture::change("keep.txt");
    deletion.after = None;
    let mut mode = Fixture::change("keep.txt");
    mode.after_mode = Some(0o755);
    for change in [deletion, mode] {
        assert_eq!(apply_changes(&source.root, &[change])["state"], "conflict");
        source.unchanged("keep.txt");
    }
    for mode in [0o755, 0o666, 0o4755] {
        let mut create = Fixture::change("new.txt");
        create.before = None;
        create.before_sha256 = None;
        create.before_mode = None;
        create.after_mode = Some(mode);
        assert_eq!(apply_changes(&source.root, &[create])["state"], "conflict");
        assert!(!source.root.join("new.txt").exists());
    }
}

#[test]
fn malformed_and_oversized_content_does_not_replace_source() {
    let source = Fixture::new();
    source.write("keep.txt");
    for text in ["bad\0text".into(), "x".repeat(MAX_FILE_BYTES + 1)] {
        let mut change = Fixture::change("keep.txt");
        change.after = Some(text);
        assert_eq!(apply_changes(&source.root, &[change])["state"], "conflict");
        source.unchanged("keep.txt");
    }
    assert_eq!(apply_changes(&source.root, &[])["state"], "conflict");
    assert_eq!(
        apply_changes(&source.root, &vec![Fixture::change("keep.txt"); 17])["state"],
        "conflict"
    );
}
