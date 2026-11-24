//! Path resolution for the approval policy: symlink-following real paths,
//! credential and harness-configuration names, scope membership, and glob
//! expansion.

use std::fs;
use std::path::{Component, Path, PathBuf};

use interface_common::paths::lexically_normalized;
use interface_workspace::is_credential_path;

use super::shell::{brace_groups, has_glob};
use super::{
    AGENT_CONFIG, AGENT_CONFIG_DIRECTORIES, AgentScope, CREDENTIAL, LINK_BUDGET, OUTSIDE, RM_ROOT,
    UNRESOLVED, Verdict,
};

/// What a path-changing act does to its target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Effect {
    /// Creates, modifies or changes the metadata of the target.
    Write,
    /// Unlinks the named entry; a trailing glob stays inside its directory.
    Remove,
    /// Removes a whole tree; a scope root, the home directory or `/` is refused.
    RemoveTree,
    /// Renames the named entry away; like `Remove`, but a scope root, the home
    /// directory or `/` itself is refused.
    Move,
}

/// `~` and `~/…` expand to the home directory; every other word is literal.
pub(super) fn expand_home(word: &str, home: &Path) -> PathBuf {
    if word == "~" {
        home.to_path_buf()
    } else if let Some(rest) = word.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(word)
    }
}

/// Follows every symlink the kernel would, one component at a time, so a `..`
/// after a link climbs from the link's target, and a dangling link lands where
/// a write through it would. Components that do not exist yet stay textual.
/// `None` for a relative path, an unreadable link or a link chain past the budget.
pub(super) fn real_path(path: &Path) -> Option<PathBuf> {
    let mut budget = LINK_BUDGET;
    follow_links(path, &mut budget)
}

fn follow_links(path: &Path, budget: &mut usize) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                let candidate = resolved.join(name);
                let is_link = fs::symlink_metadata(&candidate)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink());
                if is_link {
                    *budget = budget.checked_sub(1)?;
                    let target = fs::read_link(&candidate).ok()?;
                    resolved = follow_links(&resolved.join(target), budget)?;
                } else {
                    resolved = candidate;
                }
            }
        }
    }
    Some(resolved)
}

fn lowercase(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

fn names_credential(path: &Path) -> bool {
    is_credential_path(&path.to_string_lossy())
}

/// A path inside a harness configuration directory, an `.mcp.json` file, or
/// git machinery that runs code on the next git command: a `.git` directory
/// itself, anything under a `hooks` directory inside it, or a `config` or
/// `config.worktree` file inside it. Case-insensitive, like the default macOS
/// filesystem.
fn names_agent_config(path: &Path) -> bool {
    let names: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let git_machinery = names.iter().enumerate().any(|(index, name)| {
        let inside = &names[index + 1..];
        name == ".git"
            && (inside.is_empty()
                || inside.iter().any(|name| name == "hooks")
                || matches!(
                    inside.last().map(String::as_str),
                    Some("config" | "config.worktree")
                ))
    });
    git_machinery || names_harness_config(path)
}

fn names_harness_config(path: &Path) -> bool {
    let is_config = |name: &str| {
        AGENT_CONFIG_DIRECTORIES
            .iter()
            .any(|directory| name.eq_ignore_ascii_case(directory))
    };
    path.components()
        .any(|component| is_config(&component.as_os_str().to_string_lossy()))
        || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(".mcp.json"))
}

/// Credential stores under the home directory that the shared tables do not
/// name. Policy-local on purpose: the shared tables feed other classifiers.
const HOME_CREDENTIAL_STORES: &[&str] = &[
    "library/application support/interface",
    "library/application support/interface dev",
    ".claude",
    ".codex",
    ".grok",
    ".config/gh",
    "library/keychains",
    ".config/gcloud",
    ".password-store",
    ".local/share/keyrings",
];

/// A `hosts.yml` below a `gh` directory: the GitHub CLI's token file.
fn names_gh_hosts(lowercase: &Path) -> bool {
    lowercase
        .file_name()
        .is_some_and(|name| name == "hosts.yml")
        && lowercase.parent().is_some_and(|parent| {
            parent
                .components()
                .any(|component| component.as_os_str() == "gh")
        })
}

pub(super) struct Context<'a> {
    pub(super) require_known_effects: bool,
    pub(super) scope: &'a AgentScope,
    /// Real scope roots; a root that cannot be resolved contains nothing.
    roots: Vec<PathBuf>,
    home: Option<PathBuf>,
    /// The home directory as written and as resolved, lowercased.
    lowercase_homes: Vec<PathBuf>,
}

impl<'a> Context<'a> {
    pub(super) fn new(scope: &'a AgentScope) -> Self {
        Self {
            require_known_effects: false,
            scope,
            roots: scope
                .roots
                .iter()
                .filter_map(|root| real_path(root))
                .collect(),
            home: real_path(&scope.home_dir),
            lowercase_homes: std::iter::once(lexically_normalized(&scope.home_dir))
                .chain(real_path(&scope.home_dir))
                .map(|home| lowercase(&home))
                .collect(),
        }
    }

    fn is_credential(&self, path: &Path) -> bool {
        if names_credential(path) {
            return true;
        }
        let path = lowercase(path);
        names_gh_hosts(&path)
            || self.lowercase_homes.iter().any(|home| {
                HOME_CREDENTIAL_STORES
                    .iter()
                    .any(|store| path.starts_with(home.join(store)))
            })
    }

    /// `Some(true)` when the path as written or as the kernel would reach it
    /// names credential material; `None` when the real path cannot be determined.
    pub(super) fn credential_at(&self, joined: &Path) -> Option<bool> {
        if self.is_credential(&lexically_normalized(joined)) {
            return Some(true);
        }
        real_path(joined).map(|real| self.is_credential(&real))
    }

    /// Whether the path, followed through its links, is inside a scope root.
    pub(super) fn in_scope(&self, path: &Path) -> bool {
        real_path(path).is_some_and(|real| self.roots.iter().any(|root| real.starts_with(root)))
    }

    /// Credential, same-command replacement and scope checks for a path a
    /// command reads. Unlike `check_path`, agent configuration may be read.
    pub(super) fn check_read_path(&self, joined: &Path, created: &[PathBuf]) -> Verdict {
        let lexical = lexically_normalized(joined);
        let real = real_path(joined).ok_or(UNRESOLVED)?;
        if self.is_credential(&lexical) || self.is_credential(&real) {
            return Err(CREDENTIAL);
        }
        if created
            .iter()
            .any(|path| lexical.starts_with(path) || real.starts_with(path))
        {
            return Err(UNRESOLVED);
        }
        if !self.roots.iter().any(|root| real.starts_with(root)) {
            return Err(OUTSIDE);
        }
        Ok(())
    }

    /// Whether every candidate directory is known and resolves inside a scope root.
    pub(super) fn all_in_scope(&self, directories: Option<&[PathBuf]>) -> bool {
        directories.is_some_and(|directories| {
            directories.iter().all(|directory| {
                real_path(directory)
                    .is_some_and(|real| self.roots.iter().any(|root| real.starts_with(root)))
            })
        })
    }

    pub(super) fn is_protected(&self, real: &Path) -> bool {
        real == Path::new("/")
            || self.home.as_deref() == Some(real)
            || self.roots.iter().any(|root| root == real)
    }

    /// Root, credential, harness-configuration and scope checks for one
    /// absolute (possibly unnormalized) path, in the order their reasons are
    /// reported.
    pub(super) fn check_path(&self, joined: &Path, effect: Effect, created: &[PathBuf]) -> Verdict {
        let lexical = lexically_normalized(joined);
        let real = real_path(joined).ok_or(UNRESOLVED)?;
        if effect != Effect::Write && self.is_protected(&real) {
            return Err(RM_ROOT);
        }
        if self.is_credential(&lexical) || self.is_credential(&real) {
            return Err(CREDENTIAL);
        }
        // Agents must not reconfigure their own harness, even inside scope.
        if names_agent_config(&lexical) || names_agent_config(&real) {
            return Err(AGENT_CONFIG);
        }
        // A link, move or copy earlier in this command may have replaced
        // anything at or below this path; its real target is not knowable yet.
        if created
            .iter()
            .any(|path| lexical.starts_with(path) || real.starts_with(path))
        {
            return Err(UNRESOLVED);
        }
        if !self.roots.iter().any(|root| real.starts_with(root)) {
            return Err(OUTSIDE);
        }
        Ok(())
    }
}

/// Directory entries a glob may examine before the check fails closed.
const GLOB_ENTRY_BUDGET: usize = 20_000;

/// Over-approximates brace expansion for the credential check: each group
/// becomes `*`. `None` when a group spans a `/`, which `*` cannot.
pub(super) fn brace_groups_as_wildcards(word: &str) -> Option<String> {
    let mut pattern = String::with_capacity(word.len());
    let mut copied = 0;
    for (open, close) in brace_groups(word) {
        if open < copied {
            continue;
        }
        if word[open..close].contains('/') {
            return None;
        }
        pattern.push_str(&word[copied..open]);
        pattern.push('*');
        copied = close + 1;
    }
    pattern.push_str(&word[copied..]);
    Some(pattern)
}

/// Every existing path a glob matches, component by component. A bracket
/// class matches any one character, so the result may over-match; a leading
/// `.` must be matched literally, as in the shells' defaults. `None` past the
/// entry budget.
pub(super) fn expand_glob(pattern: &Path) -> Option<Vec<PathBuf>> {
    let mut budget = GLOB_ENTRY_BUDGET;
    let mut found = vec![PathBuf::new()];
    for component in pattern.components() {
        let text = component.as_os_str().to_string_lossy();
        if !has_glob(&text) {
            for path in &mut found {
                path.push(component);
            }
            continue;
        }
        let wanted: Vec<char> = text.chars().collect();
        let mut next = Vec::new();
        for directory in &found {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries {
                budget = budget.checked_sub(1)?;
                let name = entry.ok()?.file_name();
                let candidate: Vec<char> = name.to_string_lossy().chars().collect();
                let hidden = candidate.first() == Some(&'.') && wanted.first() != Some(&'.');
                if !hidden && wildcard_matches(&wanted, &candidate) {
                    next.push(directory.join(name));
                }
            }
        }
        found = next;
    }
    Some(found)
}

/// `*` matches any run, `?` and a closed `[…]` class any single character.
pub(super) fn wildcard_matches(pattern: &[char], name: &[char]) -> bool {
    let (mut at_pattern, mut at_name) = (0, 0);
    let mut backtrack = None;
    while at_name < name.len() {
        let step = match pattern.get(at_pattern) {
            Some('*') => {
                backtrack = Some((at_pattern, at_name));
                at_pattern += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') => class_end(pattern, at_pattern)
                .map(|close| close + 1 - at_pattern)
                .or((name[at_name] == '[').then_some(1)),
            Some(&literal) => (literal == name[at_name]).then_some(1),
            None => None,
        };
        if let Some(width) = step {
            at_pattern += width;
            at_name += 1;
        } else if let Some((star, consumed)) = backtrack {
            at_pattern = star + 1;
            at_name = consumed + 1;
            backtrack = Some((star, consumed + 1));
        } else {
            return false;
        }
    }
    pattern[at_pattern..].iter().all(|&c| c == '*')
}

/// Index of the `]` closing the class opened at `open`; a `]` first in the
/// class (after an optional `!` or `^`) is a member, not the close.
fn class_end(pattern: &[char], open: usize) -> Option<usize> {
    let mut first = open + 1;
    if matches!(pattern.get(first), Some('!' | '^')) {
        first += 1;
    }
    (first + 1..pattern.len())
        .find(|&index| pattern[index] == ']')
        .filter(|_| first < pattern.len())
}
