use super::{
    Change, Contents, Directory, MAX_FILE_BYTES, MAX_RESULT_BYTES, assert_directory, digest,
    directory, read_file, validate_names, write_file,
};
use crate::editor::WorkspaceAuthority;
use serde_json::{Value, json};

/// Explicitly applies parent-reviewed UTF-8 proposals. Preflight covers the
/// whole packet before any write; individual replacements compare-and-save.
/// An error after a write is an uncertain multi-file outcome, never replayable.
/// Deletion and permission changes remain deliberate parent operations.
#[must_use]
pub fn apply_changes(source: &std::path::Path, changes: &[Change]) -> Value {
    let prepared = preflight(source, changes);
    let (root, authority) = match prepared {
        Ok(value) => value,
        Err(error) => {
            return json!({"version":1,"state":"conflict","files":[],"error":error,"replaySafe":true});
        }
    };
    let mut applied = Vec::new();
    for change in changes {
        let result = apply_one(&root, &authority, change);
        match result {
            Ok(()) => applied.push(
                json!({"path":change.path,"sha256":digest(change.after.as_deref().unwrap_or(""))}),
            ),
            Err(error) => {
                return json!({"version":1,"state":"uncertain","files":applied,"error":error,"replaySafe":false});
            }
        }
    }
    json!({"version":1,"state":"applied","files":applied,"replaySafe":false})
}

fn preflight(
    source: &std::path::Path,
    changes: &[Change],
) -> Result<(Directory, WorkspaceAuthority), String> {
    if changes.is_empty() || changes.len() > 16 {
        return Err("Apply requires 1-16 reviewed files".into());
    }
    let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
    validate_names(&paths, &[])?;
    let root = directory(source)?;
    let authority = WorkspaceAuthority::new();
    authority.grant_root(source).map_err(|e| e.to_string())?;
    let mut bytes = 0;
    for change in changes {
        let text = change
            .after
            .as_deref()
            .ok_or("Deletion requires deliberate parent integration")?;
        bytes += text.len();
        if text.len() > MAX_FILE_BYTES || bytes > MAX_RESULT_BYTES || text.contains('\0') {
            return Err("Reviewed proposal exceeds the UTF-8 text bounds".into());
        }
        if change.after_mode != change.before_mode && change.before_mode.is_some() {
            return Err("Permission changes require deliberate parent integration".into());
        }
        if change.before_mode.is_none() && !matches!(change.after_mode, Some(0o600 | 0o644)) {
            return Err("New delegated files must have ordinary non-executable permissions".into());
        }
        compare_current(&root, change)?;
    }
    assert_directory(&root)?;
    Ok((root, authority))
}

fn compare_current(root: &Directory, change: &Change) -> Result<(), String> {
    let current = read_file(root, &change.path)?;
    if current.as_ref().map(|c| digest(&c.text)) != change.before_sha256
        || current.as_ref().map(|c| c.mode) != change.before_mode
    {
        return Err(format!(
            "Source changed since delegation: {}. Review and integrate it locally.",
            change.path
        ));
    }
    Ok(())
}

fn apply_one(
    root: &Directory,
    authority: &WorkspaceAuthority,
    change: &Change,
) -> Result<(), String> {
    compare_current(root, change)?;
    let text = change.after.as_deref().ok_or("Missing reviewed content")?;
    if let Some(expected) = &change.before_sha256 {
        authority
            .save_text(&root.path.join(&change.path), text, expected)
            .map_err(|e| e.to_string())?;
    } else {
        // Parents must already exist; this does not authorize a directory tree.
        let mode = change.after_mode.ok_or("Missing reviewed file mode")?;
        let contents = Contents {
            text: text.into(),
            mode,
        };
        write_file(root, &change.path, &contents, mode)?;
    }
    assert_directory(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn applies_reviewed_bytes_and_refuses_stale_batch_before_any_write() {
        let root = tempfile::tempdir().unwrap();
        let source = fs::canonicalize(root.path()).unwrap();
        fs::write(source.join("one.rs"), "old").unwrap();
        fs::set_permissions(source.join("one.rs"), fs::Permissions::from_mode(0o644)).unwrap();
        let change = Change {
            path: "one.rs".into(),
            before: Some("old".into()),
            before_sha256: Some(digest("old")),
            after: Some("new".into()),
            source_changed: false,
            before_mode: Some(0o644),
            after_mode: Some(0o644),
        };
        let mut stale = change.clone();
        stale.path = "two.rs".into();
        fs::write(source.join("two.rs"), "concurrent work").unwrap();
        assert_eq!(
            apply_changes(&source, &[change.clone(), stale])["state"],
            "conflict"
        );
        assert_eq!(fs::read_to_string(source.join("one.rs")).unwrap(), "old");
        assert_eq!(apply_changes(&source, &[change])["state"], "applied");
        assert_eq!(fs::read_to_string(source.join("one.rs")).unwrap(), "new");
        let create = Change {
            path: "new.rs".into(),
            before: None,
            before_sha256: None,
            after: Some("created".into()),
            source_changed: false,
            before_mode: None,
            after_mode: Some(0o644),
        };
        assert_eq!(
            apply_changes(&source, &[create.clone()])["state"],
            "applied"
        );
        assert_eq!(apply_changes(&source, &[create])["state"], "conflict");
    }
}

#[cfg(test)]
#[path = "delegation_apply_acceptance.rs"]
mod acceptance;
