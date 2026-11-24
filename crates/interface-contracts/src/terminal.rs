//! Versioned contracts for integrated terminal sessions.
//!
//! Terminal processes are deliberately separate from provider sessions. A
//! terminal may host an ordinary shell, a full-screen TUI, or a coding-agent
//! CLI, but none of those possibilities grants it provider-registry identity.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ValidationError;
use crate::engine::SessionKey;

/// Stable caller-minted identity for one logical terminal entry.
///
/// Creation is idempotent by this identifier. Restarting the process keeps the
/// identifier and advances [`TerminalGeneration`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TerminalId(Uuid);

impl TerminalId {
    #[must_use]
    pub const fn new(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for TerminalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Fences late input/output from an older process using the same terminal ID.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct TerminalGeneration(pub u64);

impl TerminalGeneration {
    #[must_use]
    pub const fn initial() -> Self {
        Self(1)
    }

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Character-cell dimensions used by both the PTY and renderer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            columns: 100,
            rows: 30,
        }
    }
}

/// Observable lifecycle state. `Exited` and `Failed` entries remain available
/// for explicit restart or dismissal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalState {
    Starting,
    Running,
    Exited,
    Failed,
}

/// Bounded process completion metadata. A platform may report a code, signal,
/// both, or neither; the renderer must not invent missing information.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalExit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
}

/// The fixed, host-owned programs that may be launched inside an integrated
/// terminal. These identifiers are requests for a curated profile, never an
/// executable name or command string supplied by the renderer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalLaunchProfileId {
    Shell,
    Claude,
    Codex,
    Grok,
    Qwen,
    /// Stable legacy wire id for the Antigravity CLI.
    Gemini,
    /// Oh My Pi terminal profile (host protocol v7).
    Omp,
    /// `OpenCode` terminal profile (host protocol v9).
    Opencode,
    /// Meta's Muse Code terminal profile (host protocol v35).
    Muse,
    /// Moonshot's Kimi Code terminal profile (host protocol v54).
    Kimi,
}

impl TerminalLaunchProfileId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Qwen => "qwen",
            Self::Gemini => "gemini",
            Self::Omp => "omp",
            Self::Opencode => "opencode",
            Self::Muse => "muse",
            Self::Kimi => "kimi",
        }
    }
}

impl fmt::Display for TerminalLaunchProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub const TERMINAL_LAUNCH_REQUEST_VERSION: u8 = 1;

/// Versioned renderer intent for one curated terminal launch profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalLaunchRequestV1 {
    pub version: u8,
    pub profile_id: TerminalLaunchProfileId,
}

pub const TERMINAL_LAUNCH_SESSION_VERSION: u8 = 1;

/// Which agent chat a launched profile should open.
///
/// Deliberately a sibling of `TerminalLaunchRequestV1` rather than a field
/// inside it, so the accepted v1 launch wire stays byte-identical and an older
/// payload simply carries no binding.
///
/// `id` may be a provider session identifier or an opaque launcher client
/// handle. Codex uses the latter while the launcher owns its provider-specific
/// resume grammar. The stable Gemini profile now launches Antigravity without
/// a synthetic binding because Antigravity owns its conversation ids.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalLaunchSessionV1 {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Reopen the named chat instead of starting a new one.
    #[serde(default)]
    pub resume: bool,
}

/// Public, credential-free availability projection for one host-owned profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalLaunchProfile {
    pub id: TerminalLaunchProfileId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    pub label: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Protocol 43: how to install a missing CLI, ending "then press Refresh".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_hint: Option<String>,
    /// Protocol 43: what the CLI's `--version` printed, first line only. Only
    /// a `details` listing (Settings) asks; the launcher never waits on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Protocol 48: whether the CLI may use Interface's tools (`pass_off`,
    /// `read_pane`, `send_to_pane`) through the `iface` entry in its own user
    /// config. Only a `details` listing reads it, and only for Grok and
    /// Antigravity, the CLIs that keep that entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_tools: Option<TerminalPaneToolsState>,
    /// Protocol 51: what changed in this CLI since Interface last checked it
    /// against what Interface depends on (its version, flags, hook names and
    /// capture strings; help and version output only, never a model turn).
    /// Only a `details` listing carries it, for a change found in the last
    /// week or one that still does not match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_since_last_check: Option<TerminalToolChangeV1>,
}

/// Protocol 51: one CLI's change since the provider canary last checked it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalToolChangeV1 {
    pub version: u8,
    /// The version seen before, when the version is what changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The version now.
    pub to: String,
    /// ISO 8601, when the canary found it.
    pub checked_at: String,
    /// What no longer matches, one sentence each; empty when it all does.
    pub findings: Vec<String>,
}

pub const TERMINAL_TOOL_CHANGE_VERSION: u8 = 1;

/// Protocol 48: what a CLI's own config holds for Interface's tools.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalPaneToolsState {
    /// Interface's own entry, enabled.
    On,
    /// No entry, or Interface's entry switched off in the CLI.
    Off,
    /// An `iface` entry that is not Interface's; Interface leaves it alone.
    Taken,
    /// The CLI's list of MCP servers could not be read.
    Unknown,
}

/// Protocol 48: turn a CLI's use of Interface's tools on or off from
/// Settings. Off removes only Interface's own entry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalPaneToolsChange {
    pub profile_id: TerminalLaunchProfileId,
    pub enabled: bool,
}

/// Public, credential-free projection of one terminal process or retained
/// exited entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSummary {
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    pub title: String,
    /// Exact authorized workspace/worktree root which owns this terminal.
    pub root: String,
    /// Initial process working directory. Usually the exact root; a pass-off
    /// pane may start at a host-verified descendant of it, which is why the
    /// two remain distinct.
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<TerminalLaunchRequestV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_session: Option<TerminalLaunchSessionV1>,
    pub state: TerminalState,
    pub size: TerminalSize,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<TerminalExit>,
    /// The pane this one was passed off from, when it was created through a
    /// handoff. Kept for the entry's life so a renderer that did not create
    /// the pane (a tool did) can still seat it beside its source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed_off_from: Option<TerminalId>,
    /// Live CLI display state. Never persisted or inferred from a launch profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<TerminalFastModeV1>,
    /// Startup files this pane's program is still waiting for a File Provider
    /// (iCloud Drive) to download. Live only; absent once they are local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_download: Option<TerminalStartupDownloadV1>,
    /// Host protocol v44: the pane's program is waiting for the user (a
    /// permission dialog, a question, an approval alert or a bell). Live
    /// only; absent while it is not waiting. Rust decides this from the PTY
    /// bytes and input it already sees; the renderer and the phone read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<TerminalAttentionV1>,
    /// Host protocol v50: the agent terminal whose pane this shell took. An
    /// agent CLI that exits cleanly by itself is handed a plain shell in its
    /// folder by Rust, whatever the window shows; the renderer moves the tab
    /// from that id to this one wherever it is laid out. Kept for the shell's
    /// life, so a renderer that missed the moment can still make the move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<TerminalId>,
}

/// Host protocol v44: a pane that needs the user, and since when (the frozen
/// ISO timestamp `updatedAt` uses).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalAttentionV1 {
    pub version: u8,
    pub since: String,
}

pub const TERMINAL_ATTENTION_VERSION: u8 = 1;

/// Host protocol v31: the files a pane's CLI or git-aware prompt reads before
/// its first paint that a File Provider evicted and has not yet downloaded.
/// `files` names at most [`TERMINAL_STARTUP_DOWNLOAD_NAMED_FILES`] of them,
/// relative to the pane's cwd or `~/`; `remaining` counts every one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalStartupDownloadV1 {
    pub version: u8,
    pub files: Vec<String>,
    pub remaining: u32,
}

pub const TERMINAL_STARTUP_DOWNLOAD_VERSION: u8 = 1;
pub const TERMINAL_STARTUP_DOWNLOAD_NAMED_FILES: usize = 3;

/// Host protocol v8: the CLI's current model-specific Fast display projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalFastModeV1 {
    pub version: u8,
    pub model: String,
    pub enabled: bool,
    pub ready: bool,
}

pub const TERMINAL_WORKSPACE_RECORD_VERSION: u8 = 1;

/// One terminal as a saved workspace remembers it.
///
/// The terminal `id` is kept, not re-minted, so the renderer's pane layout —
/// which addresses tabs by terminal id — replays verbatim on reopen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalWorkspaceTerminal {
    pub id: TerminalId,
    pub title: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<TerminalLaunchRequestV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_session: Option<TerminalLaunchSessionV1>,
}

/// A named set of terminals that reopens onto the same chats.
///
/// `layout` is the renderer's own serialized pane tree, carried opaquely: the
/// arrangement is presentation, and duplicating its schema here would give two
/// languages a shared store to disagree about.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalWorkspaceRecord {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub terminals: Vec<TerminalWorkspaceTerminal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshot {
    pub revision: u64,
    pub terminals: Vec<TerminalSummary>,
}

pub const TERMINAL_HANDOFF_VERSION: u8 = 1;
pub const TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES: usize = 512 * 1024;
pub const TERMINAL_HANDOFF_TITLE_MAX_BYTES: usize = 200;

/// What the pass-off body is: the source pane's rendered terminal output, or
/// a brief an agent wrote for the next one. Absent means transcript, which is
/// what every renderer-sent pass-off is.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalHandoffKind {
    #[default]
    Transcript,
    Brief,
}

/// Transient pass-off input consumed at spawn. It never persists: excluded
/// from the create fingerprint and saved workspaces; only its source id is
/// echoed on the summary as `passed_off_from`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalHandoffV1 {
    pub version: u8,
    pub source_terminal_id: TerminalId,
    pub transcript: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TerminalHandoffKind>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTerminalRequest {
    pub id: TerminalId,
    pub cwd: String,
    #[serde(default)]
    pub size: TerminalSize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<TerminalLaunchRequestV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_session: Option<TerminalLaunchSessionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<TerminalHandoffV1>,
    /// Protocol 41: run the `launch` profile's own sign-in command (`claude
    /// auth login`, `codex login`, `grok login`) rather than a chat. Rust
    /// owns the argv; a sign-in pane takes no chat binding or pass-off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sign_in: bool,
    /// Protocol 60: the provider account a sign-in pane signs in. Absent means
    /// the profile's default account; only a sign-in names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

impl CreateTerminalRequest {
    /// # Errors
    /// A sign-in request without a profile, or with a chat binding or a
    /// pass-off, which a sign-in pane cannot carry.
    pub fn validate_sign_in(&self) -> Result<(), ValidationError> {
        if !self.sign_in {
            return if self.account.is_some() {
                Err(ValidationError::new(
                    "account",
                    "only a sign-in pane names an account",
                ))
            } else {
                Ok(())
            };
        }
        if self.launch.is_none() {
            return Err(ValidationError::new(
                "signIn",
                "a sign-in needs a launch profile",
            ));
        }
        if let Some(account) = &self.account
            && !crate::provider_accounts::valid_account_id(account)
        {
            return Err(ValidationError::new("account", "is not an account id"));
        }
        if self.launch_session.is_some() || self.handoff.is_some() || self.session_key.is_some() {
            return Err(ValidationError::new(
                "signIn",
                "a sign-in pane takes no chat binding, session or pass-off",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameTerminalRequest {
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    pub title: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResizeTerminalRequest {
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    pub size: TerminalSize,
}

/// Protocol v24: presentation visibility plus an optional exact rendered idle prompt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalWorkspaceActivityRequest {
    pub version: u8,
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<crate::terminal_controls::TerminalControlScreenV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalGenerationRequest {
    pub id: TerminalId,
    pub generation: TerminalGeneration,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DismissTerminalRequest {
    pub id: TerminalId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DismissTerminalResult {
    pub id: TerminalId,
    pub removed: bool,
}

/// Low-volume lifecycle push. Raw PTY bytes use the dedicated binary data
/// plane and never share this control/event queue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
// One event per lifecycle change; boxing the summary would only move that one
// allocation into every constructor and match site of a frozen wire shape.
#[allow(clippy::large_enum_variant)]
pub enum TerminalLifecycleEvent {
    Upsert {
        revision: u64,
        terminal: TerminalSummary,
    },
    Removed {
        revision: u64,
        id: TerminalId,
        generation: TerminalGeneration,
    },
}

#[cfg(test)]
mod terminal_shell_handoff_v50_tests {
    use super::*;

    #[test]
    fn a_handed_off_shell_names_the_agent_it_replaces_like_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-shell-handoff-v50.json"
        ))
        .expect("fixture");
        let shell: TerminalSummary =
            serde_json::from_value(fixture["shell"].clone()).expect("shell decodes");
        assert_eq!(
            shell.replaces,
            Some(
                serde_json::from_value::<TerminalId>(serde_json::json!(
                    "00000000-0000-4000-8000-000000001401"
                ))
                .expect("agent id")
            )
        );
        assert_eq!(
            serde_json::to_value(&shell).expect("encode"),
            fixture["shell"]
        );
        // Absent on every other terminal, and never `null` on the wire.
        let plain = TerminalSummary {
            replaces: None,
            ..shell
        };
        assert!(
            serde_json::to_value(plain)
                .expect("encode")
                .get("replaces")
                .is_none()
        );
    }
}

#[cfg(test)]
mod terminal_pane_tools_v48_tests {
    use super::*;

    #[test]
    fn pane_tools_states_and_changes_match_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-pane-tools-v48.json"
        ))
        .expect("fixture");
        let states: Vec<TerminalPaneToolsState> = fixture["profiles"]
            .as_array()
            .expect("profiles")
            .iter()
            .map(|profile| {
                let decoded: TerminalLaunchProfile =
                    serde_json::from_value(profile.clone()).expect("profile decodes");
                assert_eq!(&serde_json::to_value(&decoded).expect("encode"), profile);
                decoded.pane_tools.expect("a pane tools state")
            })
            .collect();
        assert_eq!(
            states,
            [
                TerminalPaneToolsState::On,
                TerminalPaneToolsState::Off,
                TerminalPaneToolsState::Taken,
                TerminalPaneToolsState::Unknown
            ]
        );
        for request in fixture["requests"].as_array().expect("requests") {
            let change: TerminalPaneToolsChange =
                serde_json::from_value(request["paneTools"].clone()).expect("change decodes");
            assert_eq!(
                serde_json::to_value(change).expect("encode"),
                request["paneTools"]
            );
        }
    }
}

#[cfg(test)]
mod terminal_tool_change_v51_tests {
    use super::*;

    #[test]
    fn terminal_tool_changes_match_the_shared_v51_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-tool-change-v51.json"
        ))
        .expect("fixture");
        assert_eq!(fixture["protocolVersion"], 51);
        for profile in fixture["profiles"].as_array().expect("profiles") {
            let decoded: TerminalLaunchProfile =
                serde_json::from_value(profile.clone()).expect("profile decodes");
            assert_eq!(&serde_json::to_value(&decoded).expect("encode"), profile);
            if let Some(change) = &decoded.changed_since_last_check {
                assert_eq!(change.version, TERMINAL_TOOL_CHANGE_VERSION);
            }
        }
        let mut unknown = fixture["profiles"][0].clone();
        unknown["changedSinceLastCheck"]["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<TerminalLaunchProfile>(unknown).is_err());
    }
}

#[cfg(test)]
mod terminal_tools_v43_tests {
    use super::*;

    #[test]
    fn terminal_tool_profiles_match_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-tools-v43.json"
        ))
        .expect("fixture");
        for profile in fixture["profiles"].as_array().expect("profiles") {
            let decoded: TerminalLaunchProfile =
                serde_json::from_value(profile.clone()).expect("profile decodes");
            assert_eq!(&serde_json::to_value(&decoded).expect("encode"), profile);
            assert_eq!(decoded.available, decoded.install_hint.is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn id() -> TerminalId {
        TerminalId::new(Uuid::parse_str("2f56fa6c-e114-4e88-bb31-6878c8cc3702").unwrap())
    }

    #[test]
    fn terminal_fast_mode_matches_the_cross_language_v8_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-fast-mode-v8.json"
        ))
        .unwrap();
        let mode: TerminalFastModeV1 = serde_json::from_value(fixture["mode"].clone()).unwrap();
        assert_eq!(mode.version, 1);
        assert_eq!(serde_json::to_value(mode).unwrap(), fixture["mode"]);
        let mut unknown = fixture["mode"].clone();
        unknown["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<TerminalFastModeV1>(unknown).is_err());
    }

    #[test]
    fn terminal_startup_download_matches_the_cross_language_v31_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-startup-download-v31.json"
        ))
        .unwrap();
        let download: TerminalStartupDownloadV1 =
            serde_json::from_value(fixture["download"].clone()).unwrap();
        assert_eq!(download.version, TERMINAL_STARTUP_DOWNLOAD_VERSION);
        assert_eq!(download.files.len(), TERMINAL_STARTUP_DOWNLOAD_NAMED_FILES);
        assert_eq!(serde_json::to_value(download).unwrap(), fixture["download"]);
        let mut unknown = fixture["download"].clone();
        unknown["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<TerminalStartupDownloadV1>(unknown).is_err());
    }

    #[test]
    fn a_chat_binding_rides_beside_the_untouched_v1_launch_wire() {
        let bound = CreateTerminalRequest {
            sign_in: false,
            id: id(),
            cwd: "/tmp/project".into(),
            size: TerminalSize::default(),
            title: None,
            session_key: None,
            launch: Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            launch_session: Some(TerminalLaunchSessionV1 {
                version: TERMINAL_LAUNCH_SESSION_VERSION,
                id: Some("8b764f80-19ee-47c7-a9da-08088a6db888".into()),
                resume: true,
            }),
            handoff: None,
            account: None,
        };
        let wire = serde_json::to_value(&bound).unwrap();
        // The v1 launch object is unchanged by the addition; the binding is a
        // sibling key, so an older reader sees exactly what it saw before.
        assert_eq!(
            wire["launch"],
            json!({ "version": 1, "profileId": "claude" })
        );
        assert_eq!(
            wire["launchSession"],
            json!({
                "version": 1,
                "id": "8b764f80-19ee-47c7-a9da-08088a6db888",
                "resume": true
            })
        );
        assert_eq!(
            serde_json::from_value::<CreateTerminalRequest>(wire).unwrap(),
            bound
        );
    }

    #[test]
    fn create_terminal_request_round_trips_handoff() {
        let json = serde_json::json!({
            "id": "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e11",
            "cwd": "/tmp/project",
            "handoff": {
                "version": 1,
                "sourceTerminalId": "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12",
                "transcript": "hello",
                "fromTitle": "bridgemind"
            }
        });
        let request: CreateTerminalRequest = serde_json::from_value(json).expect("decode");
        let handoff = request.handoff.expect("handoff");
        assert_eq!(handoff.version, TERMINAL_HANDOFF_VERSION);
        assert_eq!(handoff.transcript, "hello");
        assert_eq!(handoff.from_title.as_deref(), Some("bridgemind"));
        // The renderer never sends a kind; absent reads as a transcript and
        // serializes back without the key so the accepted wire stays put.
        assert_eq!(handoff.kind, None);
        assert_eq!(
            handoff.kind.unwrap_or_default(),
            TerminalHandoffKind::Transcript
        );
        assert!(
            serde_json::to_value(&handoff)
                .unwrap()
                .get("kind")
                .is_none()
        );
        let brief: TerminalHandoffV1 = serde_json::from_value(serde_json::json!({
            "version": 1,
            "sourceTerminalId": "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12",
            "transcript": "do X",
            "kind": "brief"
        }))
        .expect("brief handoff");
        assert_eq!(brief.kind, Some(TerminalHandoffKind::Brief));
        assert_eq!(serde_json::to_value(&brief).unwrap()["kind"], "brief");
        let unknown_kind = serde_json::json!({"version": 1, "sourceTerminalId":
            "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12", "transcript": "x", "kind": "memo"});
        assert!(serde_json::from_value::<TerminalHandoffV1>(unknown_kind).is_err());
        // Unknown fields inside the handoff are refused like everywhere else.
        let bad = serde_json::json!({"version": 1, "sourceTerminalId":
            "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12", "transcript": "x", "extra": true});
        assert!(serde_json::from_value::<TerminalHandoffV1>(bad).is_err());
    }

    #[test]
    fn summary_carries_its_pass_off_source_only_when_it_has_one() {
        let plain = serde_json::json!({
            "id": "2f56fa6c-e114-4e88-bb31-6878c8cc3702",
            "generation": 1,
            "title": "Codex",
            "root": "/tmp/project",
            "cwd": "/tmp/project",
            "state": "running",
            "size": { "columns": 120, "rows": 40 },
            "createdAt": "2026-09-05T00:00:00.000Z",
            "updatedAt": "2026-09-05T00:00:00.000Z"
        });
        let summary: TerminalSummary = serde_json::from_value(plain.clone()).expect("summary");
        assert_eq!(summary.passed_off_from, None);
        assert_eq!(serde_json::to_value(&summary).unwrap(), plain);
        let mut passed = plain.clone();
        passed["passedOffFrom"] = serde_json::json!("0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12");
        let summary: TerminalSummary = serde_json::from_value(passed.clone()).expect("summary");
        assert_eq!(
            summary.passed_off_from.map(|id| id.to_string()).as_deref(),
            Some("0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12")
        );
        assert_eq!(serde_json::to_value(&summary).unwrap(), passed);
    }

    #[test]
    fn request_and_summary_wire_shapes_are_stable() {
        let request = CreateTerminalRequest {
            sign_in: false,
            id: id(),
            cwd: "/tmp/project".into(),
            size: TerminalSize {
                columns: 120,
                rows: 40,
            },
            title: Some("Tests".into()),
            session_key: Some("codex#thread".into()),
            launch: Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id: TerminalLaunchProfileId::Codex,
            }),
            launch_session: None,
            handoff: None,
            account: None,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "id": "2f56fa6c-e114-4e88-bb31-6878c8cc3702",
                "cwd": "/tmp/project",
                "size": { "columns": 120, "rows": 40 },
                "title": "Tests",
                "sessionKey": "codex#thread",
                "launch": { "version": 1, "profileId": "codex" }
            })
        );

        let profile = TerminalLaunchProfile {
            id: TerminalLaunchProfileId::Gemini,
            engine: Some("acp:gemini".into()),
            label: "Antigravity".into(),
            available: false,
            unavailable_reason: Some("Antigravity CLI was not found.".into()),
            install_hint: None,
            version: None,
            pane_tools: None,
            changed_since_last_check: None,
        };
        assert_eq!(
            serde_json::to_value(profile).unwrap(),
            json!({
                "id": "gemini",
                "engine": "acp:gemini",
                "label": "Antigravity",
                "available": false,
                "unavailableReason": "Antigravity CLI was not found."
            })
        );

        let event = TerminalLifecycleEvent::Removed {
            revision: 7,
            id: id(),
            generation: TerminalGeneration(3),
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "kind": "removed",
                "revision": 7,
                "id": "2f56fa6c-e114-4e88-bb31-6878c8cc3702",
                "generation": 3
            })
        );
    }

    #[test]
    fn omp_wire_round_trips_the_shared_protocol_v7_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-omp-v7.json"
        ))
        .unwrap();
        let profile: TerminalLaunchProfile =
            serde_json::from_value(fixture["profile"].clone()).unwrap();
        assert_eq!(profile.id, TerminalLaunchProfileId::Omp);
        assert_eq!(profile.id.as_str(), "omp");
        assert_eq!(serde_json::to_value(profile).unwrap(), fixture["profile"]);
        let request: CreateTerminalRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(
            request.launch.unwrap().profile_id,
            TerminalLaunchProfileId::Omp
        );
        assert!(request.launch_session.is_none());
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
    }

    #[test]
    fn opencode_wire_round_trips_the_shared_protocol_v9_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-opencode-v9.json"
        ))
        .unwrap();
        let profile: TerminalLaunchProfile =
            serde_json::from_value(fixture["profile"].clone()).unwrap();
        assert_eq!(profile.id, TerminalLaunchProfileId::Opencode);
        assert_eq!(profile.id.as_str(), "opencode");
        assert_eq!(serde_json::to_value(profile).unwrap(), fixture["profile"]);
        let request: CreateTerminalRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(
            request.launch.unwrap().profile_id,
            TerminalLaunchProfileId::Opencode
        );
        assert!(request.launch_session.is_none());
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
    }

    #[test]
    fn muse_wire_round_trips_the_shared_protocol_v35_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-muse-v35.json"
        ))
        .unwrap();
        let profile: TerminalLaunchProfile =
            serde_json::from_value(fixture["profile"].clone()).unwrap();
        assert_eq!(profile.id, TerminalLaunchProfileId::Muse);
        assert_eq!(profile.id.as_str(), "muse");
        assert_eq!(serde_json::to_value(profile).unwrap(), fixture["profile"]);
        let request: CreateTerminalRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(
            request.launch.unwrap().profile_id,
            TerminalLaunchProfileId::Muse
        );
        assert!(request.launch_session.is_none());
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
    }

    #[test]
    fn kimi_wire_round_trips_the_shared_protocol_v54_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-kimi-v54.json"
        ))
        .unwrap();
        let profile: TerminalLaunchProfile =
            serde_json::from_value(fixture["profile"].clone()).unwrap();
        assert_eq!(profile.id, TerminalLaunchProfileId::Kimi);
        assert_eq!(profile.id.as_str(), "kimi");
        assert_eq!(serde_json::to_value(profile).unwrap(), fixture["profile"]);
        let request: CreateTerminalRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(
            request.launch.unwrap().profile_id,
            TerminalLaunchProfileId::Kimi
        );
        assert!(request.launch_session.is_none());
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
    }

    #[test]
    fn generation_is_monotonic_and_saturating() {
        assert_eq!(TerminalGeneration::initial().next(), TerminalGeneration(2));
        assert_eq!(
            TerminalGeneration(u64::MAX).next(),
            TerminalGeneration(u64::MAX)
        );
    }

    #[test]
    fn strict_decoder_rejects_unknown_fields() {
        let error = serde_json::from_value::<ResizeTerminalRequest>(json!({
            "id": "2f56fa6c-e114-4e88-bb31-6878c8cc3702",
            "generation": 1,
            "size": { "columns": 80, "rows": 24 },
            "extra": true
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}

#[cfg(test)]
mod workspace_activity_contract_tests {
    use super::*;
    #[test]
    fn workspace_activity_matches_the_shared_v24_fixture_and_rejects_extra_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-workspace-activity-v24.json"
        ))
        .unwrap();
        let request: TerminalWorkspaceActivityRequest =
            serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), fixture);
        let mut malformed = fixture;
        malformed["force"] = serde_json::json!(true);
        assert!(serde_json::from_value::<TerminalWorkspaceActivityRequest>(malformed).is_err());
    }
}
