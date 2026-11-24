use std::{
    env::JoinPathsError,
    ffi::OsStr,
    ffi::OsString,
    path::{Path, PathBuf},
};

use interface_contracts::engine::EngineId;
use interface_engine::{
    ExecutableResolution, ProcessSpec, ResolvedExecutable, augment_search_path, resolve_executable,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_SLUG_BYTES: usize = 64;
const MAX_DISPLAY_NAME_BYTES: usize = 80;
const MAX_ARGS: usize = 64;
const MAX_ARG_BYTES: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinAcpProvider {
    Grok,
    Qwen,
}

/// Build the exact built-in stdio profile without invoking either provider.
///
/// # Errors
///
/// Returns the same strict profile validation errors as a custom profile.
pub fn builtin_profile(
    provider: BuiltinAcpProvider,
    executable_path: PathBuf,
) -> Result<ValidatedAcpProfile, AcpProfileError> {
    let (slug, display_name, args) = match provider {
        BuiltinAcpProvider::Grok => (
            "grok",
            "Grok",
            vec!["--no-auto-update".into(), "agent".into(), "stdio".into()],
        ),
        BuiltinAcpProvider::Qwen => ("qwen", "Qwen", vec!["--acp".into()]),
    };
    validate_profile(AcpProfile {
        slug: slug.into(),
        display_name: display_name.into(),
        executable_path,
        args,
    })
}

impl BuiltinAcpProvider {
    #[must_use]
    pub const fn executable_environment(self) -> &'static str {
        match self {
            Self::Grok => "GROK_EXECUTABLE",
            Self::Qwen => "QWEN_EXECUTABLE",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcpProfile {
    pub slug: String,
    pub display_name: String,
    pub executable_path: PathBuf,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedAcpProfile {
    pub engine_id: EngineId,
    pub slug: String,
    pub display_name: String,
    pub executable_path: PathBuf,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum AcpProfileError {
    #[error("ACP profile slug must be 1-64 lowercase letters, digits, or hyphens")]
    InvalidSlug,
    #[error("ACP profile display name must be 1-80 characters")]
    InvalidDisplayName,
    #[error("ACP profile executable path must be an absolute non-shell path")]
    InvalidExecutable,
    #[error("ACP profile args must contain at most 64 entries")]
    TooManyArguments,
    #[error("ACP profile args must be bounded strings without NUL bytes")]
    InvalidArgument,
    #[error("ACP profile args cannot carry credential-shaped flags or values")]
    CredentialShapedArgument,
}

/// Validate a configured ACP executable without inspecting credentials or invoking it.
///
/// # Errors
///
/// Returns a profile error for an unsafe identity, executable, or argv entry.
pub fn validate_profile(profile: AcpProfile) -> Result<ValidatedAcpProfile, AcpProfileError> {
    if !valid_slug(&profile.slug) {
        return Err(AcpProfileError::InvalidSlug);
    }
    let display_name = profile.display_name.trim().to_owned();
    if display_name.is_empty() || display_name.len() > MAX_DISPLAY_NAME_BYTES {
        return Err(AcpProfileError::InvalidDisplayName);
    }
    if !valid_executable(&profile.executable_path) {
        return Err(AcpProfileError::InvalidExecutable);
    }
    if profile.args.len() > MAX_ARGS {
        return Err(AcpProfileError::TooManyArguments);
    }
    for argument in &profile.args {
        if argument.len() > MAX_ARG_BYTES || argument.contains('\0') {
            return Err(AcpProfileError::InvalidArgument);
        }
        if credential_shaped(argument) {
            return Err(AcpProfileError::CredentialShapedArgument);
        }
    }
    let engine_id =
        EngineId::new(format!("acp:{}", profile.slug)).map_err(|_| AcpProfileError::InvalidSlug)?;
    Ok(ValidatedAcpProfile {
        engine_id,
        slug: profile.slug,
        display_name,
        executable_path: profile.executable_path,
        args: profile.args,
    })
}

#[must_use]
pub fn acp_process_spec(profile: &ValidatedAcpProfile, cwd: &Path) -> ProcessSpec {
    ProcessSpec {
        program: profile.executable_path.clone(),
        args: profile.args.iter().map(OsString::from).collect(),
        cwd: Some(cwd.to_path_buf()),
    }
}

/// Resolve override, login-shell result, then provider-specific known paths.
pub fn resolve_builtin_executable(
    provider: BuiltinAcpProvider,
    home: &Path,
    explicit_override: Option<PathBuf>,
    login_shell_path: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Option<ResolvedExecutable> {
    let known_locations = match provider {
        BuiltinAcpProvider::Grok => vec![
            home.join(".grok/bin/grok"),
            home.join(".local/bin/grok"),
            PathBuf::from("/opt/homebrew/bin/grok"),
            PathBuf::from("/usr/local/bin/grok"),
        ],
        BuiltinAcpProvider::Qwen => vec![
            home.join(".qwen/bin/qwen"),
            home.join(".local/bin/qwen"),
            home.join(".npm-global/bin/qwen"),
            PathBuf::from("/opt/homebrew/bin/qwen"),
            PathBuf::from("/usr/local/bin/qwen"),
        ],
    };
    resolve_executable(
        &ExecutableResolution {
            explicit_override,
            login_shell_path,
            known_locations,
            require_absolute: true,
        },
        exists,
    )
}

/// PATH inherited by a built-in ACP process and its tool subprocesses.
///
/// # Errors
///
/// Returns `JoinPathsError` if a supplied directory contains a PATH separator.
pub fn builtin_search_path(
    provider: BuiltinAcpProvider,
    current: Option<&OsStr>,
    home: Option<&Path>,
    executable: &Path,
) -> Result<OsString, JoinPathsError> {
    let mut prepend = Vec::new();
    if let Some(parent) = executable.parent() {
        prepend.push(parent.to_path_buf());
    }
    if let Some(home) = home.filter(|home| !home.as_os_str().is_empty()) {
        if matches!(provider, BuiltinAcpProvider::Grok) {
            prepend.push(home.join(".grok/bin"));
        }
        prepend.push(home.join(".local/bin"));
        if matches!(provider, BuiltinAcpProvider::Qwen) {
            prepend.push(home.join(".npm-global/bin"));
        }
    }
    prepend.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    augment_search_path(current, prepend)
}

fn valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > MAX_SLUG_BYTES {
        return false;
    }
    let bytes = slug.as_bytes();
    if bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-') {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn valid_executable(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .as_os_str()
            .to_string_lossy()
            .contains(['\0', '\r', '\n'])
    {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "sh" | "bash" | "zsh" | "dash" | "fish" | "env"
    )
}

fn credential_shaped(argument: &str) -> bool {
    let compact = argument.to_ascii_lowercase().replace(['-', '_'], "");
    ["apikey", "token", "secret", "password", "credential"]
        .iter()
        .any(|term| compact.contains(term))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> AcpProfile {
        AcpProfile {
            slug: "fake-agent".into(),
            display_name: "Fake Agent".into(),
            executable_path: "/opt/fake/bin/agent".into(),
            args: vec!["--acp".into()],
        }
    }

    #[test]
    fn valid_profile_becomes_an_exact_argv_process_spec() {
        let validated = validate_profile(profile()).unwrap();
        assert_eq!(validated.engine_id.as_str(), "acp:fake-agent");
        let spec = acp_process_spec(&validated, Path::new("/workspace"));
        assert_eq!(spec.program, PathBuf::from("/opt/fake/bin/agent"));
        assert_eq!(spec.args, vec![OsString::from("--acp")]);
        assert_eq!(spec.cwd, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn shell_wrappers_and_credential_argv_are_refused() {
        let mut shell = profile();
        shell.executable_path = "/bin/sh".into();
        assert_eq!(
            validate_profile(shell),
            Err(AcpProfileError::InvalidExecutable)
        );
        let mut credential = profile();
        credential.args = vec!["--api-key=value".into()];
        assert_eq!(
            validate_profile(credential),
            Err(AcpProfileError::CredentialShapedArgument)
        );
    }

    #[test]
    fn slug_is_bounded_and_cannot_escape_the_engine_namespace() {
        let mut invalid = profile();
        invalid.slug = "../grok".into();
        assert_eq!(validate_profile(invalid), Err(AcpProfileError::InvalidSlug));
    }

    #[test]
    fn search_path_prepends_provider_directories_and_keeps_the_inherited_tail() {
        let grok = builtin_search_path(
            BuiltinAcpProvider::Grok,
            Some(OsStr::new("/usr/bin:/usr/local/bin")),
            Some(Path::new("/Users/test")),
            Path::new("/resolved/bin/grok"),
        )
        .unwrap();
        assert_eq!(
            grok,
            OsString::from(
                "/resolved/bin:/Users/test/.grok/bin:/Users/test/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin"
            )
        );
        let qwen = builtin_search_path(
            BuiltinAcpProvider::Qwen,
            None,
            Some(Path::new("/Users/test")),
            Path::new("/resolved/bin/qwen"),
        )
        .unwrap();
        assert_eq!(
            qwen,
            OsString::from(
                "/resolved/bin:/Users/test/.local/bin:/Users/test/.npm-global/bin:/opt/homebrew/bin:/usr/local/bin"
            )
        );
    }

    #[test]
    fn search_path_omits_home_entries_when_home_is_absent_or_empty() {
        let absent = builtin_search_path(
            BuiltinAcpProvider::Grok,
            None,
            None,
            Path::new("/resolved/bin/grok"),
        )
        .unwrap();
        assert_eq!(
            absent,
            OsString::from("/resolved/bin:/opt/homebrew/bin:/usr/local/bin")
        );
        let empty = builtin_search_path(
            BuiltinAcpProvider::Grok,
            None,
            Some(Path::new("")),
            Path::new("/resolved/bin/grok"),
        )
        .unwrap();
        assert_eq!(empty, absent);
    }

    #[test]
    fn builtins_share_ordering_but_keep_provider_specific_paths() {
        let qwen = resolve_builtin_executable(
            BuiltinAcpProvider::Qwen,
            Path::new("/Users/test"),
            Some("/stale/qwen".into()),
            None,
            |path| path == Path::new("/Users/test/.npm-global/bin/qwen"),
        )
        .unwrap();
        assert_eq!(qwen.path, PathBuf::from("/Users/test/.npm-global/bin/qwen"));
        assert_eq!(
            BuiltinAcpProvider::Grok.executable_environment(),
            "GROK_EXECUTABLE"
        );
    }
}
