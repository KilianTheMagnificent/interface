use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::SettingsError;

const FILE_PROVIDER_XATTR: &str = "com.apple.file-provider-domain-id";

fn existing_directory(candidate: &Path) -> Option<PathBuf> {
    let metadata = fs::metadata(candidate).ok()?;
    metadata
        .is_dir()
        .then(|| fs::canonicalize(candidate).ok())
        .flatten()
}

/// Prefers the canonical `Desktop/Projects` collection and falls back to the
/// canonical home directory.
///
/// # Errors
///
/// Returns [`SettingsError::InvalidHome`] when neither is a real directory.
pub fn resolve_default_directory(home: &Path) -> Result<PathBuf, SettingsError> {
    let absolute_home = std::path::absolute(home)?;
    if let Some(projects) = existing_directory(&absolute_home.join("Desktop/Projects")) {
        return Ok(projects);
    }
    existing_directory(&absolute_home).ok_or(SettingsError::InvalidHome)
}

/// Production File Provider probe using the fixed macOS xattr binary and no
/// shell. Failure means the attribute is absent/unavailable.
#[must_use]
pub fn is_file_provider_managed(target: &Path) -> bool {
    let start = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    start.ancestors().any(has_file_provider_marker)
}

/// Whether this exact path carries the File Provider domain marker. One
/// `/usr/bin/xattr` spawn; its ancestors are not checked.
fn has_file_provider_marker(candidate: &Path) -> bool {
    Command::new("/usr/bin/xattr")
        .args(["-p", FILE_PROVIDER_XATTR])
        .arg(candidate)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Avoids opening a native picker inside an iCloud/File Provider tree.
#[must_use]
pub fn directory_picker_start_path(preferred: &Path, home: &Path) -> PathBuf {
    // The picker walk already visits every ancestor, so each step probes only
    // its own path: one spawn per level instead of one per level per ancestor.
    directory_picker_start_path_with(preferred, home, has_file_provider_marker)
}

#[must_use]
pub fn directory_picker_start_path_with<F>(preferred: &Path, home: &Path, probe: F) -> PathBuf
where
    F: FnMut(&Path) -> bool,
{
    let start = std::path::absolute(preferred).unwrap_or_else(|_| preferred.to_path_buf());
    if start.ancestors().any(probe) {
        std::path::absolute(home).unwrap_or_else(|_| home.to_path_buf())
    } else {
        start
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{directory_picker_start_path_with, resolve_default_directory};

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[cfg(unix)]
    #[test]
    fn default_directory_prefers_canonical_projects_and_falls_back_home() {
        let scratch = TempDir::new().expect("scratch");
        let home = scratch.path().join("home");
        let actual = scratch.path().join("projects");
        fs::create_dir_all(home.join("Desktop")).expect("desktop");
        fs::create_dir(&actual).expect("projects");
        symlink(&actual, home.join("Desktop/Projects")).expect("projects link");
        assert_eq!(
            resolve_default_directory(&home).expect("resolve projects"),
            fs::canonicalize(&actual).expect("canonical actual")
        );
        fs::remove_file(home.join("Desktop/Projects")).expect("remove link");
        fs::write(home.join("Desktop/Projects"), "not a directory").expect("file fixture");
        assert_eq!(
            resolve_default_directory(&home).expect("home fallback"),
            fs::canonicalize(&home).expect("canonical home")
        );
    }

    #[test]
    fn picker_walks_ancestors_for_file_provider_marker() {
        let home = TempDir::new().expect("home");
        let desktop = home.path().join("Desktop");
        let projects = desktop.join("Projects");
        fs::create_dir_all(&projects).expect("projects");
        assert_eq!(
            directory_picker_start_path_with(&projects, home.path(), |candidate| {
                candidate == desktop
            }),
            std::path::absolute(home.path()).expect("absolute home")
        );
        assert_eq!(
            directory_picker_start_path_with(&projects, home.path(), |_| false),
            std::path::absolute(&projects).expect("absolute projects")
        );
    }

    #[test]
    fn picker_probes_each_ancestor_exactly_once() {
        let home = TempDir::new().expect("home");
        let deep = home.path().join("a/b/c");
        let absolute = std::path::absolute(&deep).expect("absolute deep");
        let mut probed = Vec::new();
        assert_eq!(
            directory_picker_start_path_with(&deep, home.path(), |candidate| {
                probed.push(candidate.to_path_buf());
                false
            }),
            absolute
        );
        assert_eq!(
            probed,
            absolute
                .ancestors()
                .map(std::path::Path::to_path_buf)
                .collect::<Vec<_>>()
        );
    }
}
