//! Pass-off: where a handed-off pane starts, the transcript file it reads, and
//! the host-owned prompt that points it there.
use super::{
    ApprovedTerminalRoots, Arc, AuthorizedDirectory, CreateTerminalRequest, Duration, Instant,
    Path, PathBuf, ResolvedTerminalLaunch, SystemTime, TerminalError, TerminalGenerationRequest,
    TerminalHandoffKind, TerminalHandoffV1, TerminalId, TerminalLaunchProfileId, TerminalService,
    TerminalState, Write, bind_handoff_prompt, current_entry, live_process_cwd, lock, root_error,
};
use crate::launch::HandoffDelivery;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

/// A pass-off transcript is read once, by the agent spawned against it, so the
/// directory is swept on the same terms as the pasted-image scratch space.
const HANDOFF_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const HANDOFF_KEEP: usize = 64;

/// A typed pass-off waits for the CLI's editor to enable bracketed paste and
/// then for its output to rest this long, so the first frame has drawn.
const TYPED_HANDOFF_QUIET: Duration = Duration::from_millis(600);
const TYPED_HANDOFF_POLL: Duration = Duration::from_millis(100);
/// A TUI that never becomes ready is left alone: the prompt is dropped
/// unsent rather than typed into whatever it shows later.
const TYPED_HANDOFF_DEADLINE: Duration = Duration::from_secs(60);
/// Enter goes separately, after the paste has landed in the editor.
const TYPED_HANDOFF_SUBMIT_DELAY: Duration = Duration::from_millis(150);

/// Scratch space for pass-off transcripts when the host names no directory.
pub(super) fn default_handoff_directory() -> PathBuf {
    std::env::temp_dir().join("Interface").join("handoffs")
}

impl TerminalService {
    /// Point a resolved launch at this pass-off: the host-owned prompt as the
    /// trailing argument, and the transcript file that prompt names on disk. A
    /// create without a handoff is returned unchanged. A CLI that takes no
    /// initial prompt gets it back as the text to type once its TUI is ready
    /// ([`Self::type_handoff_when_ready`]).
    ///
    /// The prompt names the file before the file exists, so a profile that
    /// cannot take one is refused without leaving a transcript behind.
    pub(super) fn pass_off_launch(
        &self,
        request: &CreateTerminalRequest,
        launch: ResolvedTerminalLaunch,
        cwd: &Path,
    ) -> Result<(ResolvedTerminalLaunch, Option<String>), TerminalError> {
        let Some(handoff) = request.handoff.as_ref() else {
            return Ok((launch, None));
        };
        let profile_id = request.launch.map(|launch| launch.profile_id).ok_or(
            TerminalError::HandoffUnsupportedProfile(TerminalLaunchProfileId::Shell),
        )?;
        let mut launch = launch;
        let file = self.handoff_file(request.id);
        let prompt = handoff_prompt(handoff, &file);
        let typed = match bind_handoff_prompt(&mut launch, profile_id, &prompt)? {
            HandoffDelivery::Argument => None,
            HandoffDelivery::Typed(prompt) => Some(prompt),
        };
        self.write_handoff_file(&file, handoff, cwd)?;
        Ok((launch, typed))
    }

    /// Type a pass-off prompt into a CLI that takes none on argv, once: after
    /// its editor has enabled bracketed paste and its output has rested, as
    /// one paste and then Enter. The prompt is one line, so the paste can
    /// neither submit early nor answer a dialog. A pane that ends, restarts
    /// or never becomes ready within the deadline is never typed into. A
    /// pass-off pane tracks its paste mode from its first byte, so the
    /// editor's switch cannot be missed before this starts watching.
    pub(super) fn type_handoff_when_ready(
        &self,
        request: TerminalGenerationRequest,
        prompt: String,
    ) {
        let Some(power) = ({
            let state = lock(&self.shared.state);
            current_entry(&state, request.id, request.generation)
                .ok()
                .map(|entry| Arc::clone(&entry.power))
        }) else {
            return;
        };
        let shared = Arc::downgrade(&self.shared);
        std::thread::spawn(move || {
            let deadline = Instant::now() + TYPED_HANDOFF_DEADLINE;
            let mut rested: Option<(u64, Instant)> = None;
            let service = loop {
                std::thread::sleep(TYPED_HANDOFF_POLL);
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                let running = {
                    let state = lock(&shared.state);
                    current_entry(&state, request.id, request.generation)
                        .is_ok_and(|entry| entry.summary.state == TerminalState::Running)
                };
                if !running {
                    return;
                }
                if Instant::now() >= deadline {
                    // Definitely unsent; the transcript file stays for the
                    // owner to point the pane at, and is swept as usual.
                    return;
                }
                let (ready, cursor) = lock(&power).paste_ready();
                match rested {
                    _ if !ready => rested = None,
                    Some((seen, since)) if seen == cursor => {
                        if since.elapsed() >= TYPED_HANDOFF_QUIET {
                            break TerminalService { shared };
                        }
                    }
                    _ => rested = Some((cursor, Instant::now())),
                }
            };
            let paste = [b"\x1b[200~".as_slice(), prompt.as_bytes(), b"\x1b[201~"].concat();
            if service.write(request, &paste).is_err() {
                return;
            }
            drop(service);
            std::thread::sleep(TYPED_HANDOFF_SUBMIT_DELAY);
            if let Some(shared) = shared.upgrade() {
                let _ = TerminalService { shared }.write(request, b"\r");
            }
        });
    }

    /// The pass-off pane starts where the source agent actually works: the
    /// live cwd of its child process when readable and inside an approved
    /// root, else the source pane's recorded cwd, else the requested cwd.
    ///
    /// Reading the live cwd shells out and is bounded at roughly two seconds.
    /// `create` already runs on a blocking worker, and this sits outside the
    /// lifecycle gate, so a slow answer delays only its own request.
    pub(super) fn pass_off_directory(
        &self,
        source_id: TerminalId,
        requested: &str,
        approved: &ApprovedTerminalRoots,
    ) -> Result<AuthorizedDirectory, TerminalError> {
        let (recorded_cwd, pid) = {
            let state = lock(&self.shared.state);
            let Some(entry) = state.entries.get(&source_id) else {
                return Err(TerminalError::HandoffSourceUnavailable);
            };
            if entry.summary.state != TerminalState::Running {
                return Err(TerminalError::HandoffSourceUnavailable);
            }
            (entry.cwd.clone(), entry.resources.control.process_id())
        };
        if let Some(live) = pid.and_then(live_process_cwd)
            && let Ok(directory) = approved.authorize_pass_off(&live)
        {
            return Ok(directory);
        }
        if let Ok(directory) = approved.authorize_pass_off(&recorded_cwd) {
            return Ok(directory);
        }
        approved
            .authorize(Path::new(requested))
            .map(AuthorizedDirectory::exact)
            .map_err(root_error)
    }

    /// Drop a transcript whose pane never started: the agent that would have
    /// read it does not exist, and nothing else reaps the file before the next
    /// prune.
    pub(super) fn discard_handoff(&self, request: &CreateTerminalRequest) {
        if request.handoff.is_some() {
            let _ = std::fs::remove_file(self.handoff_file(request.id));
        }
    }

    fn handoff_file(&self, id: TerminalId) -> PathBuf {
        self.shared.handoff_directory.join(format!("{id}.md"))
    }

    /// A transcript is verbatim terminal output, so it lands on the same terms
    /// the pasted-image scratch space already uses for host-written scratch
    /// files: a 0700 directory, a 0600 file, and no symbolic link at that path
    /// is ever followed.
    fn write_handoff_file(
        &self,
        file: &Path,
        handoff: &TerminalHandoffV1,
        cwd: &Path,
    ) -> Result<(), TerminalError> {
        let directory = &self.shared.handoff_directory;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(directory)
            .map_err(|error| TerminalError::Backend(error.to_string()))?;
        prune_handoffs(directory);
        let from = handoff.from_title.as_deref().unwrap_or("another agent");
        let fence = transcript_fence(&handoff.transcript);
        // The transcript is up to half a megabyte; it goes to the file straight
        // from the request rather than through a second formatted copy.
        let header = match handoff.kind.unwrap_or_default() {
            TerminalHandoffKind::Transcript => format!(
                "# Pass-off transcript\n\nFrom: {from}\nWorking directory: {}\n\n\
                 Rendered terminal transcript, newest last; TUI chrome may appear.\n\n\
                 {fence}text\n",
                cwd.display(),
            ),
            // A brief names the pane to answer, so the agent reading it can
            // report back through the same pane tools that sent it.
            TerminalHandoffKind::Brief => format!(
                "# Pass-off brief\n\nFrom: {from} (pane {source})\nWorking directory: {}\n\n\
                 When the task is done, report the result to the agent that sent it: \
                 call the send_to_pane tool with pane_id {source} and a short message, \
                 then stop.\n\n{fence}text\n",
                cwd.display(),
                source = handoff.source_terminal_id,
            ),
        };
        let footer = format!("\n{fence}\n");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        options
            .open(file)
            .and_then(|mut handle| {
                handle.write_all(header.as_bytes())?;
                handle.write_all(handoff.transcript.as_bytes())?;
                handle.write_all(footer.as_bytes())
            })
            .map_err(|error| TerminalError::Backend(error.to_string()))
    }
}

/// Refuse a pass-off that cannot work before it reaches cwd resolution: a
/// malformed payload, or a profile with no prompt to take. A create without a
/// handoff passes through untouched.
pub(super) fn validated_pass_off(request: &CreateTerminalRequest) -> Result<(), TerminalError> {
    let Some(handoff) = request.handoff.as_ref() else {
        return Ok(());
    };
    validated_handoff(handoff)?;
    // A login shell has nothing to hand a prompt to, whether the request omits
    // the profile or names the shell outright.
    if request
        .launch
        .is_none_or(|launch| launch.profile_id == TerminalLaunchProfileId::Shell)
    {
        return Err(TerminalError::HandoffUnsupportedProfile(
            TerminalLaunchProfileId::Shell,
        ));
    }
    Ok(())
}

/// Bound the renderer-supplied pass-off payload before any of it reaches the
/// filesystem or an argument vector.
fn validated_handoff(handoff: &TerminalHandoffV1) -> Result<(), TerminalError> {
    use interface_contracts::terminal::{
        TERMINAL_HANDOFF_TITLE_MAX_BYTES, TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES,
        TERMINAL_HANDOFF_VERSION,
    };

    let title_ok = handoff.from_title.as_deref().is_none_or(|title| {
        !title.is_empty()
            && title.len() <= TERMINAL_HANDOFF_TITLE_MAX_BYTES
            && !title.chars().any(char::is_control)
    });
    if handoff.version != TERMINAL_HANDOFF_VERSION
        || handoff.transcript.len() > TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES
        || handoff.transcript.contains('\0')
        || !title_ok
    {
        return Err(TerminalError::InvalidHandoff);
    }
    Ok(())
}

/// The host-owned opening instruction for a pass-off pane. The renderer (or
/// the pane tool) supplies the file's content, never a word of this sentence.
fn handoff_prompt(handoff: &TerminalHandoffV1, file: &Path) -> String {
    let from = handoff
        .from_title
        .as_deref()
        .map_or(String::new(), |title| format!(" ({title})"));
    match handoff.kind.unwrap_or_default() {
        TerminalHandoffKind::Transcript => format!(
            "You are taking over for another agent{from}. Read {} - its terminal \
             transcript, newest last - then continue its work in this directory. \
             Do not redo work the transcript shows as finished.",
            file.display()
        ),
        TerminalHandoffKind::Brief => format!(
            "Another agent{from} passed you a task. Read {}, do it in this \
             directory, and report back the way the file says.",
            file.display()
        ),
    }
}

/// A code fence the transcript cannot close: one backtick longer than its
/// longest run, never shorter than three. Agent panes print fenced code all
/// day, and a transcript that closed the block early would hand the next agent
/// its own instructions as prose.
fn transcript_fence(transcript: &str) -> String {
    let mut longest = 0_usize;
    let mut run = 0_usize;
    for character in transcript.chars() {
        if character == '`' {
            run = run.saturating_add(1);
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.saturating_add(1).max(3))
}

/// Drop transcripts past the TTL and everything beyond the newest
/// `HANDOFF_KEEP`. Regular files only, and symbolic links are removed from
/// consideration rather than followed.
fn prune_handoffs(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    let mut kept = Vec::new();
    for entry in entries.flatten() {
        // `symlink_metadata` below is the authoritative check, and readdir's own
        // type is often unknown on macOS, which would stat here anyway.
        let target = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&target) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age > HANDOFF_TTL)
        {
            let _ = std::fs::remove_file(target);
        } else {
            kept.push((target, modified));
        }
    }
    kept.sort_by(|left, right| right.1.cmp(&left.1));
    for (target, _) in kept.into_iter().skip(HANDOFF_KEEP) {
        let _ = std::fs::remove_file(target);
    }
}
