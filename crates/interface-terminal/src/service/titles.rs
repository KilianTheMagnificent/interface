//! Naming policy is independent of provider effects and terminal activity.
use super::{
    CreateTerminalRequest, Duration, Instant, Path, Shared, TerminalBackendError, TerminalError,
    TerminalGeneration, TerminalId, TerminalLaunchProfileId, TerminalLaunchSessionV1,
    TerminalLifecycleEvent, TerminalService, TerminalState, Weak, default_title,
    is_provider_session_id, lock, normalize_title, now,
};
use interface_contracts::terminal::TerminalWorkspaceTerminal;

/// Owner-provided durability for explicit Interface names. Never provider data.
pub trait TerminalTitlePersistence: Send + Sync {
    /// # Errors
    /// Refuses an unavailable or invalid name store.
    fn user_title(&self, id: TerminalId) -> Result<Option<String>, TerminalBackendError>;
    /// # Errors
    /// Reports failure or uncertain durability instead of acknowledging a pin.
    fn pin_title(&self, id: TerminalId, title: &str) -> Result<(), TerminalBackendError>;
}

pub(super) struct EphemeralTitles;
impl TerminalTitlePersistence for EphemeralTitles {
    fn user_title(&self, _: TerminalId) -> Result<Option<String>, TerminalBackendError> {
        Ok(None)
    }
    fn pin_title(&self, _: TerminalId, _: &str) -> Result<(), TerminalBackendError> {
        Ok(())
    }
}

fn stored_user_title(shared: &Shared, id: TerminalId) -> Result<Option<String>, TerminalError> {
    shared
        .title_persistence
        .user_title(id)
        .map_err(|error| TerminalError::Backend(error.to_string()))?
        .map(|title| normalize_title(&title))
        .transpose()
}

fn codex_name(
    shared: &Shared,
    binding: Option<&TerminalLaunchSessionV1>,
) -> Option<(String, String)> {
    let id = binding.filter(|b| b.resume)?.id.as_deref()?;
    let launches = shared.launches();
    let root = launches.codex_sessions_root()?;
    let title = lock(&shared.codex_names).lookup(root, id)?;
    (!title.is_empty()).then(|| (id.to_owned(), title))
}

/// Grok's saved `resume` flag stays false, so the minted id is the lookup key.
fn grok_name(
    shared: &Shared,
    binding: Option<&TerminalLaunchSessionV1>,
    cwd: &Path,
) -> Option<(String, String)> {
    let id = binding.and_then(|binding| binding.id.as_deref())?;
    let launches = shared.launches();
    let root = launches.grok_sessions_root()?;
    let title = lock(&shared.grok_names).lookup(root, cwd, id)?;
    (!title.is_empty()).then(|| (id.to_owned(), title))
}

fn antigravity_name(
    shared: &Shared,
    binding: Option<&TerminalLaunchSessionV1>,
) -> Option<(String, String)> {
    let id = binding.filter(|b| b.resume)?.id.as_deref()?;
    let launches = shared.launches();
    let root = launches.antigravity_root()?;
    let title = lock(&shared.antigravity_names).lookup(root, id)?;
    (!title.is_empty()).then(|| (id.to_owned(), title))
}

impl TerminalService {
    /// Repair display metadata using only an already-bound provider ID. This
    /// never establishes, changes, or resumes a provider session.
    /// # Errors
    /// Refuses unavailable explicit-name storage instead of erasing a pin.
    pub fn saved_terminal_title(
        &self,
        terminal: &TerminalWorkspaceTerminal,
    ) -> Result<String, TerminalError> {
        if let Some(title) = stored_user_title(&self.shared, terminal.id)? {
            return Ok(title);
        }
        if terminal
            .launch
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Grok)
        {
            // Interface renames live in the pin store above. A roster label is
            // the numbered default or the last automatic name, so the session
            // file wins when Grok has one.
            if let Some((_, title)) = grok_name(
                &self.shared,
                terminal.launch_session.as_ref(),
                Path::new(&terminal.cwd),
            ) {
                return Ok(title);
            }
            return Ok(terminal.title.clone());
        }
        if terminal
            .launch
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Gemini)
        {
            if let Some((_, title)) =
                antigravity_name(&self.shared, terminal.launch_session.as_ref())
            {
                return Ok(title);
            }
            return Ok(terminal.title.clone());
        }
        if terminal
            .launch
            .is_none_or(|l| l.profile_id != TerminalLaunchProfileId::Codex)
        {
            return Ok(terminal.title.clone());
        }
        if let Some((_, title)) = codex_name(&self.shared, terminal.launch_session.as_ref()) {
            let id = terminal
                .launch_session
                .as_ref()
                .and_then(|b| b.id.as_deref())
                .unwrap_or("");
            let numbered = terminal
                .title
                .to_ascii_lowercase()
                .strip_prefix("codex ")
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()));
            if !numbered
                && !unusable_codex_title(&terminal.title, Path::new(&terminal.cwd))
                && !lock(&self.shared.codex_names).is_automatic_title(id, &terminal.title)
                && terminal.title != title
            {
                // Legacy rosters did not record rename provenance. Preserve a
                // useful name that cannot be identified as a provider preview.
                self.shared
                    .title_persistence
                    .pin_title(terminal.id, &terminal.title)
                    .map_err(|error| TerminalError::Backend(error.to_string()))?;
                return Ok(terminal.title.clone());
            }
            return Ok(title);
        }
        let title = crate::fast_mode::codex_title(&terminal.title)
            .map_or(terminal.title.as_str(), |(_, title)| title);
        if unusable_codex_title(title, Path::new(&terminal.cwd)) {
            return Ok("Codex".into());
        }
        Ok(crate::task_title::codex_request_title(title))
    }

    pub(super) fn initial_title(
        &self,
        request: &CreateTerminalRequest,
        title: String,
    ) -> Result<(String, bool, Option<String>), TerminalError> {
        if let Some(title) = stored_user_title(&self.shared, request.id)? {
            return Ok((title, true, None));
        }
        if request
            .launch
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Grok)
        {
            if let Some((id, title)) = grok_name(
                &self.shared,
                request.launch_session.as_ref(),
                Path::new(&request.cwd),
            ) {
                return Ok((title, false, Some(id)));
            }
            return Ok((title, false, None));
        }
        if request
            .launch
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Gemini)
        {
            if let Some((id, title)) =
                antigravity_name(&self.shared, request.launch_session.as_ref())
            {
                return Ok((title, false, Some(id)));
            }
            return Ok((title, false, None));
        }
        if request
            .launch
            .is_some_and(|l| l.profile_id == TerminalLaunchProfileId::Codex)
        {
            if let Some((id, title)) = codex_name(&self.shared, request.launch_session.as_ref()) {
                return Ok((title, false, Some(id)));
            }
            let display = crate::fast_mode::codex_title(&title).map_or(title.as_str(), |(_, t)| t);
            let title = if unusable_codex_title(display, Path::new(&request.cwd)) {
                "Codex".to_owned()
            } else {
                crate::task_title::codex_request_title(display)
            };
            return Ok((title, false, None));
        }
        Ok((title, false, None))
    }
}

pub(super) fn unusable_codex_title(title: &str, cwd: &Path) -> bool {
    is_uninformative_title(title, cwd)
        || title.starts_with("codex | ")
        || title.starts_with("[ ! ]")
        || title.starts_with("[ . ]")
        || title.starts_with("Action Required")
        || matches!(
            title,
            "Ready" | "Starting" | "Thinking" | "Working" | "Waiting" | "Fast on" | "Fast off"
        )
}

pub(super) fn refresh_codex_title(
    weak: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) {
    let Some(shared) = weak.upgrade() else { return };
    let (profile_id, binding) = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return;
        };
        let Some(profile_id) = entry.summary.launch.map(|l| l.profile_id).filter(|p| {
            matches!(
                p,
                TerminalLaunchProfileId::Codex | TerminalLaunchProfileId::Gemini
            )
        }) else {
            return;
        };
        if entry.summary.generation != generation
            || entry.summary.state != TerminalState::Running
            || entry.title_is_user
            || entry.closing
            || entry
                .summary
                .launch_session
                .as_ref()
                .is_none_or(|b| !b.resume || b.id.is_none())
            || entry
                .title_lookup_at
                .is_some_and(|last| last.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        entry.title_lookup_at = Some(Instant::now());
        (profile_id, entry.summary.launch_session.clone())
    };
    // Filesystem work never holds the registry/event locks or the PTY reader.
    let resolved = match profile_id {
        TerminalLaunchProfileId::Codex => codex_name(&shared, binding.as_ref()),
        TerminalLaunchProfileId::Gemini => antigravity_name(&shared, binding.as_ref()),
        _ => None,
    };
    let Some((session, title)) = resolved else {
        return;
    };
    let event_gate = lock(&shared.event_gate);
    let event = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return;
        };
        if entry.summary.generation != generation
            || entry.summary.state != TerminalState::Running
            || entry.title_is_user
            || entry.closing
            || entry.summary.launch_session != binding
        {
            return;
        }
        entry.file_title_session = Some(session);
        if entry.summary.title == title {
            return;
        }
        entry.summary.title = title;
        entry.summary.updated_at = now();
        let terminal = entry.summary.clone();
        state.revision = state.revision.saturating_add(1);
        TerminalLifecycleEvent::Upsert {
            revision: state.revision,
            terminal,
        }
    };
    shared.events.publish(event);
    drop(event_gate);
}

/// One tick of the Grok name poll. `false` means the pane is gone and the
/// worker should stop. Filesystem work stays off the registry locks.
pub(super) fn poll_grok_title(
    weak: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> bool {
    let Some(shared) = weak.upgrade() else {
        return false;
    };
    if super::workspace_sleep::sleeping(weak, id, generation) {
        return true;
    }
    let (binding, cwd) = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return false;
        };
        if entry.summary.generation != generation
            || entry.summary.state != TerminalState::Running
            || entry.closing
            || entry.title_is_user
            || entry
                .summary
                .launch
                .is_none_or(|launch| launch.profile_id != TerminalLaunchProfileId::Grok)
        {
            return false;
        }
        if entry
            .title_lookup_at
            .is_some_and(|last| last.elapsed() < Duration::from_secs(2))
        {
            return true;
        }
        entry.title_lookup_at = Some(Instant::now());
        (entry.summary.launch_session.clone(), entry.cwd.clone())
    };
    let Some((session, title)) = grok_name(&shared, binding.as_ref(), &cwd) else {
        return true;
    };
    let event_gate = lock(&shared.event_gate);
    let event = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return false;
        };
        if entry.summary.generation != generation
            || entry.summary.state != TerminalState::Running
            || entry.closing
            || entry.title_is_user
        {
            return false;
        }
        if entry.summary.launch_session != binding {
            return true;
        }
        entry.file_title_session = Some(session);
        if entry.summary.title == title {
            return true;
        }
        entry.summary.title = title;
        entry.summary.updated_at = now();
        let terminal = entry.summary.clone();
        state.revision = state.revision.saturating_add(1);
        TerminalLifecycleEvent::Upsert {
            revision: state.revision,
            terminal,
        }
    };
    shared.events.publish(event);
    drop(event_gate);
    true
}

/// Titles a shell announces that say nothing a tab label needs.
///
/// Every agent CLI paints its own name at startup, so three Claude terminals
/// would all collapse onto "Claude Code" and lose the launcher's numbering.
/// These are held back until the CLI has something real to say; measured
/// against the live CLIs, Claude opens with "✳ Claude Code", Grok with "grok",
/// and Antigravity may identify itself as either "Antigravity" or "agy".
const SELF_NAMED_TITLES: [&str; 16] = [
    "claude",
    "claude code",
    "codex",
    "grok",
    "qwen",
    "gemini",
    "antigravity",
    "agy",
    "shell",
    "terminal",
    "bash",
    "zsh",
    "fish",
    "sh",
    "node",
    "login",
];

/// True when an announced title carries no more information than the tab
/// already shows.
fn is_uninformative_title(title: &str, cwd: &Path) -> bool {
    let folded = title.trim().to_lowercase();
    if folded.is_empty() {
        return true;
    }
    if SELF_NAMED_TITLES.contains(&folded.as_str()) {
        return true;
    }
    // Codex's `thread` title is just its UUID until the first prompt supplies
    // a provisional name. Keep the numbered pane label during that interval.
    // This is display filtering only, never provider-session identity capture.
    if is_provider_session_id(&folded) {
        return true;
    }
    // Codex reports the project folder, which is what `default_title` already
    // produced and is identical for every terminal opened in that folder.
    default_title(cwd).to_lowercase() == folded
}

/// Grok's terminal title is activity plus the app name until a session name
/// exists. Drop that brand and reject a status-only remainder so the tab keeps
/// its numbered label. A real session name in the same title is kept.
fn grok_osc_task(title: &str) -> String {
    let stripped = strip_grok_brand(title);
    if stripped.is_empty() || grok_activity_title(&stripped) {
        String::new()
    } else {
        stripped
    }
}

fn strip_grok_brand(title: &str) -> String {
    let trimmed = title.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == "grok" {
        return String::new();
    }
    let Some(stripped) = lower.strip_suffix(" grok") else {
        return trimmed.to_owned();
    };
    trimmed[..stripped.trim_end().len()].trim().to_owned()
}

fn grok_activity_title(title: &str) -> bool {
    let mut words = Vec::new();
    for word in title.split(|character: char| {
        character.is_whitespace() || matches!(character, '|' | '·' | '•' | '-' | '—' | '–')
    }) {
        let word = word
            .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != ':');
        if word.is_empty() || word.eq_ignore_ascii_case("grok") {
            continue;
        }
        words.push(word.to_ascii_lowercase());
    }
    if words.is_empty() {
        return true;
    }
    let body = words.join(" ");
    matches!(
        body.as_str(),
        "action required"
            | "responding"
            | "running tool"
            | "compacting"
            | "working"
            | "thinking"
            | "waiting"
            | "ready"
    ) || body.starts_with("running:")
}

/// Adopt a title the running program announced over OSC.
///
/// Silently declines when the entry is gone, the generation has moved on, a
/// human has named this terminal, or the title says nothing useful. Publishes
/// the same `Upsert` a manual rename does, so the renderer needs no new
/// channel and an unopened tab still updates.
pub(super) fn apply_auto_title(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    title: &str,
) {
    let Ok(title) = normalize_title(title) else {
        return;
    };
    let Some(shared) = shared.upgrade() else {
        return;
    };
    let event_gate = lock(&shared.event_gate);
    let event = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return;
        };
        if entry.summary.generation != generation || entry.summary.state != TerminalState::Running {
            return;
        }
        let codex = entry
            .summary
            .launch
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Codex);
        let grok = entry
            .summary
            .launch
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Grok);
        let parsed = entry
            .summary
            .launch
            .filter(|launch| launch.profile_id == TerminalLaunchProfileId::Codex)
            .and_then(|_| crate::fast_mode::codex_title(&title));
        // Unknown Codex metadata is not a fallback task name. Retain the last
        // useful name while its separate Fast control becomes unavailable.
        let (fast_mode, display_title) = parsed.unwrap_or((None, if codex { "" } else { &title }));
        let grok_label;
        let display_title = if grok {
            grok_label = grok_osc_task(display_title);
            grok_label.as_str()
        } else {
            display_title
        };
        let task_title = crate::task_title::task_title(display_title);
        let display_title = task_title.as_str();
        let fast_changed = entry.summary.fast_mode != fast_mode;
        entry.summary.fast_mode = fast_mode;
        let session_named = entry.file_title_session.as_deref().is_some_and(|id| {
            entry
                .summary
                .launch_session
                .as_ref()
                .and_then(|binding| binding.id.as_deref())
                == Some(id)
        });
        let can_auto_name = !(entry.title_is_user || session_named);
        let title_changed = can_auto_name
            && !display_title.is_empty()
            && entry.summary.title != display_title
            && !is_uninformative_title(display_title, &entry.cwd);
        if !fast_changed && !title_changed {
            return;
        }
        if title_changed {
            display_title.clone_into(&mut entry.summary.title);
        }
        entry.summary.updated_at = now();
        let summary = entry.summary.clone();
        state.revision = state.revision.saturating_add(1);
        TerminalLifecycleEvent::Upsert {
            revision: state.revision,
            terminal: summary,
        }
    };
    shared.events.publish(event);
    drop(event_gate);
}
