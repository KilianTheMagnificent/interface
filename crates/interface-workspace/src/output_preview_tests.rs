use super::*;
use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

fn project() -> (tempfile::TempDir, PathBuf, WorkspaceAuthority) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let authority = WorkspaceAuthority::new();
    authority.grant_root(&root).unwrap();
    (temp, root, authority)
}
fn request(root: &Path, action: OutputPreviewAction, path: &str) -> OutputPreviewRequest {
    OutputPreviewRequest {
        version: 1,
        action,
        root: root.to_str().unwrap().into(),
        path: path.into(),
    }
}
fn read(authority: &WorkspaceAuthority, root: &Path, path: &str) -> OutputPreviewSnapshot {
    let OutputPreviewResponse::File { snapshot, .. } = preview_with_hooks(
        authority,
        &request(root, OutputPreviewAction::Read, path),
        None,
    )
    .unwrap() else {
        panic!("file expected")
    };
    snapshot
}

#[test]
fn snapshots_bind_exact_bytes_and_supported_formats_without_changing_editor_policy() {
    let (_temp, root, authority) = project();
    fs::write(root.join("report.md"), "# First\n").unwrap();
    let first = read(&authority, &root, "report.md");
    assert!(matches!(
        first.content,
        OutputPreviewContent::Text {
            format: OutputTextFormat::Markdown,
            ..
        }
    ));
    fs::write(root.join("report.md"), "# Second\n").unwrap();
    assert_ne!(
        first.revision,
        read(&authority, &root, "report.md").revision
    );
    assert_eq!(first.byte_length, 8);
    for (name, bytes) in [
        ("a.png", b"\x89PNG\r\n\x1a\n".as_slice()),
        ("a.pdf", b"%PDF-1.7\n"),
        ("a.patch", b"+added\n-deleted\n"),
        ("a.html", b"<script>bad()</script>"),
    ] {
        fs::write(root.join(name), bytes).unwrap();
        let snapshot = read(&authority, &root, name);
        assert!(snapshot.revision.is_some());
    }
    assert!(matches!(
        read(&authority, &root, "a.html").content,
        OutputPreviewContent::Text {
            format: OutputTextFormat::Plain,
            ..
        }
    ));
    assert_eq!(
        authority.read_text(&root.join("a.png")),
        Err(WorkspaceEditorError::BinaryFile)
    );
}

#[test]
fn listing_includes_binary_outputs_but_hides_secrets_links_and_generated_folders() {
    let (_temp, root, authority) = project();
    for name in [
        "a.png",
        "a.docx",
        "a.pdf",
        "note.txt",
        ".env",
        "credentials.json",
    ] {
        fs::write(root.join(name), "fixture").unwrap();
    }
    fs::create_dir(root.join("target")).unwrap();
    fs::create_dir(root.join("outputs")).unwrap();
    symlink(root.join("note.txt"), root.join("link.txt")).unwrap();
    let OutputPreviewResponse::Directory { entries, .. } = preview_with_hooks(
        &authority,
        &request(&root, OutputPreviewAction::List, ""),
        None,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["outputs", "a.docx", "a.pdf", "a.png", "note.txt"]
    );
    for name in [".env", "credentials.json"] {
        assert_eq!(
            preview_with_hooks(
                &authority,
                &request(&root, OutputPreviewAction::Read, name),
                None
            ),
            Err(WorkspaceEditorError::ProtectedPath)
        );
    }
    assert!(
        preview_with_hooks(
            &authority,
            &request(&root, OutputPreviewAction::Read, "link.txt"),
            None
        )
        .is_err()
    );
}

#[test]
fn unsupported_and_large_files_are_honest_and_unversioned() {
    let (_temp, root, authority) = project();
    fs::write(root.join("a.pdf"), b"<script>not a PDF</script>").unwrap();
    fs::write(root.join("a.docx"), b"binary").unwrap();
    fs::write(root.join("a.txt"), [255, 0]).unwrap();
    fs::write(
        root.join("large.txt"),
        vec![b'x'; OUTPUT_TEXT_BYTE_LIMIT + 1],
    )
    .unwrap();
    fs::write(
        root.join("many-lines.diff"),
        "\n".repeat(OUTPUT_TEXT_LINE_LIMIT),
    )
    .unwrap();
    let large = fs::File::create(root.join("large.pdf")).unwrap();
    large.set_len(EDITOR_FILE_BYTE_LIMIT as u64 + 1).unwrap();
    for name in [
        "a.pdf",
        "a.docx",
        "a.txt",
        "large.txt",
        "large.pdf",
        "many-lines.diff",
    ] {
        let snapshot = read(&authority, &root, name);
        assert!(matches!(
            snapshot.content,
            OutputPreviewContent::Unsupported { .. }
        ));
        assert_eq!(snapshot.revision, None);
    }
}

#[test]
fn roots_traversal_and_sensitive_root_selection_never_grant_access() {
    let (_temp, root, _authority) = project();
    fs::write(root.join("a.txt"), "test").unwrap();
    assert!(read_output_preview(&request(&root, OutputPreviewAction::Read, "a.txt"), &[]).is_err());
    assert!(
        read_output_preview(
            &request(&root, OutputPreviewAction::Read, "../a.txt"),
            &[root.clone()]
        )
        .is_err()
    );
    let secret = root.join(".ssh");
    fs::create_dir(&secret).unwrap();
    fs::write(secret.join("config"), "fixture").unwrap();
    assert_eq!(
        read_output_preview(
            &request(&secret, OutputPreviewAction::Read, "config"),
            &[secret.clone()]
        ),
        Err(WorkspaceEditorError::ProtectedPath)
    );
}

#[test]
fn only_a_root_containing_the_target_authorizes_a_read() {
    let (_temp, root, _authority) = project();
    let (_other_temp, other, _) = project();
    fs::write(root.join("a.txt"), "inside").unwrap();
    let read = request(&root, OutputPreviewAction::Read, "a.txt");
    assert_eq!(
        read_output_preview(&read, &[other.clone()]),
        Err(WorkspaceEditorError::OutsideAuthorizedRoot)
    );
    let Ok(OutputPreviewResponse::File { snapshot, .. }) =
        read_output_preview(&read, &[other, root.clone()])
    else {
        panic!("file expected")
    };
    assert_eq!(snapshot.byte_length, 6);
}

#[test]
fn exact_file_grant_reads_only_that_attachment_outside_project_roots() {
    let (_temp, desktop_dir, _authority) = project();
    let attached = desktop_dir.join("spec.md");
    let sibling = desktop_dir.join("other.md");
    let secret = desktop_dir.join(".env");
    fs::write(&attached, "# Spec").unwrap();
    fs::write(&sibling, "# Other").unwrap();
    fs::write(&secret, "SECRET=1").unwrap();

    let Ok(OutputPreviewResponse::File { snapshot, .. }) = read_output_preview_with_files(
        &request(&desktop_dir, OutputPreviewAction::Read, "spec.md"),
        &[],
        std::slice::from_ref(&attached),
    ) else {
        panic!("expected attached file preview");
    };
    assert_eq!(
        snapshot.content,
        OutputPreviewContent::Text {
            text: "# Spec".into(),
            format: OutputTextFormat::Markdown,
        }
    );
    assert_eq!(
        read_output_preview_with_files(
            &request(&desktop_dir, OutputPreviewAction::Read, "other.md"),
            &[],
            std::slice::from_ref(&attached),
        ),
        Err(WorkspaceEditorError::OutsideAuthorizedRoot)
    );
    assert_eq!(
        read_output_preview_with_files(
            &request(&desktop_dir, OutputPreviewAction::List, ""),
            &[],
            std::slice::from_ref(&attached),
        ),
        Err(WorkspaceEditorError::OutsideAuthorizedRoot)
    );
    assert_eq!(
        read_output_preview_with_files(
            &request(&desktop_dir, OutputPreviewAction::Read, ".env"),
            &[],
            &[secret],
        ),
        Err(WorkspaceEditorError::ProtectedPath)
    );
}

struct Swap {
    parent: PathBuf,
    outside: PathBuf,
    fired: AtomicBool,
}
impl EditorHooks for Swap {
    fn after_snapshot_validated(&self, _file: &Path) {
        if !self.fired.swap(true, Ordering::SeqCst) {
            fs::rename(&self.parent, self.parent.with_extension("parked")).unwrap();
            symlink(&self.outside, &self.parent).unwrap();
        }
    }
}
#[test]
fn binary_reads_reject_a_swapped_ancestor() {
    let (_temp, root, authority) = project();
    let (_outside_temp, outside, _) = project();
    let parent = root.join("outputs");
    fs::create_dir(&parent).unwrap();
    fs::write(parent.join("a.pdf"), b"%PDF-inside").unwrap();
    fs::write(outside.join("a.pdf"), b"%PDF-outside").unwrap();
    let hook = Swap {
        parent,
        outside,
        fired: AtomicBool::new(false),
    };
    assert_eq!(
        preview_with_hooks(
            &authority,
            &request(&root, OutputPreviewAction::Read, "outputs/a.pdf"),
            Some(&hook)
        ),
        Err(WorkspaceEditorError::PathChanged)
    );
}
