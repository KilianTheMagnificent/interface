//! Disposable, explicitly selected validation inputs. Never copies dependencies,
//! Git metadata or credentials, and never integrates check-generated files.
use super::{
    Change, Contents, Directory, add_bytes, apply_changes, assert_directory, create_directory,
    digest, directory, make_parents, read_file, validate_path, write_scratch_file,
};
use interface_contracts::delegate_validation::ValidationPlan;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    path: String,
    sha256: Option<String>,
    mode: Option<u32>,
}

pub struct ValidationSnapshot {
    source: Directory,
    workspace: Directory,
    inputs: Vec<Input>,
    candidate: BTreeMap<String, Option<Contents>>,
}

impl ValidationSnapshot {
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace.path
    }

    /// # Errors
    /// Rejects changes to source inputs or candidate inputs during checks.
    pub fn verify(&self) -> Result<(), String> {
        verify_inputs(&self.source.path, &self.inputs_value()?)?;
        assert_directory(&self.source)?;
        assert_directory(&self.workspace)?;
        for (path, expected) in &self.candidate {
            if &read_file(&self.workspace, path)? != expected {
                return Err(format!("Validation changed an input file: {path}"));
            }
        }
        Ok(())
    }

    /// # Errors
    /// Returns a serialization failure without claiming successful validation.
    pub fn inputs_value(&self) -> Result<Value, String> {
        serde_json::to_value(&self.inputs).map_err(|e| e.to_string())
    }
}

impl Drop for ValidationSnapshot {
    fn drop(&mut self) {
        // Only remove the exact private directory we created; never follow a
        // substituted root. remove_dir_all does not traverse symlink children.
        if assert_directory(&self.workspace).is_ok() {
            let _ = std::fs::remove_dir_all(&self.workspace.path);
        }
    }
}

/// # Errors
/// Refuses unsafe/oversized inputs, stale baselines and modified acceptance files.
pub fn prepare_validation(
    source: &Path,
    destination: &Path,
    changes: &[Change],
    plan: &ValidationPlan,
) -> Result<ValidationSnapshot, String> {
    plan.validate()?;
    let source = directory(source)?;
    let parent = directory(
        destination
            .parent()
            .ok_or("Validation destination requires a parent")?,
    )?;
    if !destination.is_absolute()
        || destination.starts_with(&source.path)
        || source.path.starts_with(destination)
    {
        return Err("Validation destination must be separate from source".into());
    }
    let mut paths: BTreeSet<String> = plan.files.iter().cloned().collect();
    let mut folded = BTreeSet::new();
    for change in changes {
        if plan
            .protected_files
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&change.path))
        {
            return Err(format!(
                "Proposal changes a parent-protected acceptance input: {}",
                change.path
            ));
        }
        paths.insert(change.path.clone());
    }
    for path in &paths {
        validate_path(path)?;
        if !folded.insert(path.to_lowercase()) {
            return Err("Validation paths alias on a case-insensitive filesystem".into());
        }
    }
    let mut inputs = Vec::new();
    let mut candidate = BTreeMap::new();
    let mut total = 0;
    for path in &paths {
        let before = read_file(&source, path)?;
        if plan.files.contains(path) && before.is_none() {
            return Err(format!("Validation input is missing: {path}"));
        }
        add_bytes(&mut total, before.as_ref(), 16 * 1024 * 1024)?;
        inputs.push(Input {
            path: path.clone(),
            sha256: before.as_ref().map(|c| digest(&c.text)),
            mode: before.as_ref().map(|c| c.mode),
        });
        candidate.insert(path.clone(), before);
    }
    assert_directory(&source)?;
    assert_directory(&parent)?;
    let workspace = create_directory(destination)?;
    let mut snapshot = ValidationSnapshot {
        source,
        workspace,
        inputs,
        candidate,
    };
    for (path, before) in &snapshot.candidate {
        make_parents(&snapshot.workspace, path)?;
        if let Some(contents) = before {
            write_scratch_file(&snapshot.workspace, path, contents, contents.mode)?;
        }
    }
    let applied = apply_changes(snapshot.workspace(), changes);
    if applied["state"] != "applied" {
        return Err(format!(
            "Validation proposal preflight failed: {}",
            applied["error"]
        ));
    }
    for change in changes {
        snapshot.candidate.insert(
            change.path.clone(),
            read_file(&snapshot.workspace, &change.path)?,
        );
    }
    snapshot.verify()?;
    Ok(snapshot)
}

/// # Errors
/// Refuses a stale source or unsafe receipt, including context-only changes.
pub fn verify_inputs(source: &Path, value: &Value) -> Result<(), String> {
    let inputs: Vec<Input> = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if inputs.is_empty() || inputs.len() > 528 {
        return Err("Invalid validation input receipt".into());
    }
    let root = directory(source)?;
    let mut total = 0;
    for input in inputs {
        validate_path(&input.path)?;
        let current = read_file(&root, &input.path)?;
        add_bytes(&mut total, current.as_ref(), 16 * 1024 * 1024)?;
        if current.as_ref().map(|c| digest(&c.text)) != input.sha256
            || current.as_ref().map(|c| c.mode) != input.mode
        {
            return Err(format!(
                "Validation is stale: {} changed; validate the current candidate again",
                input.path
            ));
        }
    }
    assert_directory(&root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn validation_isolated_and_fenced_by_protected_inputs_and_source_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("impl.js"), "old").unwrap();
        fs::write(source.join("test.js"), "assert correct").unwrap();
        fs::set_permissions(source.join("impl.js"), fs::Permissions::from_mode(0o644)).unwrap();
        let changes = vec![Change {
            path: "impl.js".into(),
            before: Some("old".into()),
            before_sha256: Some(digest("old")),
            after: Some("new".into()),
            source_changed: false,
            before_mode: Some(0o644),
            after_mode: Some(0o644),
        }];
        let mut plan = ValidationPlan {
            version: 1,
            files: vec!["test.js".into()],
            protected_files: vec!["test.js".into()],
            checks: vec![vec!["node".into(), "test.js".into()]],
        };
        let snapshot =
            prepare_validation(&source, &root.join("validation"), &changes, &plan).unwrap();
        assert_eq!(fs::read_to_string(source.join("impl.js")).unwrap(), "old");
        assert_eq!(
            fs::read_to_string(snapshot.workspace().join("impl.js")).unwrap(),
            "new"
        );
        let receipt = snapshot.inputs_value().unwrap();
        fs::write(source.join("test.js"), "new acceptance").unwrap();
        assert!(verify_inputs(&source, &receipt).is_err());
        assert!(snapshot.verify().is_err());
        plan.files.push("impl.js".into());
        plan.protected_files.push("impl.js".into());
        assert!(prepare_validation(&source, &root.join("protected"), &changes, &plan).is_err());
        plan.files.push(".env".into());
        assert!(prepare_validation(&source, &root.join("secret"), &changes, &plan).is_err());
        let path = snapshot.workspace().to_owned();
        drop(snapshot);
        assert!(!path.exists());
        assert!(source.join("impl.js").exists());
    }
}
