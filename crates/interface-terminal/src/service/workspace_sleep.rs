//! Workspace resource policy. Visibility is a window fact; Rust decides when
//! work has finished. Unknown activity never times out into agent idleness.
use super::{
    Arc, Duration, Instant, Mutex, Shared, TerminalError, TerminalGeneration,
    TerminalGenerationRequest, TerminalId, TerminalLaunchProfileId, TerminalLifecycleEvent,
    TerminalProcessControl, TerminalService, TerminalState, TerminalSummary, Weak, current_entry,
    lock, now,
};
use crate::attention::AttentionMonitor;
use interface_contracts::terminal::{
    TERMINAL_ATTENTION_VERSION, TerminalAttentionV1, TerminalWorkspaceActivityRequest,
};

const IDLE_GRACE: Duration = Duration::from_secs(5);
/// The longest wait before retrying a suspension the process tree declined.
const DECLINED_RETRY_MAX: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessPower {
    Awake,
    /// A suspension probe is listing and signalling the tree. Reported as
    /// sleeping, so input waits for the probe and then wakes the pane.
    Suspending,
    Suspended,
    WakePending,
}

pub(super) struct WorkspacePower {
    /// Held across every suspend and resume, which list processes and signal
    /// groups for tens of milliseconds. The power lock itself is only held
    /// for short reads and writes, because the PTY reader takes it for every
    /// output chunk. Taken before the power lock, never while holding it.
    transition: Arc<Mutex<()>>,
    active: bool,
    process: ProcessPower,
    profile: TerminalLaunchProfileId,
    idle_since: Option<Instant>,
    busy: bool,
    /// Native AGY status is emitted after its hooks finish and owns readiness.
    agy_status: bool,
    output_cursor: u64,
    shell_mode: Vec<u8>,
    /// A pass-off pane's bracketed-paste mode, tracked from its first byte: a
    /// typed pass-off waits for the CLI's input editor. `None` elsewhere.
    paste_mode: Option<bool>,
    titles: crate::osc_title::OscTitleScanner,
    /// After the process tree declines a suspension (a separately grouped
    /// job, a child still running), the next attempt waits until here. The
    /// wait doubles up to a minute and clears on new activity evidence (a turn
    /// or prompt that newly ends), input or a visibility change. Plain output
    /// does not clear it: a declining tree that keeps printing would otherwise
    /// be re-probed every tick.
    retry_after: Option<Instant>,
    retry_delay: Duration,
    /// Whether the program waits for the user (host protocol v44). Fed the
    /// same visible bytes and every input write; the reader and `write`
    /// publish its changes as `TerminalSummary.attention`.
    attention: AttentionMonitor,
}

impl WorkspacePower {
    pub(super) fn new(summary: &TerminalSummary) -> Self {
        let profile = summary
            .launch
            .map_or(TerminalLaunchProfileId::Shell, |launch| launch.profile_id);
        Self {
            transition: Arc::default(),
            active: true,
            process: ProcessPower::Awake,
            profile,
            idle_since: None,
            busy: false,
            agy_status: false,
            output_cursor: 0,
            shell_mode: Vec::new(),
            paste_mode: summary.passed_off_from.is_some().then_some(false),
            titles: crate::osc_title::OscTitleScanner::activity(),
            retry_after: None,
            retry_delay: Duration::ZERO,
            attention: AttentionMonitor::new(profile),
        }
    }

    fn profile(&mut self, profile: TerminalLaunchProfileId) {
        if self.profile != profile {
            self.attention.set_profile(profile);
            self.profile = profile;
            self.busy = false;
            self.agy_status = false;
            self.idle_since = None;
            self.titles = crate::osc_title::OscTitleScanner::activity();
        }
    }

    fn idle(&mut self) {
        if self.busy || self.idle_since.is_none() {
            // A turn or command that newly ended may have changed the tree.
            self.clear_retry();
        }
        self.busy = false;
        self.idle_since.get_or_insert_with(Instant::now);
    }

    fn working(&mut self) {
        self.busy = true;
        self.idle_since = None;
    }

    /// The tree declined a suspension: retry after `IDLE_GRACE`, then twice as
    /// long each time, up to `DECLINED_RETRY_MAX`.
    fn declined(&mut self) {
        self.retry_delay = if self.retry_after.is_some() {
            (self.retry_delay * 2).min(DECLINED_RETRY_MAX)
        } else {
            IDLE_GRACE
        };
        self.retry_after = Some(Instant::now() + self.retry_delay);
    }

    fn clear_retry(&mut self) {
        self.retry_after = None;
        self.retry_delay = Duration::ZERO;
    }

    /// Feeds one chunk of visible output. Answers whether the pane's
    /// "needs you" changed, for the reader to publish.
    pub(super) fn output(&mut self, bytes: &[u8]) -> bool {
        let attention = self.attention.feed(bytes);
        self.output_cursor = self.output_cursor.saturating_add(bytes.len() as u64);
        if self.profile == TerminalLaunchProfileId::Shell || self.paste_mode.is_some() {
            // Interactive zsh/readline enable bracketed paste at their prompt
            // and disable it before a command. Quiet/builtin work is NOT idle.
            let mut rest = bytes;
            while let Some((&byte, after)) = rest.split_first() {
                if self.shell_mode.is_empty() && byte != 0x1b {
                    // Only ESC can start a mode sequence: skip straight to it.
                    let Some(escape) = rest.iter().position(|byte| *byte == 0x1b) else {
                        break;
                    };
                    rest = &rest[escape..];
                    continue;
                }
                rest = after;
                if byte == 0x1b {
                    self.shell_mode.clear();
                }
                if byte == 0x1b || !self.shell_mode.is_empty() {
                    self.shell_mode.push(byte);
                    if self.shell_mode == b"\x1b[?2004h" {
                        self.paste_mode(true);
                        self.shell_mode.clear();
                    } else if self.shell_mode == b"\x1b[?2004l" {
                        self.paste_mode(false);
                        self.shell_mode.clear();
                    } else if !b"\x1b[?2004h".starts_with(&self.shell_mode)
                        && !b"\x1b[?2004l".starts_with(&self.shell_mode)
                    {
                        self.shell_mode.clear();
                    }
                }
            }
        }
        // Only these profiles act on OSC frames; the others skip the scan.
        if !matches!(
            self.profile,
            TerminalLaunchProfileId::Claude
                | TerminalLaunchProfileId::Codex
                | TerminalLaunchProfileId::Opencode
                | TerminalLaunchProfileId::Gemini
                | TerminalLaunchProfileId::Muse
                | TerminalLaunchProfileId::Kimi
        ) {
            return attention;
        }
        for frame in self.titles.feed(bytes) {
            if let Some((channel, title)) = frame.split_once(';') {
                self.osc_frame(channel, title);
            }
        }
        attention
    }

    fn paste_mode(&mut self, enabled: bool) {
        if let Some(mode) = self.paste_mode.as_mut() {
            *mode = enabled;
        }
        if self.profile == TerminalLaunchProfileId::Shell {
            if enabled {
                self.idle();
            } else {
                self.working();
            }
        }
    }

    /// Whether the CLI's editor takes a paste now, and how much output it
    /// has written: a typed pass-off waits until that count stops moving.
    pub(super) fn paste_ready(&self) -> (bool, u64) {
        (self.paste_mode == Some(true), self.output_cursor)
    }

    fn osc_frame(&mut self, channel: &str, title: &str) {
        if self.agy_status && channel != "7501" {
            return;
        }
        if channel == "9" {
            if self.profile != TerminalLaunchProfileId::Codex {
                return;
            }
            let message = title.trim().to_ascii_lowercase();
            if message.starts_with("approval requested")
                || message.starts_with("codex wants to edit")
                || message.starts_with("plan mode prompt:")
            {
                self.working();
            } else if !message.is_empty() {
                self.idle();
            }
            return;
        }
        if channel == "7501" {
            if self.profile == TerminalLaunchProfileId::Gemini {
                let status = crate::attention::osc7501_state(title);
                if status.is_some() {
                    self.agy_status = true;
                }
                match status {
                    Some("working" | "blocked") => {
                        if !self.busy {
                            self.working();
                        }
                    }
                    Some("done" | "error" | "idle" | "clear") => self.idle(),
                    _ => {}
                }
            }
            return;
        }
        if crate::native_checkpoint::supports(self.profile) {
            match title.strip_prefix(crate::native_checkpoint::ACTIVITY_PREFIX) {
                Some("working" | "waiting") => self.working(),
                Some("complete" | "idle") => self.idle(),
                _ => {}
            }
        } else if self.profile == TerminalLaunchProfileId::Claude {
            match title.strip_prefix(crate::launch::CLAUDE_ACTIVITY_TITLE_PREFIX) {
                // A pane waiting on a permission answer is not done.
                Some("working" | "waiting") => self.working(),
                Some("idle") => self.idle(),
                _ => {} // Claude's permanent star/name never means completion.
            }
        } else if self.profile == TerminalLaunchProfileId::Opencode {
            match title.strip_prefix(crate::opencode_activity::TERMINAL_ACTIVITY_PREFIX) {
                Some("working" | "waiting") => self.working(),
                Some("complete" | "idle") => self.idle(),
                _ => {}
            }
        } else if self.profile == TerminalLaunchProfileId::Codex {
            let first = title.trim().chars().next();
            if first.is_some_and(|c| {
                matches!(c, '\u{2800}'..='\u{28ff}') || "●○◐◓◑◒◆▪★*⏳⚡".contains(c)
            }) {
                self.working();
            }
            // Renames and prompt titles are not completion receipts.
        }
    }

    pub(super) fn suspended(&self) -> bool {
        self.process != ProcessPower::Awake
    }

    pub(super) const fn waiting(&self) -> bool {
        self.attention.waiting()
    }

    fn ready(&self) -> bool {
        !self.active
            && !self.busy
            && self.process == ProcessPower::Awake
            && self.idle_since.is_some_and(|at| at.elapsed() >= IDLE_GRACE)
            && self.retry_after.is_none_or(|at| Instant::now() >= at)
    }
}

/// The gate a suspend or resume holds; see [`WorkspacePower::transition`].
fn transition_gate(power: &Mutex<WorkspacePower>) -> Arc<Mutex<()>> {
    Arc::clone(&lock(power).transition)
}

pub(super) fn sleeping(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> bool {
    let Some(shared) = shared.upgrade() else {
        return false;
    };
    let power = lock(&shared.state)
        .entries
        .get(&id)
        .filter(|entry| entry.summary.generation == generation && !entry.closing)
        .map(|entry| Arc::clone(&entry.power));
    power.is_some_and(|power| lock(&power).suspended())
}

impl TerminalService {
    /// Update workspace visibility without ending or recreating a terminal.
    /// # Errors
    /// Rejects malformed evidence, stale generations and failed process wakes.
    pub fn set_workspace_activity(
        &self,
        request: &TerminalWorkspaceActivityRequest,
    ) -> Result<TerminalSummary, TerminalError> {
        if request.version != 1
            || request
                .screen
                .as_ref()
                .is_some_and(|screen| !crate::screen::valid_screen(screen))
            || request
                .output_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.parse::<u64>().is_err())
        {
            return Err(TerminalError::InvalidConfiguration);
        }
        let (power, control, summary) = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            (
                Arc::clone(&entry.power),
                Arc::clone(&entry.resources.control),
                entry.summary.clone(),
            )
        };
        let gate = transition_gate(&power);
        let _transition = lock(&gate);
        let mut state = lock(&power);
        state.profile(
            summary
                .launch
                .map_or(TerminalLaunchProfileId::Shell, |launch| launch.profile_id),
        );
        state.active = request.active;
        state.clear_retry();
        if request.active {
            state.process = ProcessPower::WakePending;
            drop(state);
            control
                .resume()
                .map_err(|error| TerminalError::Backend(error.to_string()))?;
            lock(&power).process = ProcessPower::Awake;
        } else if let Some(screen) = request.screen.as_ref() {
            let power = &mut state;
            // A screen from an unmounted/reloaded emulator cannot prove an idle
            // prompt after newer PTY bytes. Agent work otherwise stays awake.
            if request
                .output_cursor
                .as_ref()
                .and_then(|cursor| cursor.parse::<u64>().ok())
                == Some(power.output_cursor)
                && matches!(
                    power.profile,
                    TerminalLaunchProfileId::Claude | TerminalLaunchProfileId::Codex
                )
                && !power.busy
                && crate::screen::idle(screen, power.profile)
                && !screen
                    .lines
                    .iter()
                    .any(|line| line.to_ascii_lowercase().contains("esc to interrupt"))
            {
                power.idle();
            }
        }
        Ok(summary)
    }

    /// Wakes a sleeping pane for input and notes the input. Answers whether
    /// the input ended the pane's wait for the user, for `write` to publish.
    pub(super) fn wake_for_input(
        &self,
        request: TerminalGenerationRequest,
        bytes: &[u8],
    ) -> Result<bool, TerminalError> {
        let (power, control) = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            (
                Arc::clone(&entry.power),
                Arc::clone(&entry.resources.control),
            )
        };
        let gate = transition_gate(&power);
        let _transition = lock(&gate);
        {
            let mut state = lock(&power);
            state.clear_retry();
            state.process = ProcessPower::WakePending;
        }
        control
            .resume()
            .map_err(|error| TerminalError::Backend(error.to_string()))?;
        let mut power = lock(&power);
        power.process = ProcessPower::Awake;
        if !crate::screen::is_terminal_report(bytes) {
            if bytes.contains(&b'\r') || bytes.contains(&b'\n') {
                power.working();
            } else if power.idle_since.is_some() {
                power.idle_since = Some(Instant::now());
            }
        }
        Ok(power.attention.input(bytes))
    }
}

/// Publishes one generation's "needs you" (host protocol v44) when its
/// summary no longer says what its monitor does. Each caller reads the
/// monitor under the event gate, so the last publish carries the newest
/// answer whichever thread changed it.
pub(super) fn publish_attention(
    shared: &Shared,
    id: TerminalId,
    generation: TerminalGeneration,
    power: &Mutex<WorkspacePower>,
) {
    let event_gate = lock(&shared.event_gate);
    let waiting = lock(power).waiting();
    let event = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id).filter(|entry| {
            entry.summary.generation == generation
                && entry.summary.state == TerminalState::Running
                && !entry.closing
        }) else {
            return;
        };
        if entry.summary.attention.is_some() == waiting {
            return;
        }
        let at = now();
        entry.summary.attention = waiting.then(|| TerminalAttentionV1 {
            version: TERMINAL_ATTENTION_VERSION,
            since: at.clone(),
        });
        entry.summary.updated_at = at;
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

/// One second of one terminal's sleep policy: retry a pending wake, or put an
/// inactive workspace that has idled past its grace period to sleep.
pub(super) fn tick_workspace_sleep(
    power: &Mutex<WorkspacePower>,
    control: &dyn TerminalProcessControl,
    profile: TerminalLaunchProfileId,
) {
    // The probe and resumes run under the transition gate alone, so the PTY
    // reader keeps draining output while the tree is listed and signalled.
    let gate = transition_gate(power);
    let _transition = lock(&gate);
    {
        let mut state = lock(power);
        state.profile(profile);
        if state.process == ProcessPower::WakePending {
            drop(state);
            if control.resume().is_ok() {
                let mut state = lock(power);
                state.process = ProcessPower::Awake;
                state.idle_since = Some(Instant::now());
            }
            return;
        }
        if !state.ready() {
            return;
        }
        state.process = ProcessPower::Suspending;
    }
    let outcome = control.suspend_idle(profile != TerminalLaunchProfileId::Shell);
    let resumed = outcome.is_err().then(|| control.resume().is_ok());
    let mut state = lock(power);
    match (outcome, resumed) {
        (Ok(true), _) => {
            state.process = ProcessPower::Suspended;
            state.clear_retry();
        }
        (Ok(false), _) => {
            state.process = ProcessPower::Awake;
            state.declined();
        }
        // Keep wake receipts after partial failures; never restart work.
        (Err(_), resumed) => {
            state.process = if resumed == Some(true) {
                ProcessPower::Awake
            } else {
                ProcessPower::WakePending
            };
            state.idle_since = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn power(profile: &str) -> WorkspacePower {
        let summary = serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-4000-8000-000000000001", "generation": 1,
            "title": "Test", "root": "/tmp", "cwd": "/tmp", "state": "running",
            "size": {"columns": 80, "rows": 24},
            "createdAt": "2026-09-19T00:00:00.000Z", "updatedAt": "2026-09-19T00:00:00.000Z",
            "launch": {"version": 1, "profileId": profile}
        }))
        .unwrap();
        let mut power = WorkspacePower::new(&summary);
        power.active = false;
        power
    }

    #[test]
    fn agy_and_muse_track_native_activity_and_permission_waits() {
        for profile in ["gemini", "muse"] {
            let mut power = power(profile);
            power.output(b"\x1b]2;Renamed chat\x07");
            assert!(power.idle_since.is_none());
            power.output(b"\x1b]2;interface-terminal-activity:v1:waiting\x07");
            assert!(power.idle_since.is_none());
            assert!(power.waiting());
            power.output(b"\x1b]2;interface-terminal-activity:v1:idle\x07");
            assert!(power.idle_since.is_some());
            assert!(!power.waiting());
            power.output(b"\x1b]2;interface-terminal-activity:v1:working\x07");
            assert!(power.idle_since.is_none());
        }
    }

    #[test]
    fn agy_osc7501_tracks_turns_and_permission_waits() {
        let mut power = power("gemini");
        power.output(b"\x1b]7501;state=working:id=t1:app=agy:msg=Thinking\x07");
        assert!(power.idle_since.is_none());
        power.output(b"\x1b]7501;state=blocked:id=t1:kind=permission:app=agy\x07");
        assert!(power.idle_since.is_none());
        assert!(power.waiting());
        power.output(b"\x1b]7501;state=working:id=t1:app=agy:msg=Finishing\x07");
        assert!(power.idle_since.is_none());
        assert!(!power.waiting());
        power.output(b"\x1b]7501;state=done:id=t1:app=agy\x07");
        assert!(power.idle_since.is_some());
        assert!(!power.waiting());
        power.output(b"\x1b]7501;state=working:id=t2:app=agy\x07");
        assert!(power.idle_since.is_none());
    }

    #[test]
    fn workspace_sleep_claude_waits_for_stop_and_ignores_names_and_silence() {
        let mut power = power("claude");
        power.output(b"\x1b]2;interface-claude-activity:working\x07");
        power.output("\x1b]2;✳ A renamed task\x07".as_bytes());
        assert!(power.idle_since.is_none());
        assert!(!power.ready());
        power.output(b"\x1b]2;interface-claude-activity:id");
        assert!(power.idle_since.is_none());
        power.output(b"le\x1b\\");
        assert!(!power.ready(), "completion still gets the grace period");
        power.idle_since = Instant::now().checked_sub(IDLE_GRACE);
        assert!(power.ready());
        power.active = true;
        assert!(!power.ready());
    }

    #[test]
    fn workspace_sleep_never_suspends_claude_waiting_on_a_permission_answer() {
        let mut power = power("claude");
        power.output(b"\x1b]2;interface-claude-activity:idle\x07");
        assert!(power.idle_since.is_some());
        power.output(b"\x1b]2;interface-claude-activity:waiting\x07");
        assert!(power.idle_since.is_none());
        assert!(!power.ready());
    }

    #[test]
    fn workspace_sleep_codex_completion_and_next_turn_preserve_byte_order() {
        let mut power = power("codex");
        power.output("\x1b]0;⠁ Thinking\x07\x1b]0;Renamed\x07".as_bytes());
        assert!(power.idle_since.is_none(), "a title is not completion");
        power.output(b"\x1b]9;Approval requested: command\x07");
        assert!(power.idle_since.is_none());
        power.output("\x1b]0;⠁ Thinking\x07\x1b]9;Agent turn complete\x07".as_bytes());
        assert!(power.idle_since.is_some());
        power.output("\x1b]9;Agent turn complete\x07\x1b]0;⠁ Next turn\x07".as_bytes());
        assert!(power.idle_since.is_none());
    }

    #[test]
    fn workspace_sleep_unknown_agents_and_approval_waits_stay_awake() {
        for profile in ["grok", "gemini", "omp"] {
            let mut power = power(profile);
            power.output(b"quiet output\x1b]2;arbitrary title\x07");
            assert!(power.idle_since.is_none());
        }
        let mut power = power("opencode");
        power.output(b"\x1b]2;interface-terminal-activity:v1:complete\x07");
        assert!(power.idle_since.is_some());
        power.output(b"\x1b]2;interface-terminal-activity:v1:waiting\x07");
        assert!(power.idle_since.is_none());
    }
}

#[cfg(test)]
mod declined_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DecliningControl(AtomicUsize);
    impl TerminalProcessControl for DecliningControl {
        fn resize(
            &self,
            _: interface_contracts::terminal::TerminalSize,
        ) -> Result<(), crate::TerminalBackendError> {
            Ok(())
        }
        fn terminate_tree(&self) -> Result<(), crate::TerminalBackendError> {
            Ok(())
        }
        fn suspend_idle(&self, _: bool) -> Result<bool, crate::TerminalBackendError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(false)
        }
    }

    fn idle_long_ago(power: &Mutex<WorkspacePower>) {
        lock(power).idle_since = Instant::now().checked_sub(IDLE_GRACE);
    }

    fn make_due(power: &Mutex<WorkspacePower>) {
        let mut power = lock(power);
        power.retry_after = power
            .retry_after
            .and_then(|at| at.checked_sub(DECLINED_RETRY_MAX));
    }

    /// Suspends only once the test lets it, reporting when the probe began.
    struct HeldControl {
        entered: std::sync::mpsc::Sender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl TerminalProcessControl for HeldControl {
        fn resize(
            &self,
            _: interface_contracts::terminal::TerminalSize,
        ) -> Result<(), crate::TerminalBackendError> {
            Ok(())
        }
        fn terminate_tree(&self) -> Result<(), crate::TerminalBackendError> {
            Ok(())
        }
        fn suspend_idle(&self, _: bool) -> Result<bool, crate::TerminalBackendError> {
            self.entered.send(()).unwrap();
            lock(&self.release)
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            Ok(true)
        }
    }

    #[test]
    fn output_is_read_while_a_suspension_probe_runs() {
        let power = Arc::new(Mutex::new(tests::power("claude")));
        lock(&power).output(b"\x1b]2;interface-claude-activity:idle\x07");
        idle_long_ago(&power);
        let (entered, probe_began) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let control = HeldControl {
            entered,
            release: Mutex::new(held),
        };
        let ticking = {
            let power = Arc::clone(&power);
            std::thread::spawn(move || {
                tick_workspace_sleep(&power, &control, TerminalLaunchProfileId::Claude);
            })
        };
        probe_began.recv().unwrap();
        // The reader is not held behind the probe, and the pane already
        // reports as sleeping, so input waits for the probe and then wakes it.
        let reader = power.try_lock().expect("the PTY reader was blocked");
        drop(reader);
        lock(&power).output(b"late output");
        assert!(lock(&power).suspended());
        assert!(!lock(&power).ready());
        release.send(()).unwrap();
        ticking.join().unwrap();
        assert_eq!(lock(&power).process, ProcessPower::Suspended);
        let idle = b"\x1b]2;interface-claude-activity:idle\x07".len();
        assert_eq!(
            lock(&power).output_cursor,
            (idle + b"late output".len()) as u64
        );
    }

    #[test]
    fn a_declined_suspension_backs_off_until_new_activity_evidence() {
        let power = Mutex::new(tests::power("claude"));
        lock(&power).output(b"\x1b]2;interface-claude-activity:idle\x07");
        idle_long_ago(&power);
        let control = DecliningControl(AtomicUsize::new(0));
        let profile = TerminalLaunchProfileId::Claude;

        tick_workspace_sleep(&power, &control, profile);
        assert_eq!(control.0.load(Ordering::SeqCst), 1);
        assert_eq!(lock(&power).retry_delay, IDLE_GRACE);
        // Not due yet: the next ticks do not probe the process tree.
        tick_workspace_sleep(&power, &control, profile);
        tick_workspace_sleep(&power, &control, profile);
        assert_eq!(control.0.load(Ordering::SeqCst), 1);

        let mut delays = Vec::new();
        for _ in 0..5 {
            make_due(&power);
            tick_workspace_sleep(&power, &control, profile);
            delays.push(lock(&power).retry_delay.as_secs());
        }
        assert_eq!(control.0.load(Ordering::SeqCst), 6);
        assert_eq!(delays, [10, 20, 40, 60, 60]);

        // Plain output (a background job logging) keeps the wait: the tree
        // that declined is still there.
        lock(&power).output(b"more");
        assert!(lock(&power).retry_after.is_some());
        // A turn that newly ends may have changed the tree: the wait starts over.
        lock(&power).output(b"\x1b]2;interface-claude-activity:working\x07");
        lock(&power).output(b"\x1b]2;interface-claude-activity:idle\x07");
        assert!(lock(&power).retry_after.is_none());
        idle_long_ago(&power);
        tick_workspace_sleep(&power, &control, profile);
        assert_eq!(control.0.load(Ordering::SeqCst), 7);
        assert_eq!(lock(&power).retry_delay, IDLE_GRACE);
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    #[test]
    fn workspace_sleep_shell_needs_a_prompt_and_silent_builtins_remain_working() {
        let mut power = tests::power("shell");
        assert!(power.idle_since.is_none());
        power.output(b"\x1b[?20");
        power.output(b"04h$ ");
        assert!(power.idle_since.is_some());
        power.output(b"\x1b[?2004l");
        assert!(power.idle_since.is_none());
        power.output(b"silent command output or a title\x1b]2;shell\x07");
        assert!(power.idle_since.is_none());
        power.output(b"\x1b[?2004h$ ");
        assert!(power.idle_since.is_some());
    }

    #[test]
    fn shell_mode_is_found_between_plain_bytes_and_other_escapes() {
        let mut power = tests::power("shell");
        power.output(b"plain output \x1b[31mred\x1b[0m then \x1b[?20");
        assert!(power.idle_since.is_none());
        power.output(b"04h$ ");
        assert!(power.idle_since.is_some());
        power.output(b"echo hi\r\nhi\x1b[?2004l");
        assert!(power.idle_since.is_none());
    }
}

#[cfg(test)]
#[path = "workspace_sleep_fixture_tests.rs"]
mod fixture_tests;
