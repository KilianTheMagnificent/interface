//! Path-name tables shared verbatim by the workspace classifier and the
//! checkpoint classifier, the lexical normalizer every path guard runs, and the
//! repository tooling's credential-name check.
//!
//! Only the tables are shared with the product classifiers. The two classifiers
//! around them have drifted — the workspace one is regex-driven and the
//! checkpoint one is not — and which paths each omits is observable behavior,
//! so those rules stay in their crates.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Resolves `.` and `..` textually, touching no filesystem.
///
/// This is the one half every path guard in the workspace shares. What a
/// failure to make a path absolute is called differs per caller and stays with
/// the caller. A symlink is never followed here, so a guard that needs that
/// still re-checks identity after canonicalizing.
#[must_use]
pub fn lexically_normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Joins a relative `path` onto the current directory, then resolves `.` and
/// `..` with [`lexically_normalized`], touching no other filesystem state.
///
/// An empty `path` names the current directory; a caller that must refuse it
/// checks first.
///
/// # Errors
///
/// Returns the error from reading the current directory.
pub fn lexically_absolute(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(lexically_normalized(&absolute))
}

/// Lowercase extensions that mark a file as a key, certificate or environment
/// secret on name alone.
pub const SENSITIVE_EXTENSIONS: &[&str] =
    &["env", "key", "pem", "p12", "pfx", "keystore", "jks", "ppk"];

/// Name prefixes of SSH private keys (`id_rsa`, `id_ed25519.bak`, ...).
pub const PRIVATE_KEY_PREFIXES: &[&str] = &["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"];

/// One already-lowercased path component names credential material: a `.env`
/// file other than `.env.example`, a [`SENSITIVE_EXTENSIONS`] file, an SSH
/// private key, a credential or service-account name, or a
/// [`SENSITIVE_DIRECTORIES`] entry.
///
/// The repository tooling uses this to refuse reading a file at all. The
/// product classifiers keep their own, differently drawn rules.
#[must_use]
pub fn is_sensitive_name(lower: &str) -> bool {
    lower != ".env.example"
        && (lower == ".env"
            || lower.starts_with(".env.")
            || Path::new(lower)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| SENSITIVE_EXTENSIONS.contains(&extension))
            || PRIVATE_KEY_PREFIXES
                .iter()
                .any(|prefix| lower.starts_with(prefix))
            || lower.contains("credential")
            || lower.contains("service-account")
            || lower.contains("service_account")
            || SENSITIVE_DIRECTORIES.contains(&lower))
}

/// Directory names that mark everything beneath them as credential material.
pub const SENSITIVE_DIRECTORIES: &[&str] = &[
    ".aws",
    ".docker",
    ".gnupg",
    ".kube",
    ".ssh",
    ".secret",
    ".secrets",
    ".credential",
    ".credentials",
    "credential",
    "credentials",
    "secret",
    "secrets",
    "thread-secrets",
];

/// Lowercase extensions treated as binary or container payloads on name alone.
pub const BINARY_EXTENSIONS: &[&str] = &[
    "7z", "a", "avi", "bin", "bmp", "bz2", "class", "db", "dll", "doc", "docx", "dylib", "eot",
    "exe", "flac", "gif", "gz", "heic", "icns", "ico", "jar", "jpeg", "jpg", "jks", "keystore",
    "m4a", "mkv", "mov", "mp3", "mp4", "o", "ogg", "otf", "p12", "pdf", "pfx", "png", "ppt",
    "pptx", "pyc", "rar", "sqlite", "sqlite3", "so", "tar", "tgz", "tif", "tiff", "ttf", "wav",
    "wasm", "webm", "webp", "woff", "woff2", "xls", "xlsx", "xz", "zip",
];

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        BINARY_EXTENSIONS, PRIVATE_KEY_PREFIXES, SENSITIVE_DIRECTORIES, SENSITIVE_EXTENSIONS,
        is_sensitive_name, lexically_absolute, lexically_normalized,
    };

    #[test]
    fn normalizing_drops_cur_dir_pops_parent_dir_and_keeps_the_root() {
        assert_eq!(
            lexically_normalized(Path::new("/a/./b/../c/")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            lexically_normalized(Path::new("a/../../b")),
            PathBuf::from("b")
        );
        // `..` cannot escape the root, and a purely relative `.` normalizes away.
        assert_eq!(lexically_normalized(Path::new("/..")), PathBuf::from("/"));
        assert_eq!(lexically_normalized(Path::new(".")), PathBuf::from(""));
    }

    #[test]
    fn tables_stay_lowercase_and_free_of_separators() {
        for entry in SENSITIVE_DIRECTORIES {
            assert_eq!(*entry, entry.to_lowercase(), "{entry}");
            assert!(!entry.contains('/'), "{entry}");
        }
        for entry in BINARY_EXTENSIONS {
            assert_eq!(*entry, entry.to_lowercase(), "{entry}");
            assert!(!entry.starts_with('.'), "{entry}");
        }
    }

    #[test]
    fn tables_hold_the_entries_both_classifiers_depend_on() {
        assert_eq!(SENSITIVE_DIRECTORIES.len(), 14);
        assert!(SENSITIVE_DIRECTORIES.contains(&".ssh"));
        assert!(SENSITIVE_DIRECTORIES.contains(&"credentials"));
        assert!(SENSITIVE_DIRECTORIES.contains(&"thread-secrets"));
        assert_eq!(BINARY_EXTENSIONS.len(), 59);
        assert!(BINARY_EXTENSIONS.contains(&"p12"));
        assert!(BINARY_EXTENSIONS.contains(&"sqlite3"));
    }

    /// The join-then-normalize the runtime command scope, the prompt-context
    /// resolver and the review directory guard each carried before sharing it.
    fn previous_copy(path: &Path) -> std::io::Result<PathBuf> {
        let source = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        Ok(lexically_normalized(&source))
    }

    #[test]
    fn absolute_paths_match_the_copies_they_replace() {
        let cwd = std::env::current_dir().unwrap();
        for input in [
            "",
            ".",
            "..",
            "a/b",
            "./a/../b/",
            "/",
            "/a/./b/../c/",
            "//a//b",
            "/..",
            "../../../..",
        ] {
            let path = Path::new(input);
            assert_eq!(
                lexically_absolute(path).unwrap(),
                previous_copy(path).unwrap(),
                "{input:?}"
            );
        }
        assert_eq!(lexically_absolute(Path::new("")).unwrap(), cwd);
        assert_eq!(
            lexically_absolute(Path::new("/a/./b/../c/")).unwrap(),
            PathBuf::from("/a/c")
        );
    }

    #[test]
    fn sensitive_names_cover_env_keys_ssh_keys_and_credential_names() {
        for name in [
            ".env",
            ".env.local",
            "prod.env",
            "server.key",
            "cert.pem",
            "bundle.p12",
            "bundle.pfx",
            "release.keystore",
            "release.jks",
            "putty.ppk",
            "id_rsa",
            "id_rsa.pub",
            "id_ed25519",
            "id_ecdsa_sk",
            "id_dsa",
            "credentials.json",
            "my-credential",
            "service-account.json",
            "service_account.json",
            ".ssh",
            "secrets",
        ] {
            assert!(is_sensitive_name(name), "{name}");
        }
        for name in [
            ".env.example",
            "main.rs",
            "keys.rs",
            "environment.ts",
            "identity.rs",
            "pem",
            "id.rs",
            "readme.md",
        ] {
            assert!(!is_sensitive_name(name), "{name}");
        }
        for entry in SENSITIVE_EXTENSIONS.iter().chain(PRIVATE_KEY_PREFIXES) {
            assert_eq!(*entry, entry.to_lowercase(), "{entry}");
        }
    }
}
