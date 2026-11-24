use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use interface_common::paths::lexically_normalized;

use thiserror::Error;

use crate::error::TerminalError;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectorySnapshot {
    pub path: PathBuf,
    identity: DirectoryIdentity,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ApprovedTerminalRootError {
    #[error("the terminal cwd is not a stable canonical directory")]
    Unsafe,
    #[error("the terminal cwd is not an exact approved root")]
    Unauthorized,
}

pub(crate) fn root_error(error: ApprovedTerminalRootError) -> TerminalError {
    match error {
        ApprovedTerminalRootError::Unsafe => TerminalError::UnsafeCwd,
        ApprovedTerminalRootError::Unauthorized => TerminalError::UnauthorizedCwd,
    }
}

#[cfg(unix)]
fn identity(metadata: &Metadata) -> DirectoryIdentity {
    DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn identity(metadata: &Metadata) -> DirectoryIdentity {
    use std::time::SystemTime;

    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |value| {
            u64::try_from(value.as_nanos()).unwrap_or(u64::MAX)
        });
    DirectoryIdentity {
        device: metadata.len(),
        inode: modified,
    }
}

fn normalized_absolute(path: &Path) -> Result<PathBuf, ApprovedTerminalRootError> {
    let source = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| ApprovedTerminalRootError::Unsafe)?
            .join(path)
    };
    Ok(lexically_normalized(&source))
}

pub(crate) fn snapshot_directory(
    path: &Path,
) -> Result<DirectorySnapshot, ApprovedTerminalRootError> {
    let requested = normalized_absolute(path)?;
    let before = fs::symlink_metadata(&requested).map_err(|_| ApprovedTerminalRootError::Unsafe)?;
    if !before.is_dir() || before.file_type().is_symlink() {
        return Err(ApprovedTerminalRootError::Unsafe);
    }
    let canonical = fs::canonicalize(&requested).map_err(|_| ApprovedTerminalRootError::Unsafe)?;
    let after = fs::symlink_metadata(&canonical).map_err(|_| ApprovedTerminalRootError::Unsafe)?;
    if canonical != requested
        || !after.is_dir()
        || after.file_type().is_symlink()
        || identity(&before) != identity(&after)
    {
        return Err(ApprovedTerminalRootError::Unsafe);
    }
    Ok(DirectorySnapshot {
        path: canonical,
        identity: identity(&after),
    })
}

pub(crate) fn assert_directory(
    snapshot: &DirectorySnapshot,
) -> Result<(), ApprovedTerminalRootError> {
    let current = snapshot_directory(&snapshot.path)?;
    if current.identity != snapshot.identity {
        return Err(ApprovedTerminalRootError::Unsafe);
    }
    Ok(())
}

/// A point-in-time allowlist of exact canonical workspace/session roots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedTerminalRoots {
    roots: Vec<DirectorySnapshot>,
}

impl ApprovedTerminalRoots {
    /// Pin exact approved roots by device and inode.
    ///
    /// # Errors
    ///
    /// Returns an error when any root is missing, symbolic, non-canonical, or
    /// not a directory. An empty list grants nothing.
    pub fn new(
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, ApprovedTerminalRootError> {
        let mut roots = Vec::new();
        for path in paths {
            let root = snapshot_directory(&path)?;
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        Ok(Self { roots })
    }

    /// Returns the exact approved root that owns `target`, accepting either the
    /// root itself or a stable canonical descendant. This exposes only the
    /// already-pinned host allowlist; it does not add authority.
    ///
    /// # Errors
    ///
    /// Returns [`ApprovedTerminalRootError::Unauthorized`] when no approved
    /// root contains the target, or [`ApprovedTerminalRootError::Unsafe`] when
    /// the target/root identity cannot be revalidated.
    pub fn owning_root(&self, target: &Path) -> Result<PathBuf, ApprovedTerminalRootError> {
        self.authorize_pass_off(target)
            .map(|authorized| authorized.root.path)
    }

    /// Returns the exact approved root that a link at `target` resolves to,
    /// such as the symlink a moved project leaves at its old path. Approved
    /// roots are pinned from canonicalized candidates, so the old spelling
    /// names a root this set already holds; resolving it grants nothing new.
    /// A canonical path, a link into a subfolder or outside every root, and an
    /// unresolvable path all return `None`.
    #[must_use]
    pub fn resolve_linked_root(&self, target: &Path) -> Option<PathBuf> {
        let requested = normalized_absolute(target).ok()?;
        let canonical = fs::canonicalize(&requested).ok()?;
        if canonical == requested {
            return None;
        }
        self.authorize(&canonical).ok().map(|root| root.path)
    }

    pub(crate) fn authorize(
        &self,
        target: &Path,
    ) -> Result<DirectorySnapshot, ApprovedTerminalRootError> {
        let candidate = snapshot_directory(target)?;
        let Some(approved) = self.roots.iter().find(|approved| **approved == candidate) else {
            return Err(ApprovedTerminalRootError::Unauthorized);
        };
        assert_directory(approved)?;
        Ok(candidate)
    }

    /// Pass-off panes may start in the source process's live cwd. That path is
    /// host-derived (never renderer-named), so the rule is deliberately wider
    /// than `authorize`: the same safety snapshot, accepted when the directory
    /// is an approved root or inside one.
    ///
    /// The containing root comes back with it. Everything downstream that
    /// protects a workspace — the cleanup lease, the summary's `root`, and a
    /// later reopen — is keyed on that root, never on wherever inside it a
    /// pane happens to start.
    pub(crate) fn authorize_pass_off(
        &self,
        target: &Path,
    ) -> Result<AuthorizedDirectory, ApprovedTerminalRootError> {
        let candidate = snapshot_directory(target)?;
        // Longest match wins. A BTreeSet of approved paths puts "/" first, and
        // every Unix path starts with it, so first-match would assign the whole
        // filesystem to a slash-command session whose cwd happened to be "/".
        let mut owner: Option<&DirectorySnapshot> = None;
        for approved in &self.roots {
            if *approved != candidate && !candidate.path.starts_with(&approved.path) {
                continue;
            }
            if owner.is_none_or(|current| {
                approved.path.as_os_str().len() > current.path.as_os_str().len()
            }) {
                owner = Some(approved);
            }
        }
        let Some(approved) = owner else {
            return Err(ApprovedTerminalRootError::Unauthorized);
        };
        assert_directory(approved)?;
        Ok(AuthorizedDirectory {
            root: approved.clone(),
            cwd: candidate,
        })
    }

    /// Authorizes a saved cwd against one exact root selected by durable host
    /// state. Unlike pass-off authorization, the owning root is explicit, so
    /// a renderer-authored path cannot choose a different approved workspace.
    pub(crate) fn authorize_retained(
        &self,
        target: &Path,
        root: &Path,
    ) -> Result<AuthorizedDirectory, ApprovedTerminalRootError> {
        let root = self.authorize(root)?;
        let cwd = snapshot_directory(target)?;
        if cwd != root && !cwd.path.starts_with(&root.path) {
            return Err(ApprovedTerminalRootError::Unauthorized);
        }
        assert_directory(&root)?;
        Ok(AuthorizedDirectory { root, cwd })
    }
}

/// An authorized starting directory together with the approved root that owns
/// it. For an exact authorization the two are the same directory; a pass-off
/// pane may start inside its root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthorizedDirectory {
    pub root: DirectorySnapshot,
    pub cwd: DirectorySnapshot,
}

impl AuthorizedDirectory {
    /// The ordinary case: a pane whose working directory is its own root.
    pub(crate) fn exact(directory: DirectorySnapshot) -> Self {
        Self {
            root: directory.clone(),
            cwd: directory,
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// Temporary roots live beside the crate so their paths are already
    /// canonical; the platform temp directory is a symlink on macOS, which
    /// `snapshot_directory` rejects.
    fn temp_dir() -> TempDir {
        tempfile::Builder::new()
            .prefix("interface-terminal-safety-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("temporary directory")
    }

    #[test]
    fn pass_off_authorizes_directories_inside_a_root_but_not_outside() {
        let root = temp_dir();
        let nested = root.path().join("wt");
        std::fs::create_dir(&nested).expect("nested");
        let outside = temp_dir();
        let approved = ApprovedTerminalRoots::new([root.path().to_path_buf()]).expect("approved");
        // The exact rule still refuses the nested directory…
        assert!(approved.authorize(&nested).is_err());
        // …the pass-off rule accepts the root itself and directories inside it,
        // always naming the root that owns the directory it accepted…
        let exact = approved.authorize_pass_off(root.path()).expect("root");
        assert_eq!(exact.root.path, root.path());
        assert_eq!(exact.cwd.path, root.path());
        let inside = approved.authorize_pass_off(&nested).expect("nested");
        assert_eq!(inside.root.path, root.path());
        assert_eq!(inside.cwd.path, nested);
        // …and still refuses anything outside.
        assert_eq!(
            approved.authorize_pass_off(outside.path()),
            Err(ApprovedTerminalRootError::Unauthorized)
        );
    }

    #[test]
    fn pass_off_prefers_the_deepest_approved_root() {
        let parent = temp_dir();
        let child = parent.path().join("project");
        std::fs::create_dir(&child).expect("child project");
        let nested = child.join("src");
        std::fs::create_dir(&nested).expect("nested cwd");
        // Parent is inserted first, the same order a BTreeSet would yield for
        // "/" vs a real project: shortest path first. First-match would steal
        // every descendant for the parent.
        let approved = ApprovedTerminalRoots::new([parent.path().to_path_buf(), child.clone()])
            .expect("approved roots");
        assert_eq!(approved.owning_root(&nested).expect("nested owner"), child);
        assert_eq!(approved.owning_root(&child).expect("child owner"), child);
        assert_eq!(
            approved.owning_root(parent.path()).expect("parent owner"),
            parent.path()
        );
    }

    #[cfg(unix)]
    #[test]
    fn pass_off_does_not_let_the_filesystem_root_own_every_path() {
        let project = temp_dir();
        let nested = project.path().join("src");
        std::fs::create_dir(&nested).expect("nested cwd");
        let approved =
            ApprovedTerminalRoots::new([PathBuf::from("/"), project.path().to_path_buf()])
                .expect("filesystem root and project");
        assert_eq!(
            approved.owning_root(&nested).expect("project owner"),
            project.path()
        );
        assert_eq!(
            approved.owning_root(project.path()).expect("exact project"),
            project.path()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_resolves_only_to_the_exact_approved_root_it_names() {
        let project = temp_dir();
        let nested = project.path().join("src");
        std::fs::create_dir(&nested).expect("nested directory");
        let unapproved = temp_dir();
        let links = temp_dir();
        let moved = links.path().join("moved-project");
        std::os::unix::fs::symlink(project.path(), &moved).expect("root link");
        let into_child = links.path().join("child");
        std::os::unix::fs::symlink(&nested, &into_child).expect("child link");
        let elsewhere = links.path().join("elsewhere");
        std::os::unix::fs::symlink(unapproved.path(), &elsewhere).expect("outside link");
        let approved =
            ApprovedTerminalRoots::new([project.path().to_path_buf()]).expect("approved");

        // The exact rule refuses the old spelling outright…
        assert_eq!(
            approved.authorize(&moved),
            Err(ApprovedTerminalRootError::Unsafe)
        );
        // …the resolver names the approved root it links to…
        assert_eq!(
            approved.resolve_linked_root(&moved).as_deref(),
            Some(project.path())
        );
        // …and nothing else: a canonical path, a link into a subfolder, a link
        // outside every root, and a missing path all stay unresolved.
        assert_eq!(approved.resolve_linked_root(project.path()), None);
        assert_eq!(approved.resolve_linked_root(&into_child), None);
        assert_eq!(approved.resolve_linked_root(&elsewhere), None);
        assert_eq!(
            approved.resolve_linked_root(&links.path().join("missing")),
            None
        );
    }

    #[test]
    fn retained_authority_is_bound_to_the_explicit_host_selected_root() {
        let root = temp_dir();
        let nested = root.path().join("src/deeper");
        std::fs::create_dir_all(&nested).expect("nested retained cwd");
        let other = temp_dir();
        let approved =
            ApprovedTerminalRoots::new([root.path().to_path_buf(), other.path().to_path_buf()])
                .expect("approved roots");

        let retained = approved
            .authorize_retained(&nested, root.path())
            .expect("retained descendant");
        assert_eq!(retained.root.path, root.path());
        assert_eq!(retained.cwd.path, nested);
        assert_eq!(
            approved.authorize_retained(other.path(), root.path()),
            Err(ApprovedTerminalRootError::Unauthorized)
        );
    }
}
