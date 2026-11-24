use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::DesktopServiceError;

/// Creates one new child of the owner's Projects folder, `projects` (named
/// `label` in errors), creating that folder when it is missing. Never adopts
/// an existing directory, file or symlink, and never invokes a shell or Git.
pub(crate) fn create_project(
    projects: &Path,
    label: &str,
    name: &str,
) -> Result<PathBuf, DesktopServiceError> {
    if name.is_empty()
        || name.trim() != name
        || name.len() > 255
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
    {
        return Err(DesktopServiceError::refused(
            "Choose a valid project name.",
            "Use a single folder name without slashes, colons or control characters (up to 255 bytes).",
        ));
    }
    fs::create_dir_all(projects).map_err(|_| {
        DesktopServiceError::new(
            "project-directory-unavailable",
            format!(
                "Could not open or create your Projects folder, {label}. Check its permissions."
            ),
            false,
        )
    })?;
    let parent = fs::canonicalize(projects).map_err(|_| {
        DesktopServiceError::new(
            "project-directory-unavailable",
            "Could not resolve your Projects folder.",
            false,
        )
    })?;
    let project = parent.join(name);
    // create_dir is exclusive, including when the existing entry is a symlink.
    fs::create_dir(&project).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            DesktopServiceError::refused(
                "That project name is already in use.",
                "Choose another name, or open the existing folder in a new workspace.",
            )
        } else {
            DesktopServiceError::new(
                "project-create-failed",
                "Could not create the project folder. Check its permissions.",
                false,
            )
        }
    })?;
    if fs::canonicalize(projects).ok().as_ref() != Some(&parent)
        || fs::canonicalize(&project).ok().as_ref() != Some(&project)
    {
        return Err(DesktopServiceError::new(
            "project-directory-changed",
            "The Projects folder changed during creation. Check the folder before trying again.",
            false,
        )
        .outcome_unknown());
    }
    Ok(project)
}
