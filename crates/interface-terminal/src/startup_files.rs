//! Files a pane's program reads before its first paint.
//!
//! Claude, Codex and a git-aware shell prompt all read the project's git HEAD,
//! config and branch ref and its CLAUDE.md/AGENTS.md chain before drawing
//! anything. When a File Provider (iCloud Drive with Optimize Mac Storage)
//! has evicted them, each read blocks until the file is downloaded, one after
//! another, and the pane stays blank. A fetch asks for all of them at once,
//! off every service lock, and reports which are still in flight so the pane
//! can say why it is waiting.
use interface_common::bounded_read::is_dataless;
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use interface_common::paths::lexically_normalized;
use interface_common::sync::lock;
use interface_contracts::terminal::{
    TERMINAL_STARTUP_DOWNLOAD_NAMED_FILES, TERMINAL_STARTUP_DOWNLOAD_VERSION,
    TerminalStartupDownloadV1,
};

/// Read by every ancestor from the cwd up to (not including) the home folder.
const INSTRUCTION_FILES: [&str; 3] = ["CLAUDE.md", "CLAUDE.local.md", "AGENTS.md"];
/// Read only in the cwd itself.
const PROJECT_FILES: [&str; 3] = [
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".mcp.json",
];
/// Read from the (common) git directory besides HEAD and the branch ref.
const GIT_FILES: [&str; 2] = ["config", "packed-refs"];
/// HEAD, a `.git` pointer file and `commondir` are a few bytes.
const SMALL_FILE_BYTES: u64 = 4 * 1024;
/// Resource guard against a pathological gitdir chain; a real fetch is ~30.
const MAX_FETCH_PATHS: usize = 96;

/// What a metadata lookup says about one startup file. Never downloads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalStartupFileProbe {
    Missing,
    Directory,
    /// Contents are on this Mac.
    Local,
    /// A File Provider holds the contents; reading it blocks until downloaded.
    Evicted,
}

/// The file system seen by a startup fetch. Every call may block for as long
/// as a File Provider takes, so it is only ever made on a fetch's own thread.
pub trait TerminalStartupFiles: Send + Sync {
    fn probe(&self, path: &Path) -> TerminalStartupFileProbe;
    /// Reads enough of `path` for the File Provider to download it.
    fn materialize(&self, path: &Path);
    /// Reads a small text file (HEAD, a `.git` pointer, `commondir`).
    fn read_small(&self, path: &Path) -> Option<String>;
}

/// The local file system. A dataless file (`SF_DATALESS`) is `Evicted`.
pub struct SystemTerminalStartupFiles;

impl TerminalStartupFiles for SystemTerminalStartupFiles {
    fn probe(&self, path: &Path) -> TerminalStartupFileProbe {
        match std::fs::metadata(path) {
            Err(_) => TerminalStartupFileProbe::Missing,
            Ok(metadata) if metadata.is_dir() => TerminalStartupFileProbe::Directory,
            Ok(metadata) if is_dataless(&metadata) => TerminalStartupFileProbe::Evicted,
            Ok(_) => TerminalStartupFileProbe::Local,
        }
    }

    fn materialize(&self, path: &Path) {
        if let Ok(mut file) = File::open(path) {
            let _ = file.read(&mut [0; 1]);
        }
    }

    fn read_small(&self, path: &Path) -> Option<String> {
        let mut text = String::new();
        File::open(path)
            .ok()?
            .take(SMALL_FILE_BYTES)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Fetch {
    File,
    /// `<dir>/.git`: a directory, or a linked worktree's `gitdir:` pointer.
    GitEntry,
    /// `<git dir>/HEAD`; its branch ref lives under the common git dir.
    Head {
        common: PathBuf,
    },
}

#[derive(Default)]
struct FetchState {
    pending: BTreeSet<PathBuf>,
    seen: BTreeSet<PathBuf>,
}

/// One cwd's startup files, fetched concurrently. Every path gets its own
/// thread, which holds the fetch alive until its read returns.
pub(crate) struct StartupFetch {
    files: Arc<dyn TerminalStartupFiles>,
    state: Mutex<FetchState>,
    started: Instant,
}

impl StartupFetch {
    /// Starts fetching `cwd`'s startup files, walking ancestors up to `stop`
    /// (the home folder) exclusive, or to the file system root.
    pub(crate) fn start(
        files: Arc<dyn TerminalStartupFiles>,
        cwd: &Path,
        stop: Option<&Path>,
    ) -> Arc<Self> {
        let fetch = Arc::new(Self {
            files,
            state: Mutex::new(FetchState::default()),
            started: Instant::now(),
        });
        for (path, fetch_kind) in startup_candidates(cwd, stop) {
            fetch.spawn(path, fetch_kind);
        }
        fetch
    }

    /// Paths whose fetch has not finished, in path order.
    pub(crate) fn pending(&self) -> Vec<PathBuf> {
        lock(&self.state).pending.iter().cloned().collect()
    }

    /// How long this fetch has been running.
    pub(crate) fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Fetches `path` on its own thread once per fetch. A path stays pending
    /// until its read returns and anything it leads to has been queued.
    fn spawn(self: &Arc<Self>, path: PathBuf, kind: Fetch) {
        {
            let mut state = lock(&self.state);
            if state.seen.len() >= MAX_FETCH_PATHS || !state.seen.insert(path.clone()) {
                return;
            }
            state.pending.insert(path.clone());
        }
        let fetch = Arc::clone(self);
        let unspawned = path.clone();
        let spawned = thread::Builder::new()
            .name("iface-startup-fetch".to_owned())
            .spawn(move || fetch.run(&path, kind));
        if spawned.is_err() {
            lock(&self.state).pending.remove(&unspawned);
        }
    }

    fn run(self: Arc<Self>, path: &Path, kind: Fetch) {
        match kind {
            Fetch::File => {
                if self.files.probe(path) == TerminalStartupFileProbe::Evicted {
                    self.files.materialize(path);
                }
            }
            Fetch::GitEntry => self.expand_git_entry(path),
            Fetch::Head { common } => {
                let reference = self.files.read_small(path).and_then(|head| {
                    head.trim()
                        .strip_prefix("ref:")
                        .and_then(|reference| contained(reference.trim()))
                });
                if let Some(reference) = reference {
                    self.spawn(common.join(reference), Fetch::File);
                }
            }
        }
        lock(&self.state).pending.remove(path);
    }

    /// A `.git` directory is its own common dir. A linked worktree's `.git`
    /// file names its private gitdir, whose `commondir` names the shared one.
    fn expand_git_entry(self: &Arc<Self>, entry: &Path) {
        let (git_dir, common) = match self.files.probe(entry) {
            TerminalStartupFileProbe::Missing => return,
            TerminalStartupFileProbe::Directory => (entry.to_path_buf(), entry.to_path_buf()),
            TerminalStartupFileProbe::Local | TerminalStartupFileProbe::Evicted => {
                let Some(pointer) = self.files.read_small(entry) else {
                    return;
                };
                let (Some(target), Some(parent)) =
                    (pointer.trim().strip_prefix("gitdir:"), entry.parent())
                else {
                    return;
                };
                let git_dir = lexically_normalized(&parent.join(target.trim()));
                let common = self
                    .files
                    .read_small(&git_dir.join("commondir"))
                    .map_or_else(
                        || git_dir.clone(),
                        |common| lexically_normalized(&git_dir.join(common.trim())),
                    );
                (git_dir, common)
            }
        };
        self.spawn(
            git_dir.join("HEAD"),
            Fetch::Head {
                common: common.clone(),
            },
        );
        for name in GIT_FILES {
            self.spawn(common.join(name), Fetch::File);
        }
    }
}

/// A HEAD ref such as `refs/heads/main`, never one that leaves the git dir.
fn contained(reference: &str) -> Option<PathBuf> {
    let path = Path::new(reference);
    (!reference.is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_))))
    .then(|| path.to_path_buf())
}

/// The candidates a fetch starts from, before any git expansion.
fn startup_candidates(cwd: &Path, stop: Option<&Path>) -> Vec<(PathBuf, Fetch)> {
    let mut candidates: Vec<(PathBuf, Fetch)> = PROJECT_FILES
        .iter()
        .map(|name| (cwd.join(name), Fetch::File))
        .collect();
    for directory in cwd.ancestors() {
        if Some(directory) == stop || directory.parent().is_none() {
            break;
        }
        candidates.extend(
            INSTRUCTION_FILES
                .iter()
                .map(|name| (directory.join(name), Fetch::File)),
        );
        candidates.push((directory.join(".git"), Fetch::GitEntry));
    }
    candidates
}

/// The wire projection of a pane's pending startup files, or `None` once
/// every one is local. Files in the cwd come first, then `~/` ones.
pub(crate) fn startup_download(
    pending: &[PathBuf],
    cwd: &Path,
    home: Option<&Path>,
) -> Option<TerminalStartupDownloadV1> {
    if pending.is_empty() {
        return None;
    }
    let mut named: Vec<(u8, String)> = pending
        .iter()
        .map(|path| {
            if let Ok(relative) = path.strip_prefix(cwd) {
                (0, relative.display().to_string())
            } else if let Some(relative) = home.and_then(|home| path.strip_prefix(home).ok()) {
                (1, format!("~/{}", relative.display()))
            } else {
                (2, path.display().to_string())
            }
        })
        .collect();
    named.sort();
    Some(TerminalStartupDownloadV1 {
        version: TERMINAL_STARTUP_DOWNLOAD_VERSION,
        files: named
            .into_iter()
            .take(TERMINAL_STARTUP_DOWNLOAD_NAMED_FILES)
            .map(|(_, name)| name)
            .collect(),
        remaining: u32::try_from(pending.len()).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{Condvar, Mutex},
        time::{Duration, Instant},
    };

    use super::*;

    #[derive(Clone)]
    enum Entry {
        Dir,
        File { text: String, evicted: bool },
    }

    /// A file system whose evicted files download only when a test releases
    /// them, so "still in flight" is observable.
    #[derive(Default)]
    struct FakeFiles {
        entries: Mutex<BTreeMap<PathBuf, Entry>>,
        held: Mutex<BTreeSet<PathBuf>>,
        released: Condvar,
        downloads: Mutex<Vec<PathBuf>>,
    }

    impl FakeFiles {
        fn dir(&self, path: &str) -> &Self {
            lock(&self.entries).insert(PathBuf::from(path), Entry::Dir);
            self
        }

        fn file(&self, path: &str, text: &str) -> &Self {
            let entry = Entry::File {
                text: text.to_owned(),
                evicted: false,
            };
            lock(&self.entries).insert(PathBuf::from(path), entry);
            self
        }

        /// Evicted and held: reading it blocks until [`Self::release`].
        fn evicted(&self, path: &str, text: &str) -> &Self {
            let entry = Entry::File {
                text: text.to_owned(),
                evicted: true,
            };
            lock(&self.entries).insert(PathBuf::from(path), entry);
            lock(&self.held).insert(PathBuf::from(path));
            self
        }

        fn release(&self, path: &str) {
            lock(&self.held).remove(Path::new(path));
            self.released.notify_all();
        }

        fn download(&self, path: &Path) {
            let mut held = lock(&self.held);
            while held.contains(path) {
                held = self
                    .released
                    .wait(held)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            drop(held);
            if let Some(Entry::File { evicted, .. }) = lock(&self.entries).get_mut(path)
                && *evicted
            {
                *evicted = false;
                lock(&self.downloads).push(path.to_path_buf());
            }
        }

        fn downloads(&self) -> Vec<PathBuf> {
            lock(&self.downloads).clone()
        }
    }

    impl TerminalStartupFiles for FakeFiles {
        fn probe(&self, path: &Path) -> TerminalStartupFileProbe {
            match lock(&self.entries).get(path) {
                None => TerminalStartupFileProbe::Missing,
                Some(Entry::Dir) => TerminalStartupFileProbe::Directory,
                Some(Entry::File { evicted: true, .. }) => TerminalStartupFileProbe::Evicted,
                Some(Entry::File { .. }) => TerminalStartupFileProbe::Local,
            }
        }

        fn materialize(&self, path: &Path) {
            self.download(path);
        }

        fn read_small(&self, path: &Path) -> Option<String> {
            self.download(path);
            match lock(&self.entries).get(path) {
                Some(Entry::File { text, .. }) => Some(text.clone()),
                _ => None,
            }
        }
    }

    fn paths(values: &[&str]) -> Vec<PathBuf> {
        values.iter().map(PathBuf::from).collect()
    }

    /// Waits for the fetch's pending set to settle on `expected`.
    fn settles(fetch: &StartupFetch, expected: &[&str]) {
        let expected = paths(expected);
        let deadline = Instant::now() + Duration::from_secs(5);
        while fetch.pending() != expected {
            assert!(
                Instant::now() < deadline,
                "pending {:?}, expected {expected:?}",
                fetch.pending()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    const HOME: &str = "/Users/me";
    const CWD: &str = "/Users/me/Desktop/Projects/App";

    fn start(files: &Arc<FakeFiles>) -> Arc<StartupFetch> {
        let files: Arc<dyn TerminalStartupFiles> = files.clone();
        StartupFetch::start(files, Path::new(CWD), Some(Path::new(HOME)))
    }

    #[test]
    fn candidates_walk_instruction_files_up_to_home_and_project_files_only_in_the_cwd() {
        let candidates: Vec<PathBuf> = startup_candidates(Path::new(CWD), Some(Path::new(HOME)))
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        for expected in [
            "/Users/me/Desktop/Projects/App/CLAUDE.md",
            "/Users/me/Desktop/Projects/App/CLAUDE.local.md",
            "/Users/me/Desktop/Projects/App/AGENTS.md",
            "/Users/me/Desktop/Projects/App/.claude/settings.json",
            "/Users/me/Desktop/Projects/App/.mcp.json",
            "/Users/me/Desktop/Projects/App/.git",
            "/Users/me/Desktop/Projects/AGENTS.md",
            "/Users/me/Desktop/Projects/.git",
            "/Users/me/Desktop/CLAUDE.md",
        ] {
            assert!(candidates.contains(&PathBuf::from(expected)), "{expected}");
        }
        for absent in [
            "/Users/me/CLAUDE.md",
            "/Users/me/.git",
            "/Users/me/Desktop/Projects/.mcp.json",
            "/Users/CLAUDE.md",
        ] {
            assert!(!candidates.contains(&PathBuf::from(absent)), "{absent}");
        }
    }

    #[test]
    fn local_files_finish_at_once_without_a_download() {
        let files = Arc::new(FakeFiles::default());
        files
            .dir("/Users/me/Desktop/Projects/App/.git")
            .file(
                "/Users/me/Desktop/Projects/App/.git/HEAD",
                "ref: refs/heads/main\n",
            )
            .file(
                "/Users/me/Desktop/Projects/App/.git/refs/heads/main",
                "abc\n",
            )
            .file("/Users/me/Desktop/Projects/App/CLAUDE.md", "@AGENTS.md\n");
        let fetch = start(&files);
        settles(&fetch, &[]);
        assert!(files.downloads().is_empty());
    }

    #[test]
    fn an_evicted_file_stays_pending_until_it_downloads() {
        let files = Arc::new(FakeFiles::default());
        files
            .evicted("/Users/me/Desktop/Projects/App/CLAUDE.md", "@AGENTS.md\n")
            .evicted("/Users/me/Desktop/Projects/AGENTS.md", "rules\n");
        let fetch = start(&files);
        settles(
            &fetch,
            &[
                "/Users/me/Desktop/Projects/AGENTS.md",
                "/Users/me/Desktop/Projects/App/CLAUDE.md",
            ],
        );
        files.release("/Users/me/Desktop/Projects/App/CLAUDE.md");
        settles(&fetch, &["/Users/me/Desktop/Projects/AGENTS.md"]);
        files.release("/Users/me/Desktop/Projects/AGENTS.md");
        settles(&fetch, &[]);
        assert_eq!(files.downloads().len(), 2);
    }

    #[test]
    fn a_git_directory_fetches_head_config_and_then_the_branch_ref() {
        let files = Arc::new(FakeFiles::default());
        files
            .dir("/Users/me/Desktop/Projects/App/.git")
            .evicted(
                "/Users/me/Desktop/Projects/App/.git/HEAD",
                "ref: refs/heads/main\n",
            )
            .evicted("/Users/me/Desktop/Projects/App/.git/config", "[core]\n")
            .evicted(
                "/Users/me/Desktop/Projects/App/.git/refs/heads/main",
                "abc\n",
            );
        let fetch = start(&files);
        settles(
            &fetch,
            &[
                "/Users/me/Desktop/Projects/App/.git/HEAD",
                "/Users/me/Desktop/Projects/App/.git/config",
            ],
        );
        files.release("/Users/me/Desktop/Projects/App/.git/HEAD");
        settles(
            &fetch,
            &[
                "/Users/me/Desktop/Projects/App/.git/config",
                "/Users/me/Desktop/Projects/App/.git/refs/heads/main",
            ],
        );
        files.release("/Users/me/Desktop/Projects/App/.git/config");
        files.release("/Users/me/Desktop/Projects/App/.git/refs/heads/main");
        settles(&fetch, &[]);
        assert_eq!(files.downloads().len(), 3);
    }

    #[test]
    fn a_linked_worktree_follows_gitdir_and_commondir() {
        let files = Arc::new(FakeFiles::default());
        files
            .file(
                "/Users/me/Desktop/Projects/App/.git",
                "gitdir: /Users/me/Repo/.git/worktrees/app\n",
            )
            .file("/Users/me/Repo/.git/worktrees/app/commondir", "../..\n")
            .file(
                "/Users/me/Repo/.git/worktrees/app/HEAD",
                "ref: refs/heads/feature\n",
            )
            .evicted("/Users/me/Repo/.git/config", "[core]\n")
            .evicted("/Users/me/Repo/.git/refs/heads/feature", "abc\n");
        let fetch = start(&files);
        settles(
            &fetch,
            &[
                "/Users/me/Repo/.git/config",
                "/Users/me/Repo/.git/refs/heads/feature",
            ],
        );
        files.release("/Users/me/Repo/.git/config");
        files.release("/Users/me/Repo/.git/refs/heads/feature");
        settles(&fetch, &[]);
    }

    #[test]
    fn a_head_ref_that_leaves_the_git_directory_is_never_read() {
        let files = Arc::new(FakeFiles::default());
        files
            .dir("/Users/me/Desktop/Projects/App/.git")
            .file(
                "/Users/me/Desktop/Projects/App/.git/HEAD",
                "ref: ../../../secret\n",
            )
            .evicted("/Users/me/Desktop/Projects/secret", "no\n");
        let fetch = start(&files);
        settles(&fetch, &[]);
        assert!(files.downloads().is_empty());
    }

    #[test]
    fn the_local_file_system_probes_without_downloading_and_reads_small_files() {
        let directory = tempfile::TempDir::new().unwrap();
        let head = directory.path().join("HEAD");
        std::fs::write(&head, "ref: refs/heads/main\n").unwrap();
        let large = directory.path().join("large");
        std::fs::write(&large, "x".repeat(8 * 1024)).unwrap();
        let files = SystemTerminalStartupFiles;
        assert_eq!(
            files.probe(&directory.path().join("absent")),
            TerminalStartupFileProbe::Missing
        );
        assert_eq!(
            files.probe(directory.path()),
            TerminalStartupFileProbe::Directory
        );
        assert_eq!(files.probe(&head), TerminalStartupFileProbe::Local);
        files.materialize(&head);
        assert_eq!(
            files.read_small(&head).as_deref(),
            Some("ref: refs/heads/main\n")
        );
        assert_eq!(
            files.read_small(&large).map(|text| text.len()),
            Some(4 * 1024)
        );
        assert_eq!(files.read_small(&directory.path().join("absent")), None);
    }

    #[test]
    fn the_projection_names_the_nearest_three_and_counts_every_file() {
        let pending = paths(&[
            "/Users/me/Desktop/Projects/AGENTS.md",
            "/Users/me/Desktop/Projects/App/.git/HEAD",
            "/Users/me/Desktop/Projects/App/.git/config",
            "/Users/me/Desktop/Projects/App/CLAUDE.md",
            "/Volumes/Other/AGENTS.md",
        ]);
        let download = startup_download(&pending, Path::new(CWD), Some(Path::new(HOME))).unwrap();
        assert_eq!(download.version, TERMINAL_STARTUP_DOWNLOAD_VERSION);
        assert_eq!(download.files, [".git/HEAD", ".git/config", "CLAUDE.md"]);
        assert_eq!(download.remaining, 5);
        let far = startup_download(&pending[..1], Path::new(CWD), Some(Path::new(HOME))).unwrap();
        assert_eq!(far.files, ["~/Desktop/Projects/AGENTS.md"]);
        let outside = startup_download(&pending[4..], Path::new(CWD), None).unwrap();
        assert_eq!(outside.files, ["/Volumes/Other/AGENTS.md"]);
        assert!(startup_download(&[], Path::new(CWD), Some(Path::new(HOME))).is_none());
    }
}
