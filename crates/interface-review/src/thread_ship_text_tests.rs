use interface_contracts::threads::{DiffFile, DiffFileStatus, DiffOmitted, DiffScope, ThreadDiff};

use super::*;

fn file(name: &str, patch: Option<&str>, omitted: Option<DiffOmitted>) -> DiffFile {
    DiffFile {
        path: name.into(),
        old_path: None,
        status: DiffFileStatus::Modified,
        added: 3,
        removed: 1,
        binary: omitted == Some(DiffOmitted::Binary),
        patch: patch.map(str::to_owned),
        omitted,
    }
}

fn diff(files: Vec<DiffFile>) -> ThreadDiff {
    ThreadDiff {
        scope: DiffScope::Uncommitted,
        base: "HEAD".into(),
        head: "working tree".into(),
        files,
        truncated: false,
    }
}

#[test]
fn the_material_names_every_file_but_reads_only_ordinary_ones() {
    let material = ship_text_material(
        &diff(vec![
            file(
                "src/login.rs",
                Some("@@ -1 +1 @@\n-old\n+new token=abc123secret\n"),
                None,
            ),
            file(".env", None, Some(DiffOmitted::Sensitive)),
            file("logo.png", None, Some(DiffOmitted::Binary)),
        ]),
        &["Fix the login\ntest".into(), "  ".into()],
    );
    assert!(material.starts_with("<changes>\n M src/login.rs | +3 -1\n"));
    assert!(material.contains(" M .env (credential-shaped: content left out)\n"));
    assert!(material.contains(" M logo.png (binary)\n"));
    assert!(material.contains("<patch>\n@@ -1 +1 @@\n-old\n+new token=[redacted]\n</patch>"));
    assert!(!material.contains("abc123secret"), "{material}");
    assert!(material.ends_with("<prompts>\n- Fix the login test\n</prompts>\n"));
}

#[test]
fn the_patch_and_the_whole_material_keep_to_their_bounds() {
    let big = format!("+{}\n", "x".repeat(20_000));
    let files = (0..80)
        .map(|index| file(&format!("src/file{index}.rs"), Some(&big), None))
        .collect();
    let prompts = (0..200)
        .map(|index| format!("prompt number {index} {}", "y".repeat(500)))
        .collect::<Vec<_>>();
    let material = ship_text_material(&diff(files), &prompts);
    assert!(material.len() <= MATERIAL_BYTES, "{}", material.len());
    assert!(material.contains(" … and 20 more files\n"));
    let patch = material
        .split("<patch>\n")
        .nth(1)
        .and_then(|rest| rest.split("\n</patch>").next())
        .unwrap();
    assert!(patch.len() <= PATCH_BYTES);
    // The newest prompts are the ones kept, oldest of them first.
    if let Some(prompts) = material.split("<prompts>\n").nth(1) {
        assert!(prompts.contains("prompt number 199"));
        assert!(!prompts.contains("prompt number 0 "));
    }
}

#[test]
fn nothing_changed_is_an_empty_stat_and_no_patch() {
    let material = ship_text_material(&diff(Vec::new()), &[]);
    assert_eq!(material, "<changes>\n</changes>\n");
}
