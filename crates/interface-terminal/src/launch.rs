use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use interface_common::uuid::{canonical_uuid, canonical_uuid_v4};
use interface_contracts::terminal::{
    TERMINAL_LAUNCH_REQUEST_VERSION, TERMINAL_LAUNCH_SESSION_VERSION, TerminalLaunchProfile,
    TerminalLaunchProfileId, TerminalLaunchRequestV1, TerminalLaunchSessionV1,
};
use thiserror::Error;

use crate::TerminalError;
use crate::codex_names::codex_rollout_candidates;
use crate::opencode_session::opencode_session_id;

const MAX_ARGUMENTS: usize = 32;
const MAX_ARGUMENT_BYTES: usize = 4_096;
const MAX_ENVIRONMENT_ENTRIES: usize = 16;
const MAX_ENVIRONMENT_BYTES: usize = 16_384;

/// Private `interface-host` mode installed as a Claude `SessionStart` hook.
///
/// The hook commits provider metadata to a host-private receipt and emits
/// [`CLAUDE_SESSION_BINDING_TITLE_PREFIX`] followed by a generation wake token
/// through an OSC 2 terminal-title sequence. Keeping these literals in the
/// terminal crate prevents the launcher, helper, and PTY parser from silently
/// drifting apart.
pub const CLAUDE_SESSION_HOOK_MODE: &str = "--terminal-claude-session-hook";
pub const CLAUDE_SESSION_BINDING_TITLE_PREFIX: &str = "interface-claude-binding:";
/// OSC 2 title prefix the private Claude `UserPromptSubmit` and `Stop` hooks
/// emit, followed by `working` or `idle`. Claude Code never animates its own
/// title, so this frame is the only way a Claude pane can report that a turn
/// is running. The title scanner drops it and the renderer's activity monitor
/// consumes it; `electron/src/renderer/terminal-activity.ts` mirrors the
/// literal.
pub const CLAUDE_ACTIVITY_TITLE_PREFIX: &str = "interface-claude-activity:";
/// Replaced with a fresh, host-minted token for every Claude PTY generation.
///
/// The placeholder lives only inside the host-owned inline hook settings. A
/// resolved launch must replace every occurrence before spawning Claude; the
/// helper echoes the token in its private OSC frame so unrelated project hooks
/// and ordinary child output cannot select another generation's receipt.
pub const CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER: &str =
    "__INTERFACE_CLAUDE_SESSION_CAPTURE_TOKEN__";
/// Reserved host argv mode for the private Codex `SessionStart` hook.
pub const CODEX_SESSION_HOOK_MODE: &str = "--terminal-codex-session-hook";
/// Environment variable carrying one Codex pane's capture token.
///
/// Codex hooks inherit the pane's environment. Keeping the token out of the
/// hook command keeps the command identical for every pane, so Codex's
/// one-time hook-trust hash covers all of them.
pub const CODEX_SESSION_CAPTURE_TOKEN_ENV: &str = "INTERFACE_CODEX_CAPTURE_TOKEN";

/// A validated direct program selected by the host for one fixed launch
/// profile. The renderer can name the profile, but can never populate this
/// executable, argv, or environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalLaunchProgram {
    profile_id: TerminalLaunchProfileId,
    executable: PathBuf,
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    captures_provider_session: bool,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum TerminalLaunchProgramError {
    #[error("the shell profile cannot be replaced with a renderer-selectable program")]
    ShellProfile,
    #[error("the terminal launch executable is not a safe absolute executable file")]
    UnsafeExecutable,
    #[error("the terminal launch argv is not a bounded inert argument vector")]
    InvalidArguments,
    #[error("the terminal launch environment is not a bounded inert mapping")]
    InvalidEnvironment,
    #[error("provider-session capture is supported only for the Claude profile")]
    UnsupportedSessionCaptureProfile,
    #[error("the Claude provider-session hook is missing its capture-token placeholder")]
    MissingSessionCaptureTokenPlaceholder,
}

impl TerminalLaunchProgram {
    /// Validate one host-resolved direct program, retaining its launcher path.
    ///
    /// Installers advance symlinks such as `bin/codex -> current/bin/codex`.
    /// Retaining the canonical target here would pin every future pane to the
    /// release installed when the host started. Resolve it anew for each launch.
    ///
    /// # Errors
    ///
    /// Refuses the reserved shell profile, a missing/relative/non-executable
    /// file, oversized argv, or malformed environment entries.
    pub fn new(
        profile_id: TerminalLaunchProfileId,
        executable: impl AsRef<Path>,
        arguments: Vec<OsString>,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Self, TerminalLaunchProgramError> {
        if profile_id == TerminalLaunchProfileId::Shell {
            return Err(TerminalLaunchProgramError::ShellProfile);
        }
        let executable = executable.as_ref().to_path_buf();
        canonical_executable(&executable).ok_or(TerminalLaunchProgramError::UnsafeExecutable)?;
        if arguments.len() > MAX_ARGUMENTS
            || arguments.iter().any(|argument| {
                argument.is_empty()
                    || argument.as_encoded_bytes().len() > MAX_ARGUMENT_BYTES
                    || argument.as_encoded_bytes().contains(&0)
            })
        {
            return Err(TerminalLaunchProgramError::InvalidArguments);
        }
        if !valid_environment(&environment) {
            return Err(TerminalLaunchProgramError::InvalidEnvironment);
        }
        Ok(Self {
            profile_id,
            executable,
            arguments,
            environment,
            captures_provider_session: false,
        })
    }

    /// Mark a supported native launcher whose session identity the
    /// host captures.
    ///
    /// The marker carries no renderer-controlled process material. It only
    /// tells the terminal service not to publish the caller's launch UUID as
    /// provider truth before the host proves what the provider actually
    /// adopted. Claude's hook takes the token in argv; Codex's takes it from
    /// [`CODEX_SESSION_CAPTURE_TOKEN_ENV`] at spawn; `OpenCode`'s TUI transport
    /// receives a token for its process-scoped selected-session observation.
    ///
    /// # Errors
    ///
    /// Only launchers with a verified native capture contract can opt in.
    pub fn with_provider_session_capture(mut self) -> Result<Self, TerminalLaunchProgramError> {
        match self.profile_id {
            TerminalLaunchProfileId::Claude => {
                if !self.arguments.iter().any(|argument| {
                    argument.to_str().is_some_and(|argument| {
                        argument.contains(CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER)
                    })
                }) {
                    return Err(TerminalLaunchProgramError::MissingSessionCaptureTokenPlaceholder);
                }
            }
            TerminalLaunchProfileId::Codex
            | TerminalLaunchProfileId::Opencode
            | TerminalLaunchProfileId::Gemini
            | TerminalLaunchProfileId::Muse
            | TerminalLaunchProfileId::Kimi => {}
            _ => return Err(TerminalLaunchProgramError::UnsupportedSessionCaptureProfile),
        }
        self.captures_provider_session = true;
        Ok(self)
    }
}

/// Spawn-time command resolved for one terminal generation, including restarts.
/// Direct programs never pass through a shell or renderer-owned split.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedTerminalLaunch {
    LoginShell,
    LoginShellWithEnvironment {
        environment: Vec<(OsString, OsString)>,
    },
    Program {
        executable: PathBuf,
        arguments: Vec<OsString>,
        environment: Vec<(OsString, OsString)>,
    },
}

fn valid_environment(environment: &[(OsString, OsString)]) -> bool {
    environment.len() <= MAX_ENVIRONMENT_ENTRIES
        && environment.iter().all(|(name, value)| {
            let name = name.as_encoded_bytes();
            let value = value.as_encoded_bytes();
            !name.is_empty()
                && name.len().saturating_add(value.len()) <= MAX_ENVIRONMENT_BYTES
                && !name.contains(&0)
                && !name.contains(&b'=')
                && !value.contains(&0)
        })
}

impl TryFrom<&TerminalLaunchProgram> for ResolvedTerminalLaunch {
    type Error = TerminalError;

    fn try_from(program: &TerminalLaunchProgram) -> Result<Self, Self::Error> {
        let executable = canonical_executable(&program.executable)
            .ok_or(TerminalError::UnavailableLaunchProfile(program.profile_id))?;
        Ok(Self::Program {
            executable,
            arguments: program.arguments.clone(),
            environment: program.environment.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum TerminalLaunchCatalogError {
    #[error("the terminal launch catalog contains a duplicate profile")]
    DuplicateProfile,
    #[error("the pane environment is not a bounded inert mapping")]
    InvalidPaneEnvironment,
}

/// The argv of a profile CLI's own sign-in flow, for the profiles that have
/// one. Each opens the provider's browser sign-in and exits when it is done.
#[must_use]
pub const fn sign_in_arguments(
    profile: TerminalLaunchProfileId,
) -> Option<&'static [&'static str]> {
    match profile {
        TerminalLaunchProfileId::Claude => Some(&["auth", "login"]),
        TerminalLaunchProfileId::Codex | TerminalLaunchProfileId::Grok => Some(&["login"]),
        _ => None,
    }
}

/// What an added provider account (protocol 60) changes in one terminal
/// launch: its directory variable on the profile's environment, and the
/// stores a Codex or Grok reopen checks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalAccountLaunch {
    pub environment: Vec<(OsString, OsString)>,
    pub codex_sessions_root: Option<PathBuf>,
    pub grok_sessions_root: Option<PathBuf>,
}

/// The host's provider accounts, as a terminal launch needs them.
pub trait TerminalAccounts: Send + Sync {
    /// What `account` (absent: the profile's default account, or the account
    /// `session`'s chat is bound to when it names one) changes in a launch
    /// of `profile`. `None` is the Default account: nothing changes.
    ///
    /// # Errors
    /// A named account that does not exist.
    fn launch(
        &self,
        profile: TerminalLaunchProfileId,
        account: Option<&str>,
        session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<Option<TerminalAccountLaunch>, String>;
}

/// A catalog of host-owned terminal launch programs. The service holds one
/// at a time and swaps the whole of it when Models & providers → Refresh (or
/// the launcher's Try again) finds a CLI installed while Interface runs; a
/// launch reads one catalog from start to spawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalLaunchCatalog {
    programs: BTreeMap<TerminalLaunchProfileId, TerminalLaunchProgram>,
    /// Process-local environment injected only into login shells so a manual
    /// Claude invocation can opt into the same host-authenticated capture path
    /// as a curated Claude pane. Values may contain the per-generation token
    /// placeholder and never leave the Rust launch owner.
    shell_provider_capture_environment: Option<Vec<(OsString, OsString)>>,
    /// Codex's `~/.codex/sessions` store. A captured thread id is only asked
    /// to resume while its rollout still exists there.
    codex_sessions_root: Option<PathBuf>,
    /// Grok's `~/.grok/sessions` store. A minted chat id is asked to resume
    /// once Grok holds that session there, and opened as new before that.
    grok_sessions_root: Option<PathBuf>,
    /// Antigravity's `~/.gemini/antigravity-cli` store, read for conversation
    /// titles and first-request fallbacks of already-bound `agy-<uuid>` panes.
    antigravity_root: Option<PathBuf>,
    /// Host-owned variables every pane gets, shells included: today the path
    /// of the pane MCP socket, which the static Grok/Antigravity config entry
    /// reads. Appended last, so a profile's own environment cannot hide it.
    pane_environment: Vec<(OsString, OsString)>,
}

impl TerminalLaunchCatalog {
    pub(crate) fn codex_sessions_root(&self) -> Option<&Path> {
        self.codex_sessions_root.as_deref()
    }

    pub(crate) fn grok_sessions_root(&self) -> Option<&Path> {
        self.grok_sessions_root.as_deref()
    }

    pub(crate) fn antigravity_root(&self) -> Option<&Path> {
        self.antigravity_root.as_deref()
    }
    #[must_use]
    pub const fn shell_only() -> Self {
        Self {
            programs: BTreeMap::new(),
            shell_provider_capture_environment: None,
            codex_sessions_root: None,
            grok_sessions_root: None,
            antigravity_root: None,
            pane_environment: Vec::new(),
        }
    }

    /// Point Codex reopens at the store `codex resume <id>` reads, so a thread
    /// the user deleted or archived falls back instead of exiting.
    #[must_use]
    pub fn with_codex_sessions_root(mut self, root: PathBuf) -> Self {
        self.codex_sessions_root = Some(root);
        self
    }

    /// Point Grok reopens at the store `grok --resume <id>` reads. Grok keeps
    /// the id Interface minted, so whether that store already holds the chat
    /// is what separates a new `--session-id` from a `--resume`.
    #[must_use]
    pub fn with_grok_sessions_root(mut self, root: PathBuf) -> Self {
        self.grok_sessions_root = Some(root);
        self
    }

    /// Point Antigravity title lookups at `~/.gemini/antigravity-cli` so a pane
    /// with a captured `agy-<uuid>` conversation adopts its generated title or
    /// first request.
    #[must_use]
    pub fn with_antigravity_root(mut self, root: PathBuf) -> Self {
        self.antigravity_root = Some(root);
        self
    }

    /// Returns the validated host-selected launcher path for a built-in profile.
    /// Its current canonical target is checked again for every new generation.
    /// Process material remains absent from the public availability projection.
    #[must_use]
    pub fn executable(&self, profile_id: TerminalLaunchProfileId) -> Option<&Path> {
        self.programs
            .get(&profile_id)
            .map(|program| program.executable.as_path())
    }

    /// Whether this fixed profile has a host-owned provider identity capture
    /// path. The renderer never selects or configures this capability.
    #[must_use]
    pub fn captures_provider_session(&self, profile_id: TerminalLaunchProfileId) -> bool {
        if profile_id == TerminalLaunchProfileId::Shell {
            return self.shell_provider_capture_environment.is_some();
        }
        self.programs
            .get(&profile_id)
            .is_some_and(|program| program.captures_provider_session)
    }

    /// Build the catalog from host-validated fixed programs.
    ///
    /// # Errors
    ///
    /// Duplicate stable profile identifiers are refused.
    pub fn new(
        programs: impl IntoIterator<Item = TerminalLaunchProgram>,
    ) -> Result<Self, TerminalLaunchCatalogError> {
        let mut by_id = BTreeMap::new();
        for program in programs {
            if by_id.insert(program.profile_id, program).is_some() {
                return Err(TerminalLaunchCatalogError::DuplicateProfile);
            }
        }
        Ok(Self {
            programs: by_id,
            shell_provider_capture_environment: None,
            codex_sessions_root: None,
            grok_sessions_root: None,
            antigravity_root: None,
            pane_environment: Vec::new(),
        })
    }

    /// Variables injected into every pane launch, login shells included.
    ///
    /// # Errors
    ///
    /// Refuses malformed or oversized entries, the same bound as a program's
    /// own environment.
    pub fn with_pane_environment(
        mut self,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Self, TerminalLaunchCatalogError> {
        if !valid_environment(&environment) {
            return Err(TerminalLaunchCatalogError::InvalidPaneEnvironment);
        }
        self.pane_environment = environment;
        Ok(self)
    }

    /// Install a host-owned login-shell environment that can launch Claude
    /// through Interface's private capture wrapper.
    ///
    /// The environment is fixed during bootstrap. A fresh UUID placeholder is
    /// required so one shell generation cannot wake or claim another pane.
    ///
    /// # Errors
    ///
    /// Refuses malformed or oversized environment entries and templates that
    /// do not carry the capture-token placeholder.
    pub fn with_shell_provider_session_capture(
        mut self,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Self, TerminalLaunchProgramError> {
        if !valid_environment(&environment) {
            return Err(TerminalLaunchProgramError::InvalidEnvironment);
        }
        if !environment.iter().any(|(_, value)| {
            value
                .to_str()
                .is_some_and(|value| value.contains(CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER))
        }) {
            return Err(TerminalLaunchProgramError::MissingSessionCaptureTokenPlaceholder);
        }
        self.shell_provider_capture_environment = Some(environment);
        Ok(self)
    }

    /// Public availability projection in stable UI order. Executable paths,
    /// argv, and environment never leave this catalog.
    #[must_use]
    pub fn profiles(&self) -> Vec<TerminalLaunchProfile> {
        PROFILE_ORDER
            .into_iter()
            .map(|id| {
                let definition = profile_definition(id);
                let available = id == TerminalLaunchProfileId::Shell
                    || self.programs.contains_key(&id);
                TerminalLaunchProfile {
                    id,
                    engine: definition.engine.map(str::to_owned),
                    label: definition.label.to_owned(),
                    available,
                    unavailable_reason: (!available).then(|| {
                        format!(
                            "{} CLI was not found in the login shell or a supported install location.",
                            definition.label
                        )
                    }),
                    install_hint: (!available).then(|| install_hint(id)).flatten(),
                    version: None,
                    pane_tools: None,
                    changed_since_last_check: None,
                }
            })
            .collect()
    }

    /// This catalog with `account` applied to `profile`: its environment
    /// after the profile's own, and its Codex and Grok stores for a reopen.
    ///
    /// # Errors
    /// The combined environment is not a bounded inert mapping.
    pub fn for_account(
        &self,
        profile: TerminalLaunchProfileId,
        account: &TerminalAccountLaunch,
    ) -> Result<Self, TerminalError> {
        let mut catalog = self.clone();
        if let Some(program) = catalog.programs.get_mut(&profile) {
            program
                .environment
                .extend(account.environment.iter().cloned());
            if !valid_environment(&program.environment) {
                return Err(TerminalError::AccountUnavailable(
                    "its environment is not a bounded inert mapping".into(),
                ));
            }
        }
        if let Some(root) = &account.codex_sessions_root {
            catalog.codex_sessions_root = Some(root.clone());
        }
        if let Some(root) = &account.grok_sessions_root {
            catalog.grok_sessions_root = Some(root.clone());
        }
        Ok(catalog)
    }

    /// The profile's CLI running its own sign-in command (`claude auth
    /// login`, `codex login`, `grok login`) instead of a chat, with the
    /// profile's environment and none of its chat or capture arguments.
    ///
    /// # Errors
    ///
    /// The request version is unknown, the profile has no sign-in command,
    /// or its CLI is not available.
    pub fn resolve_sign_in(
        &self,
        request: Option<TerminalLaunchRequestV1>,
    ) -> Result<ResolvedTerminalLaunch, TerminalError> {
        let request = request.ok_or(TerminalError::NoSignIn(TerminalLaunchProfileId::Shell))?;
        if request.version != TERMINAL_LAUNCH_REQUEST_VERSION {
            return Err(TerminalError::UnsupportedLaunchVersion(request.version));
        }
        let arguments = sign_in_arguments(request.profile_id)
            .ok_or(TerminalError::NoSignIn(request.profile_id))?;
        let program = self
            .programs
            .get(&request.profile_id)
            .ok_or(TerminalError::UnavailableLaunchProfile(request.profile_id))?;
        let executable = canonical_executable(&program.executable)
            .ok_or(TerminalError::UnavailableLaunchProfile(request.profile_id))?;
        Ok(
            self.with_pane_environment_applied(ResolvedTerminalLaunch::Program {
                executable,
                arguments: arguments.iter().map(OsString::from).collect(),
                environment: program.environment.clone(),
            }),
        )
    }

    /// Resolve a launch that knows whether a live Codex terminal already holds
    /// this cwd. Only the Codex reopen reads `codex_cwd_shared`: two panes told
    /// to take the same folder's newest thread fight over one writer lock and
    /// all but the first die during bootstrap.
    ///
    /// # Errors
    ///
    /// Returns an explicit version or availability error before lease/spawn.
    pub fn resolve_with_peers(
        &self,
        request: Option<TerminalLaunchRequestV1>,
        session: Option<&TerminalLaunchSessionV1>,
        codex_cwd_shared: bool,
        provider_session_capture_token: Option<&str>,
    ) -> Result<ResolvedTerminalLaunch, TerminalError> {
        self.resolve_program(
            request,
            session,
            codex_cwd_shared,
            provider_session_capture_token,
        )
        .map(|launch| self.with_pane_environment_applied(launch))
    }

    fn resolve_program(
        &self,
        request: Option<TerminalLaunchRequestV1>,
        session: Option<&TerminalLaunchSessionV1>,
        codex_cwd_shared: bool,
        provider_session_capture_token: Option<&str>,
    ) -> Result<ResolvedTerminalLaunch, TerminalError> {
        let Some(request) = request else {
            return self.resolve_login_shell(provider_session_capture_token);
        };
        if request.version != TERMINAL_LAUNCH_REQUEST_VERSION {
            return Err(TerminalError::UnsupportedLaunchVersion(request.version));
        }
        if request.profile_id == TerminalLaunchProfileId::Shell {
            // A login shell has no chat to be pointed at, and silently dropping
            // a binding the caller asked for would restore the wrong thing.
            if session.is_some() {
                return Err(TerminalError::UnbindableLaunchProfile(request.profile_id));
            }
            return self.resolve_login_shell(provider_session_capture_token);
        }
        let program = self
            .programs
            .get(&request.profile_id)
            .ok_or(TerminalError::UnavailableLaunchProfile(request.profile_id))?;
        let mut resolved = ResolvedTerminalLaunch::try_from(program)?;
        bind_provider_session_capture(
            &mut resolved,
            request.profile_id,
            program.captures_provider_session,
            provider_session_capture_token,
        )?;
        if let Some(session) = session {
            bind_session(
                &mut resolved,
                request.profile_id,
                session,
                codex_cwd_shared,
                self.codex_sessions_root.as_deref(),
                self.grok_sessions_root.as_deref(),
            )?;
        }
        Ok(resolved)
    }

    /// The pane environment goes on last, after every profile-specific
    /// binding, so a plain login shell becomes one with an environment and a
    /// program's own entries never shadow it.
    fn with_pane_environment_applied(
        &self,
        launch: ResolvedTerminalLaunch,
    ) -> ResolvedTerminalLaunch {
        if self.pane_environment.is_empty() {
            return launch;
        }
        match launch {
            ResolvedTerminalLaunch::LoginShell => {
                ResolvedTerminalLaunch::LoginShellWithEnvironment {
                    environment: self.pane_environment.clone(),
                }
            }
            ResolvedTerminalLaunch::LoginShellWithEnvironment { mut environment } => {
                environment.extend(self.pane_environment.iter().cloned());
                ResolvedTerminalLaunch::LoginShellWithEnvironment { environment }
            }
            ResolvedTerminalLaunch::Program {
                executable,
                arguments,
                mut environment,
            } => {
                environment.extend(self.pane_environment.iter().cloned());
                ResolvedTerminalLaunch::Program {
                    executable,
                    arguments,
                    environment,
                }
            }
        }
    }

    fn resolve_login_shell(
        &self,
        provider_session_capture_token: Option<&str>,
    ) -> Result<ResolvedTerminalLaunch, TerminalError> {
        let Some(template) = self.shell_provider_capture_environment.as_ref() else {
            return Ok(ResolvedTerminalLaunch::LoginShell);
        };
        let token = provider_session_capture_token
            .filter(|token| canonical_uuid_v4(token))
            .ok_or(TerminalError::ProviderSessionCaptureUnavailable(
                TerminalLaunchProfileId::Claude,
            ))?;
        let mut environment = template.clone();
        let replacements =
            replace_capture_placeholder(environment.iter_mut().map(|(_, value)| value), token);
        if replacements == 0 {
            return Err(TerminalError::ProviderSessionCaptureUnavailable(
                TerminalLaunchProfileId::Claude,
            ));
        }
        Ok(ResolvedTerminalLaunch::LoginShellWithEnvironment { environment })
    }
}

/// Replace the inert bootstrap placeholder with one generation's correlation
/// token immediately before spawn. It only coalesces lookup wake-ups on the
/// matching PTY; provider metadata, never this token, owns session identity.
fn bind_provider_session_capture(
    launch: &mut ResolvedTerminalLaunch,
    profile_id: TerminalLaunchProfileId,
    enabled: bool,
    token: Option<&str>,
) -> Result<(), TerminalError> {
    if !enabled {
        return Ok(());
    }
    let token = token
        .filter(|token| canonical_uuid_v4(token))
        .ok_or(TerminalError::ProviderSessionCaptureUnavailable(profile_id))?;
    let ResolvedTerminalLaunch::Program {
        arguments,
        environment,
        ..
    } = launch
    else {
        return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
    };
    if profile_id == TerminalLaunchProfileId::Codex {
        environment.push((
            OsString::from(CODEX_SESSION_CAPTURE_TOKEN_ENV),
            OsString::from(token),
        ));
        return Ok(());
    }
    if profile_id == TerminalLaunchProfileId::Opencode {
        environment.push((
            OsString::from("INTERFACE_OPENCODE_CAPTURE_TOKEN"),
            OsString::from(token),
        ));
        return Ok(());
    }
    if crate::native_checkpoint::supports(profile_id) {
        environment.push((
            OsString::from(crate::native_checkpoint::TOKEN_ENV),
            OsString::from(token),
        ));
        replace_capture_placeholder(arguments.iter_mut(), token);
        return Ok(());
    }
    if replace_capture_placeholder(arguments.iter_mut(), token) == 0 {
        return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
    }
    Ok(())
}

/// Put `token` in place of every capture-token placeholder in `values`, and
/// count the replacements. A value that is not UTF-8 cannot hold one.
fn replace_capture_placeholder<'a>(
    values: impl Iterator<Item = &'a mut OsString>,
    token: &str,
) -> usize {
    let mut replacements = 0_usize;
    for value in values {
        let Some(text) = value.to_str() else {
            continue;
        };
        let count = text
            .match_indices(CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER)
            .count();
        if count == 0 {
            continue;
        }
        *value = OsString::from(text.replace(CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER, token));
        replacements = replacements.saturating_add(count);
    }
    replacements
}

/// Codex mints RFC 4122 version-7 thread ids; the renderer mints version 4.
pub(crate) fn codex_thread_id(value: &str) -> bool {
    canonical_uuid(value, b'7')
}

/// Whether Codex still holds thread `id` under `sessions/YYYY/MM/DD/`. The id
/// has already been validated as a canonical UUID.
fn codex_rollout_exists(sessions_root: &Path, id: &str) -> bool {
    codex_rollout_candidates(sessions_root, id, usize::MAX)
        .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
}

/// Whether Grok's store holds session `id`: `sessions/<encoded cwd>/<id>/`.
///
/// The cwd folder is Grok's own percent-encoding of the path, so every project
/// folder is scanned for the id instead of re-deriving that encoding. `id` was
/// already validated as a bare identifier by `validated_session_id`.
fn grok_session_exists(sessions_root: &Path, id: &str) -> bool {
    std::fs::read_dir(sessions_root)
        .map(|projects| {
            projects.flatten().any(|project| {
                project.file_type().is_ok_and(|kind| kind.is_dir())
                    && project.path().join(id).is_dir()
            })
        })
        .unwrap_or(false)
}

/// Rewrite a resolved program so it opens one named chat.
///
/// The chat id is renderer-supplied and lands in argv, so it is validated as a
/// bare identifier here rather than trusted or escaped downstream.
fn bind_session(
    launch: &mut ResolvedTerminalLaunch,
    profile_id: TerminalLaunchProfileId,
    session: &TerminalLaunchSessionV1,
    codex_cwd_shared: bool,
    codex_sessions_root: Option<&Path>,
    grok_sessions_root: Option<&Path>,
) -> Result<(), TerminalError> {
    if session.version != TERMINAL_LAUNCH_SESSION_VERSION {
        return Err(TerminalError::UnsupportedLaunchSessionVersion(
            session.version,
        ));
    }
    let id = match session.id.as_deref() {
        None => None,
        Some(id) => Some(validated_session_id(id)?),
    };
    let ResolvedTerminalLaunch::Program {
        arguments,
        environment,
        ..
    } = launch
    else {
        return Err(TerminalError::UnbindableLaunchProfile(profile_id));
    };
    match profile_id {
        TerminalLaunchProfileId::Claude => {
            // A missing ID opens Claude's native picker, whatever the saved
            // `resume` flag says. The host reaches here without an id in two
            // ways: a recovery resume after it proved a legacy saved UUID
            // absent, and a cold reopen of a pane whose SessionStart receipt
            // never arrived — `public_provider_session_state` files a fresh
            // Claude launch as id-less `resume:false`, and the reopen
            // authority replays that shape. Neither has a fresh chat to mint
            // (the renderer always mints one), so refusing here left such a
            // pane failing on every restart. The hook then binds the exact
            // provider ID selected on this PTY.
            let Some(id) = id else {
                arguments.push(OsString::from("--resume"));
                return Ok(());
            };
            let flag = if session.resume {
                "--resume"
            } else {
                "--session-id"
            };
            arguments.push(OsString::from(format!("{flag}={id}")));
        }
        // Grok accepts the id Interface mints, so the chat is exact in either
        // direction. The saved `resume` flag cannot decide which direction:
        // no hook ever advances a Grok binding in the host-private reopen
        // authority, so it stays `resume:false` for life, and `--session-id`
        // on an id Grok already holds exits with "Session ID … is already in
        // use". Grok's own store decides; the flag stands in only when no
        // store root is configured.
        TerminalLaunchProfileId::Grok => {
            let Some(id) = id else {
                return Err(TerminalError::InvalidLaunchSessionId);
            };
            let resume =
                grok_sessions_root.map_or(session.resume, |root| grok_session_exists(root, &id));
            let flag = if resume { "--resume" } else { "--session-id" };
            arguments.push(OsString::from(format!("{flag}={id}")));
        }
        // `codex` assigns its own provider id and has no flag to be given one.
        // The exact thread id is what its `SessionStart` hook reported through
        // Interface's capture receipt; only that reopens as `codex resume <id>`.
        // ponytail: a captured id is told apart from a legacy renderer-minted
        // handle by UUID version alone (Codex mints v7, Interface mints v4).
        // Without one, a reopen can only take this folder's newest thread, and
        // only while no other live Codex pane holds this cwd, because two panes
        // pointed at one thread collide on its writer lock and every loser dies
        // during bootstrap; with a peer there it is codex's own cwd-scoped
        // picker. A captured thread whose rollout has left the sessions store
        // (deleted or archived) opens fresh: `codex resume <id>` would exit,
        // and the renderer already refuses folder-global history for a pane
        // that never had a thread of its own.
        TerminalLaunchProfileId::Codex => {
            if !session.resume {
                return Ok(());
            }
            // `resume` is a subcommand, so it has to lead the argument vector.
            let mut resume = vec![OsString::from("resume")];
            match id.as_deref().filter(|id| codex_thread_id(id)) {
                Some(id)
                    if codex_sessions_root.is_none_or(|root| codex_rollout_exists(root, id)) =>
                {
                    resume.push(OsString::from(id));
                }
                Some(_) => return Ok(()),
                None if !codex_cwd_shared => resume.push(OsString::from("--last")),
                None => {}
            }
            resume.append(arguments);
            *arguments = resume;
        }
        // Preserve an exact provider ID even when a bounded listing cannot
        // find it. OpenCode owns validation; lookup failure is not deletion.
        TerminalLaunchProfileId::Opencode => {
            if !session.resume {
                return Ok(());
            }
            // OpenCode mints its ids itself and takes no flag to pre-select
            // one, so a resumable id in a binding came from the provider.
            if let Some(id) = id.as_deref().filter(|id| opencode_session_id(id)) {
                arguments.push(OsString::from(format!("--session={id}")));
            } else {
                // Never guess the newest chat: sibling panes may share a cwd.
                // The private TUI bridge opens OpenCode's native picker.
                if !environment
                    .iter()
                    .any(|(name, _)| name == "INTERFACE_OPENCODE_RECEIPTS_ROOT")
                {
                    return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
                }
                environment.push((
                    OsString::from("INTERFACE_OPENCODE_RESUME_PICKER"),
                    OsString::from("1"),
                ));
            }
        }
        TerminalLaunchProfileId::Gemini
        | TerminalLaunchProfileId::Muse
        | TerminalLaunchProfileId::Kimi => {
            bind_native_session(arguments, profile_id, session.resume, id.as_deref())?;
        }
        TerminalLaunchProfileId::Omp
        | TerminalLaunchProfileId::Qwen
        | TerminalLaunchProfileId::Shell => {
            return Err(TerminalError::UnbindableLaunchProfile(profile_id));
        }
    }
    Ok(())
}

/// Antigravity, Muse Code and Kimi Code mint their own conversation ids and
/// take no flag to be given one: only the exact id a native hook reported
/// (`agy-`, `muse-` or `kimi-<uuid>`) reopens, each in its CLI's grammar.
/// The stable Gemini profile launches Antigravity, which does not accept the
/// retired Gemini CLI's `--session-id` / `--resume`; Kimi Code reopens as
/// `kimi --session session_<uuid>`.
fn bind_native_session(
    arguments: &mut Vec<OsString>,
    profile_id: TerminalLaunchProfileId,
    resume: bool,
    binding: Option<&str>,
) -> Result<(), TerminalError> {
    let unbindable = || TerminalError::UnbindableLaunchProfile(profile_id);
    let binding = binding.filter(|_| resume).ok_or_else(unbindable)?;
    if profile_id == TerminalLaunchProfileId::Kimi {
        let id = crate::native_checkpoint::kimi_session_argument(binding).ok_or_else(unbindable)?;
        arguments.extend([OsString::from("--session"), OsString::from(id)]);
        return Ok(());
    }
    let id = crate::native_checkpoint::session_id(profile_id, binding).ok_or_else(unbindable)?;
    if profile_id == TerminalLaunchProfileId::Gemini {
        arguments.extend([OsString::from("--conversation"), OsString::from(id)]);
    } else {
        arguments.splice(0..0, [OsString::from("resume"), OsString::from(id)]);
    }
    Ok(())
}

/// How a pass-off prompt reaches the new pane's CLI.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum HandoffDelivery {
    /// The prompt is on argv.
    Argument,
    /// The CLI takes no initial prompt, so the host types this into its TUI
    /// once the TUI is ready for input.
    Typed(String),
}

/// Bind the host-templated pass-off prompt using the selected provider's CLI
/// grammar.
///
/// The prompt is host-built (the renderer supplies only the transcript file's
/// CONTENT, never argv), but it is bounded here anyway because it bypasses
/// `TerminalLaunchProgram::new`, which validated the catalog argv at boot.
///
/// Call this *after* `bind_session`: its Codex arm prepends `resume --last`,
/// so binding a chat afterwards would push the prompt out of trailing position
/// and into `codex resume`'s positional `SESSION_ID` slot.
pub(crate) fn bind_handoff_prompt(
    launch: &mut ResolvedTerminalLaunch,
    profile_id: TerminalLaunchProfileId,
    prompt: &str,
) -> Result<HandoffDelivery, TerminalError> {
    if prompt.is_empty() || prompt.len() > MAX_ARGUMENT_BYTES || prompt.contains('\0') {
        return Err(TerminalError::InvalidHandoff);
    }
    let ResolvedTerminalLaunch::Program { arguments, .. } = launch else {
        return Err(TerminalError::HandoffUnsupportedProfile(profile_id));
    };
    match profile_id {
        // Verified against each CLI's --help: an interactive session accepts
        // an initial prompt as a positional argument.
        TerminalLaunchProfileId::Claude
        | TerminalLaunchProfileId::Codex
        | TerminalLaunchProfileId::Grok
        | TerminalLaunchProfileId::Omp => {
            arguments.push(OsString::from(prompt));
            Ok(HandoffDelivery::Argument)
        }
        // OpenCode's positional argument is a project path, not a prompt.
        TerminalLaunchProfileId::Opencode => {
            arguments.push(OsString::from("--prompt"));
            arguments.push(OsString::from(prompt));
            Ok(HandoffDelivery::Argument)
        }
        // `agy --help` exposes initial interactive prompts through
        // `--prompt-interactive`; a bare positional prompt belongs to the
        // retired Gemini CLI and is rejected by current Antigravity builds.
        TerminalLaunchProfileId::Gemini => {
            arguments.push(OsString::from("--prompt-interactive"));
            arguments.push(OsString::from(prompt));
            Ok(HandoffDelivery::Argument)
        }
        // Kimi Code 2.3 rejects any positional argument and `-p` is headless
        // only, so the prompt is typed into its TUI instead. The host prompt
        // is one line: it can never submit early or answer a dialog itself.
        TerminalLaunchProfileId::Kimi if !prompt.contains(['\r', '\n']) => {
            Ok(HandoffDelivery::Typed(prompt.to_owned()))
        }
        // Muse Code documents no initial-prompt argument for its TUI (only
        // headless `muse exec`), so a pass-off cannot start one.
        TerminalLaunchProfileId::Muse
        | TerminalLaunchProfileId::Kimi
        | TerminalLaunchProfileId::Qwen
        | TerminalLaunchProfileId::Shell => {
            Err(TerminalError::HandoffUnsupportedProfile(profile_id))
        }
    }
}

fn validated_session_id(id: &str) -> Result<String, TerminalError> {
    let usable = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if usable {
        Ok(id.to_owned())
    } else {
        Err(TerminalError::InvalidLaunchSessionId)
    }
}

impl Default for TerminalLaunchCatalog {
    fn default() -> Self {
        Self::shell_only()
    }
}

// Personal launcher roster projected to Electron. Qwen remains a stable
// internal profile identifier and provider, but is intentionally absent from
// this owner's terminal-session composer.
pub(crate) const PROFILE_ORDER: [TerminalLaunchProfileId; 9] = [
    TerminalLaunchProfileId::Shell,
    TerminalLaunchProfileId::Claude,
    TerminalLaunchProfileId::Codex,
    TerminalLaunchProfileId::Grok,
    TerminalLaunchProfileId::Gemini,
    TerminalLaunchProfileId::Omp,
    TerminalLaunchProfileId::Opencode,
    TerminalLaunchProfileId::Muse,
    TerminalLaunchProfileId::Kimi,
];

struct ProfileDefinition {
    engine: Option<&'static str>,
    label: &'static str,
}

const fn profile_definition(id: TerminalLaunchProfileId) -> ProfileDefinition {
    match id {
        TerminalLaunchProfileId::Shell => ProfileDefinition {
            engine: None,
            label: "Terminal",
        },
        TerminalLaunchProfileId::Claude => ProfileDefinition {
            engine: Some("claude"),
            label: "Claude Code",
        },
        TerminalLaunchProfileId::Codex => ProfileDefinition {
            engine: Some("codex"),
            label: "Codex",
        },
        TerminalLaunchProfileId::Grok => ProfileDefinition {
            engine: Some("acp:grok"),
            label: "Grok",
        },
        TerminalLaunchProfileId::Qwen => ProfileDefinition {
            engine: Some("acp:qwen"),
            label: "Qwen",
        },
        TerminalLaunchProfileId::Opencode => ProfileDefinition {
            engine: Some("acp:opencode"),
            label: "OpenCode",
        },
        TerminalLaunchProfileId::Omp => ProfileDefinition {
            engine: Some("acp:omp"),
            label: "OMP",
        },
        TerminalLaunchProfileId::Muse => ProfileDefinition {
            engine: Some("acp:muse"),
            label: "Muse Code",
        },
        TerminalLaunchProfileId::Kimi => ProfileDefinition {
            engine: Some("acp:kimi"),
            label: "Kimi Code",
        },
        TerminalLaunchProfileId::Gemini => ProfileDefinition {
            engine: Some("acp:gemini"),
            label: "Antigravity",
        },
    }
}

/// How to install a profile's CLI, for a profile whose CLI is missing. The
/// two with one standard install say the command; the rest name the command
/// Interface looks for. Each ends with the Refresh that finds it.
#[must_use]
pub fn install_hint(id: TerminalLaunchProfileId) -> Option<String> {
    let command = match id {
        TerminalLaunchProfileId::Shell => return None,
        TerminalLaunchProfileId::Claude => {
            return Some(
                "Install it with `npm install -g @anthropic-ai/claude-code`, then press Refresh."
                    .to_owned(),
            );
        }
        TerminalLaunchProfileId::Codex => {
            return Some(
                "Install it with `npm install -g @openai/codex`, then press Refresh.".to_owned(),
            );
        }
        TerminalLaunchProfileId::Grok => "grok",
        TerminalLaunchProfileId::Qwen => "qwen",
        TerminalLaunchProfileId::Gemini => "agy",
        TerminalLaunchProfileId::Omp => "omp",
        TerminalLaunchProfileId::Opencode => "opencode",
        TerminalLaunchProfileId::Muse => "muse",
        TerminalLaunchProfileId::Kimi => "kimi",
    };
    Some(format!(
        "Install {} so `{command}` runs in your login shell, then press Refresh.",
        profile_definition(id).label
    ))
}

/// The name a launch profile goes by in the launcher and in pane listings.
#[must_use]
pub const fn profile_label(id: TerminalLaunchProfileId) -> &'static str {
    profile_definition(id).label
}

fn canonical_executable(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let canonical = fs::canonicalize(path).ok()?;
    let metadata = fs::metadata(&canonical).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return None;
    }
    Some(canonical)
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
