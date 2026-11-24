//! The programs a repository's own git settings can make git run, and how
//! Interface's own git calls keep from running them.
//!
//! A project folder can arrive with its own `.git` (an archive, a copied
//! folder). Its settings can name programs that git runs while merely reading
//! the folder: filter drivers (`filter.<name>.clean`, `smudge` and `process`,
//! picked by `.gitattributes`) run on every file `git status`, `git diff` or
//! `git add` has to re-read, and a remote read runs `core.sshCommand`,
//! `core.askPass` and credential helpers; a commit runs the signing program;
//! the owner's own Git LFS runs the extensions and transfer agents the
//! repository's settings name. Interface's automatic git calls (Review,
//! thread checkpoints and restores, worktree bookkeeping, the Ship card's
//! reads) must run none of them.
//!
//! Before such a call the caller lists those settings with [`PROBE_ARGS`]
//! (reading settings runs nothing) and hands the [`Overrides`] it yields to the
//! call. Every one of them that the repository sets, in `.git/config`, in a
//! file that file includes (`include.path`, `includeIf`) or in a worktree's
//! `config.worktree`, is replaced by the value the owner's own global or
//! system settings give it, or by a neutral value when they give none. The
//! owner's own settings are left alone, so a Git LFS installed with
//! `git lfs install` keeps working; one the repository sets up for itself is
//! switched off.
//!
//! The overrides travel through `GIT_CONFIG_COUNT`, not `-c`: git splits
//! `-c name=value` at the first `=`, and a filter's name may contain one. They
//! also reach the git processes git starts itself (`git worktree remove`'s
//! status, git-lfs's helpers). They cannot name a filter that only a nested
//! repository's own settings define, so a populated submodule's filters stay
//! out of reach of this module.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Command;

/// `git config` arguments listing every setting [`Overrides`] cares about,
/// across every scope git reads (system, global, the repository, its
/// worktree and everything they include), each tagged with its scope.
pub const PROBE_ARGS: [&str; 6] = [
    "--no-pager",
    "config",
    "--show-scope",
    "-z",
    "--get-regexp",
    PROBE_PATTERN,
];

/// git matches this against keys whose section and variable it has lowered;
/// a subsection (a filter's name, a credential URL) keeps its case.
const PROBE_PATTERN: &str = r"^(filter\..+\.(clean|smudge|process|required)|core\.(sshcommand|askpass)|credential\.(.+\.)?helper|gpg\.(program|(openpgp|x509|ssh)\.program|ssh\.defaultkeycommand)|lfs\.(extension\..+\.(clean|smudge)|customtransfer\..+\.path))$";

/// A legitimate listing is a few hundred bytes; a longer one is refused.
pub const PROBE_MAX_BYTES: usize = 256 * 1024;

/// Global options for every hardened call: no `ext::` transport (it runs a
/// command named in the URL, and a repository may allow it for itself), and
/// no checkout that recurses into submodules, whose own settings
/// [`Overrides`] cannot see.
pub const STATIC_ARGS: [&str; 4] = [
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "submodule.recurse=false",
];

/// Environment for every hardened call. A partial clone fetches a missing
/// object through the repository's own remote settings (an upload-pack
/// command among them); `core.gitProxy` keeps its first value, which the
/// repository's settings would win, so it is switched off whole.
pub const STATIC_ENV: [(&str, &str); 2] = [("GIT_NO_LAZY_FETCH", "1"), ("GIT_PROXY_COMMAND", "")];

/// Whether a [`PROBE_ARGS`] run answered: 0 lists settings, 1 means none.
#[must_use]
pub const fn probe_answered(exit_code: Option<i32>) -> bool {
    matches!(exit_code, Some(0 | 1))
}

/// What a caller says when the probe's answer cannot be used.
pub const PROBE_REFUSED: &str =
    "Interface could not read this repository's git settings, so it did not run git there.";

/// The settings one hardened git call runs with, from a [`PROBE_ARGS`] listing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Overrides {
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Settings that hold one program, the last value winning, with the value
/// that stands for "not set".
const PROGRAMS: [(&[u8], &[u8]); 7] = [
    (b"core.sshcommand", b"ssh"),
    (b"core.askpass", b""),
    (b"gpg.program", b"gpg"),
    (b"gpg.openpgp.program", b"gpg"),
    (b"gpg.x509.program", b"gpgsm"),
    (b"gpg.ssh.program", b"ssh-keygen"),
    (b"gpg.ssh.defaultkeycommand", b""),
];

const FILTER_COMMANDS: [&[u8]; 3] = [b"clean", b"smudge", b"process"];

struct Entry<'a> {
    trusted: bool,
    key: &'a [u8],
    value: Option<&'a [u8]>,
}

impl Overrides {
    /// Reads what [`PROBE_ARGS`] printed with exit 0 or 1. `None` when it is
    /// not shaped that way; the caller then runs nothing.
    #[must_use]
    pub fn from_listing(listing: &[u8]) -> Option<Self> {
        let entries = parse(listing)?;
        let mut pairs = Vec::new();
        let untrusted = |wanted: &dyn Fn(&[u8]) -> bool| {
            entries
                .iter()
                .any(|entry| !entry.trusted && wanted(entry.key))
        };
        let trusted_value = |key: &[u8]| {
            entries
                .iter()
                .rev()
                .find(|entry| entry.trusted && entry.key == key)
                .and_then(|entry| entry.value)
        };

        for (key, unset) in PROGRAMS {
            if untrusted(&|candidate| candidate == key) {
                pairs.push((key.to_vec(), trusted_value(key).unwrap_or(unset).to_vec()));
            }
        }
        // The owner's own Git LFS still reads the repository's settings, and
        // runs the extensions and transfer agents they name.
        let mut lfs_programs: Vec<&[u8]> = Vec::new();
        for entry in &entries {
            if !entry.trusted && is_lfs_program(entry.key) && !lfs_programs.contains(&entry.key) {
                lfs_programs.push(entry.key);
            }
        }
        for key in lfs_programs {
            pairs.push((
                key.to_vec(),
                trusted_value(key).unwrap_or_default().to_vec(),
            ));
        }

        let mut names: Vec<&[u8]> = Vec::new();
        for entry in entries.iter().filter(|entry| !entry.trusted) {
            if let Some((name, _)) = filter_key(entry.key)
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        for name in names {
            let key = |field: &[u8]| [b"filter.", name, b".", field].concat();
            let mut runs_something = false;
            for field in FILTER_COMMANDS {
                let value = trusted_value(&key(field)).unwrap_or_default();
                runs_something |= !value.is_empty();
                pairs.push((key(field), value.to_vec()));
            }
            // A blanked filter that stays `required` makes git refuse the file.
            let required = trusted_value(&key(b"required"))
                .filter(|_| runs_something)
                .unwrap_or(b"false".as_slice());
            pairs.push((key(b"required"), required.to_vec()));
        }

        if untrusted(&is_credential_helper) {
            // An empty helper empties the list collected so far, URL-specific
            // entries included; the owner's own helpers are then replayed.
            pairs.push((b"credential.helper".to_vec(), Vec::new()));
            for entry in &entries {
                if entry.trusted
                    && is_credential_helper(entry.key)
                    && let Some(value) = entry.value
                {
                    pairs.push((entry.key.to_vec(), value.to_vec()));
                }
            }
        }
        Some(Self { pairs })
    }

    /// The settings, in the order git reads them.
    #[must_use]
    pub fn pairs(&self) -> &[(Vec<u8>, Vec<u8>)] {
        &self.pairs
    }

    /// Puts [`STATIC_ENV`] and these settings into `command`'s environment.
    /// Call it after removing the inherited `GIT_*` variables.
    pub fn apply(&self, command: &mut Command) {
        command.envs(STATIC_ENV);
        command.env("GIT_CONFIG_COUNT", self.pairs.len().to_string());
        for (index, (key, value)) in self.pairs.iter().enumerate() {
            command.env(format!("GIT_CONFIG_KEY_{index}"), OsStr::from_bytes(key));
            command.env(
                format!("GIT_CONFIG_VALUE_{index}"),
                OsStr::from_bytes(value),
            );
        }
    }
}

/// `scope NUL key [LF value] NUL`, repeated. Only `system` and `global` are
/// the owner's own; `local`, `worktree` and anything else is the repository's.
fn parse(listing: &[u8]) -> Option<Vec<Entry<'_>>> {
    let mut fields = listing.split(|byte| *byte == 0);
    let mut entries = Vec::new();
    loop {
        let scope = fields.next()?;
        if scope.is_empty() {
            return fields.next().is_none().then_some(entries);
        }
        let record = fields.next()?;
        let (key, value) = match record.iter().position(|byte| *byte == b'\n') {
            Some(at) => (&record[..at], Some(&record[at + 1..])),
            None => (record, None),
        };
        if key.is_empty() || !scope.iter().all(u8::is_ascii_lowercase) {
            return None;
        }
        entries.push(Entry {
            trusted: matches!(scope, b"system" | b"global"),
            key,
            value,
        });
    }
}

/// `filter.<name>.<field>` for a field [`Overrides`] handles.
fn filter_key(key: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = key.strip_prefix(b"filter.")?;
    let dot = rest.iter().rposition(|byte| *byte == b'.')?;
    let (name, field) = (&rest[..dot], &rest[dot + 1..]);
    (!name.is_empty() && (FILTER_COMMANDS.contains(&field) || field == b"required"))
        .then_some((name, field))
}

fn is_credential_helper(key: &[u8]) -> bool {
    key.starts_with(b"credential.") && key.ends_with(b".helper")
}

/// `lfs.extension.<name>.clean|smudge` and `lfs.customtransfer.<name>.path`.
fn is_lfs_program(key: &[u8]) -> bool {
    let named = |prefix: &[u8], fields: &[&[u8]]| {
        key.strip_prefix(prefix).is_some_and(|rest| {
            rest.iter()
                .rposition(|byte| *byte == b'.')
                .is_some_and(|dot| dot > 0 && fields.contains(&&rest[dot + 1..]))
        })
    };
    named(b"lfs.extension.", &[b"clean", b"smudge"]) || named(b"lfs.customtransfer.", &[b"path"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (scope, record) in entries {
            bytes.extend_from_slice(scope.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(record.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    fn pairs(overrides: &Overrides) -> Vec<(String, String)> {
        overrides
            .pairs()
            .iter()
            .map(|(key, value)| {
                (
                    String::from_utf8(key.clone()).unwrap(),
                    String::from_utf8(value.clone()).unwrap(),
                )
            })
            .collect()
    }

    fn owned(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_repository_filter_is_blanked_and_no_longer_required() {
        let overrides = Overrides::from_listing(&listing(&[
            ("local", "filter.x.clean\nsh -c evil"),
            ("local", "filter.x.required\ntrue"),
        ]))
        .unwrap();
        assert_eq!(
            pairs(&overrides),
            owned(&[
                ("filter.x.clean", ""),
                ("filter.x.smudge", ""),
                ("filter.x.process", ""),
                ("filter.x.required", "false"),
            ])
        );
    }

    #[test]
    fn the_owners_own_filter_stays_and_wins_over_the_repositorys_copy() {
        let global_only = Overrides::from_listing(&listing(&[
            ("global", "filter.lfs.process\ngit-lfs filter-process"),
            ("global", "filter.lfs.required\ntrue"),
        ]))
        .unwrap();
        assert!(global_only.pairs().is_empty());

        let both = Overrides::from_listing(&listing(&[
            ("global", "filter.lfs.clean\ngit-lfs clean -- %f"),
            ("global", "filter.lfs.process\ngit-lfs filter-process"),
            ("global", "filter.lfs.required\ntrue"),
            ("local", "filter.lfs.process\nevil"),
            ("worktree", "filter.lfs.required\nfalse"),
        ]))
        .unwrap();
        assert_eq!(
            pairs(&both),
            owned(&[
                ("filter.lfs.clean", "git-lfs clean -- %f"),
                ("filter.lfs.smudge", ""),
                ("filter.lfs.process", "git-lfs filter-process"),
                ("filter.lfs.required", "true"),
            ])
        );
    }

    #[test]
    fn a_filter_name_keeps_its_case_dots_and_equals_signs() {
        let overrides =
            Overrides::from_listing(&listing(&[("local", "filter.A=b.c.smudge\nevil")])).unwrap();
        assert_eq!(overrides.pairs()[0].0, b"filter.A=b.c.clean");
        assert_eq!(overrides.pairs()[3].0, b"filter.A=b.c.required");
    }

    #[test]
    fn programs_fall_back_to_the_owners_value_or_their_default() {
        let overrides = Overrides::from_listing(&listing(&[
            ("global", "core.sshcommand\nssh -i ~/.ssh/work"),
            ("local", "core.sshcommand\nevil"),
            ("local", "core.askpass\nevil"),
            ("worktree", "gpg.program\nevil"),
            ("local", "gpg.ssh.program\nevil"),
        ]))
        .unwrap();
        assert_eq!(
            pairs(&overrides),
            owned(&[
                ("core.sshcommand", "ssh -i ~/.ssh/work"),
                ("core.askpass", ""),
                ("gpg.program", "gpg"),
                ("gpg.ssh.program", "ssh-keygen"),
            ])
        );
    }

    #[test]
    fn credential_helpers_are_emptied_then_the_owners_are_replayed() {
        let overrides = Overrides::from_listing(&listing(&[
            ("system", "credential.helper\nosxkeychain"),
            ("local", "credential.https://example.com.helper\n!evil"),
            (
                "global",
                "credential.https://github.com.helper\n!gh auth git-credential",
            ),
        ]))
        .unwrap();
        assert_eq!(
            pairs(&overrides),
            owned(&[
                ("credential.helper", ""),
                ("credential.helper", "osxkeychain"),
                (
                    "credential.https://github.com.helper",
                    "!gh auth git-credential"
                ),
            ])
        );
        let owners_only =
            Overrides::from_listing(&listing(&[("system", "credential.helper\nosxkeychain")]))
                .unwrap();
        assert!(owners_only.pairs().is_empty());
    }

    #[test]
    fn git_lfs_extensions_and_transfer_agents_the_repository_names_are_blanked() {
        let overrides = Overrides::from_listing(&listing(&[
            ("global", "lfs.extension.mine.clean\nmine-clean %f"),
            ("local", "lfs.extension.mine.clean\nevil"),
            ("local", "lfs.extension.theirs.smudge\nevil"),
            ("worktree", "lfs.customtransfer.agent.path\nevil"),
        ]))
        .unwrap();
        assert_eq!(
            pairs(&overrides),
            owned(&[
                ("lfs.extension.mine.clean", "mine-clean %f"),
                ("lfs.extension.theirs.smudge", ""),
                ("lfs.customtransfer.agent.path", ""),
            ])
        );
        assert!(!is_lfs_program(b"lfs.extension..clean"));
        assert!(!is_lfs_program(b"lfs.customtransfer.agent.args"));
    }

    #[test]
    fn empty_and_malformed_listings() {
        assert_eq!(Overrides::from_listing(b""), Some(Overrides::default()));
        assert_eq!(Overrides::from_listing(b"local\0filter.x.clean\nx"), None);
        assert_eq!(Overrides::from_listing(b"local\0"), None);
        assert_eq!(Overrides::from_listing(b"Local\0filter.x.clean\nx\0"), None);
        assert_eq!(Overrides::from_listing(b"local\0\0"), None);
        assert!(probe_answered(Some(1)));
        assert!(!probe_answered(Some(128)));
        assert!(!probe_answered(None));
    }
}
