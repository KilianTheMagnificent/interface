use std::collections::BTreeSet;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use interface_common::paths::lexically_absolute;

use std::os::unix::fs::MetadataExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContextPathKind {
    File,
    Folder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContextPathError {
    InvalidRequest,
    TargetMissing,
    TargetType,
    SymbolicLink,
    OutsideAuthorizedRoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

fn identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

pub(crate) fn lexical_absolute(path: &Path) -> Result<PathBuf, ContextPathError> {
    if path.as_os_str().is_empty() {
        return Err(ContextPathError::InvalidRequest);
    }
    let normalized = lexically_absolute(path).map_err(|_| ContextPathError::TargetMissing)?;
    if normalized.is_absolute() {
        Ok(normalized)
    } else {
        Err(ContextPathError::TargetMissing)
    }
}

pub(crate) fn inside(root: &Path, target: &Path) -> bool {
    target.starts_with(root)
}

pub(crate) fn canonical_launch_root(requested: &Path) -> Result<PathBuf, ContextPathError> {
    let resolved = lexical_absolute(requested)?;
    let before = fs::symlink_metadata(&resolved).map_err(|_| ContextPathError::TargetMissing)?;
    if before.file_type().is_symlink() {
        return Err(ContextPathError::SymbolicLink);
    }
    if !before.is_dir() {
        return Err(ContextPathError::TargetType);
    }
    let canonical = fs::canonicalize(&resolved).map_err(|_| ContextPathError::TargetMissing)?;
    let after = fs::symlink_metadata(&canonical).map_err(|_| ContextPathError::TargetMissing)?;
    if canonical != resolved
        || after.file_type().is_symlink()
        || !after.is_dir()
        || identity(&before) != identity(&after)
    {
        return Err(ContextPathError::SymbolicLink);
    }
    Ok(canonical)
}

pub(crate) fn grant_candidate(
    requested: &Path,
) -> Result<(PathBuf, ContextPathKind), ContextPathError> {
    let resolved = lexical_absolute(requested)?;
    let before = fs::symlink_metadata(&resolved).map_err(|_| ContextPathError::TargetMissing)?;
    if before.file_type().is_symlink() {
        return Err(ContextPathError::SymbolicLink);
    }
    let canonical = fs::canonicalize(&resolved).map_err(|_| ContextPathError::TargetMissing)?;
    let after = fs::symlink_metadata(&canonical).map_err(|_| ContextPathError::TargetMissing)?;
    if after.file_type().is_symlink() || identity(&before) != identity(&after) {
        return Err(ContextPathError::SymbolicLink);
    }
    let kind = if after.is_dir() {
        ContextPathKind::Folder
    } else if after.is_file() {
        ContextPathKind::File
    } else {
        return Err(ContextPathError::TargetType);
    };
    Ok((canonical, kind))
}

pub(crate) fn stable_context_path(
    requested: &Path,
    kind: ContextPathKind,
    authorized_roots: &[PathBuf],
    exact_grant: &dyn Fn(&Path) -> bool,
    exact_launch_root: Option<&Path>,
) -> Result<PathBuf, ContextPathError> {
    let resolved = lexical_absolute(requested)?;
    let before = fs::symlink_metadata(&resolved).map_err(|_| ContextPathError::TargetMissing)?;
    if before.file_type().is_symlink() {
        return Err(ContextPathError::SymbolicLink);
    }

    let canonical = resolve_lexical_in_roots(&resolved, authorized_roots).or_else(|_| {
        let canonical = fs::canonicalize(&resolved).map_err(|_| ContextPathError::TargetMissing)?;
        let permitted_by_launch = exact_launch_root.is_some_and(|root| inside(root, &canonical));
        if permitted_by_launch || exact_grant(&canonical) {
            Ok(canonical)
        } else {
            Err(ContextPathError::OutsideAuthorizedRoot)
        }
    })?;

    let after = fs::symlink_metadata(&canonical).map_err(|_| ContextPathError::TargetMissing)?;
    if canonical != resolved
        || after.file_type().is_symlink()
        || identity(&before) != identity(&after)
    {
        return Err(ContextPathError::SymbolicLink);
    }
    match kind {
        ContextPathKind::File if !after.is_file() => Err(ContextPathError::TargetType),
        ContextPathKind::Folder if !after.is_dir() => Err(ContextPathError::TargetType),
        ContextPathKind::File | ContextPathKind::Folder => Ok(canonical),
    }
}

pub(crate) fn resolve_canonical_in_roots(
    requested: &Path,
    authorized_roots: &[PathBuf],
) -> Result<PathBuf, ContextPathError> {
    resolve_lexical_in_roots(&lexical_absolute(requested)?, authorized_roots)
}

/// Admits an already lexical-absolute path. The lexical test runs first and
/// needs no syscall; only then is each distinct root canonicalized, and only
/// until one contains the canonical target.
fn resolve_lexical_in_roots(
    lexical: &Path,
    authorized_roots: &[PathBuf],
) -> Result<PathBuf, ContextPathError> {
    let lexical_roots = authorized_roots
        .iter()
        .filter_map(|root| lexical_absolute(root).ok())
        .collect::<BTreeSet<_>>();
    if !lexical_roots.iter().any(|root| inside(root, lexical)) {
        return Err(ContextPathError::OutsideAuthorizedRoot);
    }
    let canonical = fs::canonicalize(lexical).map_err(|_| ContextPathError::TargetMissing)?;
    if lexical_roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| inside(&root, &canonical))
    {
        Ok(canonical)
    } else {
        Err(ContextPathError::OutsideAuthorizedRoot)
    }
}

pub(crate) fn relative_slash(root: &Path, target: &Path) -> String {
    pathdiff(root, target).to_string_lossy().into_owned()
}

fn pathdiff(root: &Path, target: &Path) -> PathBuf {
    let root_parts = root.components().collect::<Vec<_>>();
    let target_parts = target.components().collect::<Vec<_>>();
    let shared = root_parts
        .iter()
        .zip(&target_parts)
        .take_while(|(left, right)| left == right)
        .count();
    let mut result = PathBuf::new();
    for _ in shared..root_parts.len() {
        result.push("..");
    }
    for component in &target_parts[shared..] {
        result.push(component.as_os_str());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_containment_does_not_confuse_sibling_prefixes() {
        assert!(inside(Path::new("/work"), Path::new("/work")));
        assert!(inside(Path::new("/work"), Path::new("/work/src")));
        assert!(!inside(Path::new("/work"), Path::new("/work-secret")));
    }

    #[test]
    fn relative_paths_match_node_path_relative_shape() {
        assert_eq!(
            relative_slash(Path::new("/a/b"), Path::new("/a/c/x")),
            "../c/x"
        );
    }
}
