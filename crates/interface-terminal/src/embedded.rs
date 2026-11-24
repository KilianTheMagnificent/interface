use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

use interface_common::sync::lock;
use interface_contracts::terminal::{TerminalExit, TerminalLaunchProfileId, TerminalSize};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
#[cfg(unix)]
use rustix::io::Errno;
#[cfg(unix)]
use rustix::process::{Pid, Signal};

use crate::error::TerminalBackendError;
use crate::launch::ResolvedTerminalLaunch;

/// Spawn-time inputs available to an embedded PTY backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSpawnSpec {
    pub cwd: PathBuf,
    pub size: TerminalSize,
    pub launch: ResolvedTerminalLaunch,
}

/// A separately synchronized input handle for one PTY.
pub trait TerminalInput: Send + Sync {
    /// Write exact bytes to the PTY. The implementation must not interpret
    /// them as a shell command string.
    ///
    /// # Errors
    ///
    /// Returns a backend error when input is closed or the write fails.
    fn write(&self, bytes: &[u8]) -> Result<(), TerminalBackendError>;

    /// Close the input side. Calls after the first are idempotent.
    fn close(&self);
}

/// Process and terminal controls that may run concurrently with output reads.
pub trait TerminalProcessControl: Send + Sync {
    /// # Errors
    ///
    /// Returns a backend error when the PTY is closed or resizing fails.
    fn resize(&self, size: TerminalSize) -> Result<(), TerminalBackendError>;

    /// Tear down the shell and its PTY process group. Calls after the first
    /// are idempotent.
    ///
    /// # Errors
    ///
    /// Returns a backend error when process termination fails.
    fn terminate_tree(&self) -> Result<(), TerminalBackendError>;

    /// Suspend an idle terminal without replacing its PTY or process identity.
    /// A shell with any child job must remain awake. Unsupported backends decline.
    /// # Errors
    /// Returns an error if exact process ownership or signalling cannot be verified.
    fn suspend_idle(&self, _agent_idle: bool) -> Result<bool, TerminalBackendError> {
        Ok(false)
    }

    /// Resume only process groups stopped by this control.
    /// # Errors
    /// Returns an error when a retained process group cannot be resumed.
    fn resume(&self) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    /// The direct child's OS pid, when the backend retains one. Pass-off uses
    /// it to read the live cwd; test doubles may leave the default `None`.
    fn process_id(&self) -> Option<u32> {
        None
    }

    /// The spawn-time identity of the direct child, when the backend can pin
    /// it. Provider metadata lookups use both fields so a recycled PID can
    /// never be mistaken for the terminal generation that originally owned
    /// it.
    fn process_identity(&self) -> Option<TerminalProcessIdentity> {
        None
    }

    /// Identity of the process-group leader that currently owns this retained
    /// PTY's foreground. Interactive shells place foreground commands in a
    /// child process group, so this is distinct from [`Self::process_identity`].
    ///
    /// # Errors
    ///
    /// Returns a backend error when the foreground identity cannot be read
    /// safely from the operating system.
    fn foreground_process_identity(
        &self,
    ) -> Result<Option<TerminalProcessIdentity>, TerminalBackendError> {
        Ok(None)
    }

    /// Best-effort display identity of the foreground job, never session or
    /// process-ownership authority. No argv, environment, or provider store is read.
    ///
    /// # Errors
    /// Returns an error when the foreground job cannot be inspected safely.
    fn foreground_profile(&self) -> Result<Option<TerminalLaunchProfileId>, TerminalBackendError> {
        Ok(None)
    }

    /// Whether an exact previously captured child identity remains live.
    /// Implementations must include their stable start/TTY evidence rather
    /// than treating a recycled PID as the same process.
    ///
    /// # Errors
    ///
    /// Returns a backend error when exact process liveness cannot be
    /// established safely.
    fn process_identity_is_live(
        &self,
        _process: &TerminalProcessIdentity,
    ) -> Result<bool, TerminalBackendError> {
        Ok(false)
    }

    /// Whether any process remains in the exact captured foreground process
    /// group. A provider leader can exit while descendants keep that group
    /// alive, so teardown must use this stronger check before releasing a
    /// workspace lease.
    ///
    /// # Errors
    ///
    /// Returns a backend error when exact process-group liveness cannot be
    /// established safely.
    fn process_group_identity_is_live(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<bool, TerminalBackendError> {
        self.process_identity_is_live(process)
    }

    /// Best-effort live cwd for an exact child identity. The default refuses
    /// to guess; system controls revalidate the identity before probing it.
    ///
    /// # Errors
    ///
    /// Returns a backend error when the process cwd cannot be read safely.
    fn process_cwd(
        &self,
        _process: &TerminalProcessIdentity,
    ) -> Result<Option<PathBuf>, TerminalBackendError> {
        Ok(None)
    }
}

/// Stable spawn-time identity for a terminal's direct provider process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalProcessIdentity {
    pub process_id: u32,
    pub process_group_id: u32,
    /// Retained controlling-terminal device on macOS. It distinguishes two
    /// process instances even when PID reuse lands in `lstart`'s one-second
    /// resolution window.
    pub terminal_device: Option<String>,
    /// `ps lstart` in the C locale and UTC, normalized to single spaces.
    pub process_start: String,
}

/// The uniquely-owned blocking wait handle for a spawned terminal process.
pub trait TerminalProcessWaiter: Send {
    /// # Errors
    ///
    /// Returns a backend error when the platform wait fails.
    fn wait(&mut self) -> Result<TerminalExit, TerminalBackendError>;
}

/// Split handles returned by a PTY backend so reads, writes, resizes, and
/// process waiting never serialize behind one blocking mutex.
pub struct SpawnedTerminal {
    pub reader: Box<dyn Read + Send>,
    pub input: Arc<dyn TerminalInput>,
    pub control: Arc<dyn TerminalProcessControl>,
    pub waiter: Box<dyn TerminalProcessWaiter>,
}

/// Injectable PTY factory used by the production service and deterministic
/// tests.
pub trait TerminalPtyBackend: Send + Sync {
    /// # Errors
    ///
    /// Returns a backend error when the PTY or child process cannot be
    /// created.
    fn spawn(&self, spec: &TerminalSpawnSpec) -> Result<SpawnedTerminal, TerminalBackendError>;
}

struct SystemTerminalInput {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
}

impl TerminalInput for SystemTerminalInput {
    fn write(&self, bytes: &[u8]) -> Result<(), TerminalBackendError> {
        let mut writer = lock(&self.writer);
        let writer = writer
            .as_mut()
            .ok_or_else(|| TerminalBackendError::new("terminal input is closed"))?;
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| TerminalBackendError::new(error.to_string()))
    }

    fn close(&self) {
        lock(&self.writer).take();
    }
}

struct SystemTerminalControl {
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    input: Arc<SystemTerminalInput>,
    killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
    #[cfg(unix)]
    process_group_receipt: ProcessGroupReceipt,
    #[cfg(unix)]
    sleeping_groups: Mutex<Vec<ProcessGroupReceipt>>,
    #[cfg(unix)]
    foreground_profile_cache: Mutex<crate::foreground_profile::ForegroundProfileCache>,
    terminated: Mutex<bool>,
    exited: Arc<AtomicBool>,
}

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcessGroupReceipt {
    process_id: u32,
    process_group_id: u32,
    process_start: String,
    identity: ProcessIdentity,
}

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum ProcessIdentity {
    // The master side of this PTY remains owned by `SystemTerminalControl` for
    // the receipt's entire lifetime. macOS cannot allocate this device to a
    // different PTY while that master is live, so the kernel terminal-device
    // number distinguishes even a PID/PGID recycled inside `ps lstart`'s
    // one-second resolution window.
    #[cfg(target_os = "macos")]
    RetainedPty { terminal_device: String },
    #[cfg(not(target_os = "macos"))]
    ProcessStart(String),
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManagedProcessGroupStatus {
    Absent,
    Owned,
    Recycled,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessGroupProbe {
    Signalable,
    Absent,
    Unsignalable,
}

#[cfg(unix)]
impl ProcessGroupProbe {
    fn exists_for_teardown(self) -> bool {
        matches!(self, Self::Signalable)
    }

    fn is_authoritatively_present(self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// Reads a `kill(-pgid, 0)` answer. Any errno but ESRCH and EPERM proves
/// nothing about the group either way.
#[cfg(unix)]
fn classify_process_group_probe(answer: Result<(), Errno>) -> Result<ProcessGroupProbe, Errno> {
    match answer {
        Ok(()) => Ok(ProcessGroupProbe::Signalable),
        Err(Errno::SRCH) => Ok(ProcessGroupProbe::Absent),
        Err(Errno::PERM) => Ok(ProcessGroupProbe::Unsignalable),
        Err(errno) => Err(errno),
    }
}

#[cfg(unix)]
const PROCESS_GROUP_HUP_GRACE: Duration = Duration::from_millis(200);
#[cfg(unix)]
const PROCESS_GROUP_HUP_POLL: Duration = Duration::from_millis(5);
#[cfg(unix)]
const PROCESS_GROUP_KILL_GRACE: Duration = Duration::from_millis(500);

/// The `kill(2)` target for `process_id`. No process carries an id beyond
/// `i32::MAX`, and 0 would address the caller's own process group, so neither
/// is ever signalled or probed.
#[cfg(unix)]
fn kill_target(process_id: u32) -> Result<Pid, Errno> {
    i32::try_from(process_id)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(Errno::INVAL)
}

/// Names a refused group signal in its error by the flag `/bin/kill` took,
/// keeping that error text byte-identical.
#[cfg(unix)]
fn signal_flag(signal: Signal) -> &'static str {
    match signal {
        Signal::HUP => "-HUP",
        Signal::KILL => "-KILL",
        Signal::STOP => "-STOP",
        Signal::CONT => "-CONT",
        _ => "-?",
    }
}

#[cfg(unix)]
fn raw_signal_process_group(process_group_id: u32, signal: Signal) -> bool {
    // `portable-pty` creates the child as a session leader, so its process id
    // is also its PTY process group's id: `kill(-pgid)` reaches the group.
    kill_target(process_group_id)
        .and_then(|group| rustix::process::kill_process_group(group, signal))
        .is_ok()
}

#[cfg(unix)]
fn wait_for_hup_exit(exited: &AtomicBool) {
    let deadline = Instant::now() + PROCESS_GROUP_HUP_GRACE;
    while !exited.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(PROCESS_GROUP_HUP_POLL);
    }
}

#[cfg(unix)]
fn probe_process_group(process_group_id: u32) -> std::io::Result<ProcessGroupProbe> {
    // `kill(-pgid, 0)` answers EPERM, not ESRCH, when the group still exists
    // but none of its members can be signalled. On macOS that includes a group
    // whose only members are exited-but-unreaped zombies: XNU's killpg skips
    // exiting processes and reports EPERM when it signalled nothing. A HUP'd
    // shell sits in exactly that state until the waiter thread reaps it.
    // Either way nothing in the group can be signalled from here, so it is
    // absent for teardown purposes.
    kill_target(process_group_id)
        .and_then(|group| {
            classify_process_group_probe(rustix::process::test_kill_process_group(group))
        })
        .map_err(|errno| {
            std::io::Error::other(format!(
                "could not verify process group {process_group_id}: {errno}"
            ))
        })
}

#[cfg(unix)]
fn process_group_exists(process_id: u32) -> std::io::Result<bool> {
    probe_process_group(process_id).map(ProcessGroupProbe::exists_for_teardown)
}

#[cfg(unix)]
fn captured_process_group_is_present(process_id: u32) -> std::io::Result<bool> {
    probe_process_group(process_id).map(ProcessGroupProbe::is_authoritatively_present)
}

/// Whether `process_id` names a live process, through kill(2) signal 0
/// rather than a `/bin/kill` fork.
///
/// # Errors
///
/// Unlike a group's, one process's EPERM (alive, but not ours to signal) is
/// an error rather than either answer.
#[cfg(unix)]
pub fn process_exists(process_id: u32) -> std::io::Result<bool> {
    match kill_target(process_id).and_then(rustix::process::test_kill_process) {
        Ok(()) => Ok(true),
        Err(Errno::SRCH) => Ok(false),
        Err(errno) => Err(std::io::Error::other(format!(
            "could not verify process {process_id}: {errno}"
        ))),
    }
}

#[cfg(unix)]
fn process_snapshot(process_id: u32) -> std::io::Result<Option<ProcessGroupReceipt>> {
    #[cfg(target_os = "macos")]
    let columns = ["pid", "pgid", "tdev", "lstart"];
    #[cfg(not(target_os = "macos"))]
    let columns = ["pid", "pgid", "lstart"];
    let output = crate::PsProbe {
        columns: &columns,
        select: &["-p", &process_id.to_string()],
        utc: true,
        stderr: true,
    }
    .output()?;
    if !output.status.success() {
        if !process_exists(process_id)? {
            return Ok(None);
        }
        return Err(std::io::Error::other(format!(
            "could not read process {process_id} group identity: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|error| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            format!("process {process_id} identity was not UTF-8: {error}"),
        )
    })?;
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            format!("process {process_id} identity was empty"),
        )
    })?;
    if lines.next().is_some() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("process {process_id} identity was ambiguous"),
        ));
    }
    let mut fields = line.split_whitespace();
    let observed_process_id = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                format!("process {process_id} identity omitted its PID"),
            )
        })?;
    let process_group_id = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                format!("process {process_id} identity omitted its process group"),
            )
        })?;
    #[cfg(target_os = "macos")]
    let identity = fields
        .next()
        .filter(|terminal_device| !terminal_device.is_empty())
        .map(|terminal_device| ProcessIdentity::RetainedPty {
            terminal_device: terminal_device.to_owned(),
        })
        .ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                format!("process {process_id} identity omitted its terminal device"),
            )
        })?;
    let process_start = fields.collect::<Vec<_>>().join(" ");
    #[cfg(target_os = "macos")]
    let invalid_identity = process_start.is_empty();
    #[cfg(not(target_os = "macos"))]
    let (identity, invalid_identity) = (
        ProcessIdentity::ProcessStart(process_start.clone()),
        process_start.is_empty(),
    );
    if observed_process_id != process_id || invalid_identity {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("process {process_id} returned an invalid group identity"),
        ));
    }
    Ok(Some(ProcessGroupReceipt {
        process_id: observed_process_id,
        process_group_id,
        process_start,
        identity,
    }))
}

#[cfg(unix)]
fn foreground_process_group_id(process_id: u32) -> std::io::Result<Option<u32>> {
    let output = crate::PsProbe {
        columns: &["tpgid"],
        select: &["-p", &process_id.to_string()],
        utc: false,
        stderr: true,
    }
    .output()?;
    if !output.status.success() {
        if !process_exists(process_id)? {
            return Ok(None);
        }
        return Err(std::io::Error::other(format!(
            "could not read process {process_id} foreground group: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|error| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                format!("process {process_id} foreground group was not UTF-8: {error}"),
            )
        })?
        .trim();
    if value.is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("process {process_id} foreground group was empty"),
        ));
    }
    let Ok(group) = value.parse::<i64>() else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("process {process_id} returned an invalid foreground group"),
        ));
    };
    Ok(u32::try_from(group).ok().filter(|group| *group != 0))
}

#[cfg(unix)]
fn capture_process_group_receipt(
    process_id: u32,
    foreground_process_group: Option<i32>,
) -> std::io::Result<ProcessGroupReceipt> {
    let receipt = process_snapshot(process_id)?.ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::NotFound,
            format!("terminal process {process_id} exited before its identity was captured"),
        )
    })?;
    if receipt.process_group_id != process_id {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "terminal process {process_id} was not its process-group leader (group {})",
                receipt.process_group_id
            ),
        ));
    }
    // `portable-pty` calls `setsid` before exec. Requiring the child PID to be
    // both its PGID and the kernel-reported foreground PGID proves that the
    // receipt belongs to that new PTY session rather than an ambient process.
    // A shell that starts a job at once may already have handed the
    // foreground to that job's group; that group is accepted only when the
    // child leads its own session and the group's leader is in it, which
    // proves the same ownership.
    let foreground_process_group =
        foreground_process_group.and_then(|group| u32::try_from(group).ok());
    if foreground_process_group != Some(process_id)
        && !foreground_process_group
            .is_some_and(|group| foreground_group_joined_session(process_id, group))
    {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("terminal process {process_id} did not own its controlling PTY"),
        ));
    }
    #[cfg(target_os = "macos")]
    if matches!(
        &receipt.identity,
        ProcessIdentity::RetainedPty { terminal_device }
            if terminal_device == "??" || terminal_device == "-"
    ) {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("terminal process {process_id} had no controlling PTY"),
        ));
    }
    Ok(receipt)
}

/// Whether `session_leader` leads its own session and `group`'s leader is a
/// member of that session. A PTY's foreground group always belongs to the
/// session the PTY controls, so this proves the PTY is the leader's. A group
/// whose leader has gone, or any unreadable session, proves nothing.
#[cfg(unix)]
fn foreground_group_joined_session(session_leader: u32, group: u32) -> bool {
    let session_of = |process_id: u32| {
        kill_target(process_id)
            .ok()
            .and_then(|pid| rustix::process::getsid(Some(pid)).ok())
            .and_then(|session| u32::try_from(session.as_raw_pid()).ok())
    };
    session_of(session_leader) == Some(session_leader) && session_of(group) == Some(session_leader)
}

#[cfg(unix)]
fn require_process_id(process_id: Option<u32>) -> std::io::Result<u32> {
    process_id.ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            "terminal backend did not report a process ID",
        )
    })
}

#[cfg(unix)]
fn classify_process_group(
    receipt: &ProcessGroupReceipt,
    group_exists: bool,
    current_leader: Option<&ProcessGroupReceipt>,
) -> ManagedProcessGroupStatus {
    if !group_exists {
        return ManagedProcessGroupStatus::Absent;
    }
    match current_leader {
        Some(current) if current == receipt => ManagedProcessGroupStatus::Owned,
        Some(_) => ManagedProcessGroupStatus::Recycled,
        // A leaderless group cannot share its numeric PGID with a newly
        // allocated PID, so it is still the spawned terminal's descendants.
        None => ManagedProcessGroupStatus::Owned,
    }
}

#[cfg(unix)]
fn managed_process_group_status(
    receipt: &ProcessGroupReceipt,
) -> std::io::Result<ManagedProcessGroupStatus> {
    let group_exists = process_group_exists(receipt.process_group_id)?;
    if !group_exists {
        return Ok(ManagedProcessGroupStatus::Absent);
    }
    let current_leader = process_snapshot(receipt.process_id)?;
    if current_leader.is_none() && !process_group_exists(receipt.process_group_id)? {
        return Ok(ManagedProcessGroupStatus::Absent);
    }
    Ok(classify_process_group(
        receipt,
        true,
        current_leader.as_ref(),
    ))
}

#[cfg(unix)]
fn signal_managed_process_group(
    receipt: &ProcessGroupReceipt,
    signal: Signal,
) -> std::io::Result<bool> {
    if managed_process_group_status(receipt)? != ManagedProcessGroupStatus::Owned {
        return Ok(false);
    }
    if raw_signal_process_group(receipt.process_group_id, signal) {
        return Ok(true);
    }
    if managed_process_group_status(receipt)? == ManagedProcessGroupStatus::Owned {
        return Err(std::io::Error::other(format!(
            "process-group {} signal was refused",
            signal_flag(signal)
        )));
    }
    Ok(false)
}

#[cfg(unix)]
fn force_kill_process_group(receipt: &ProcessGroupReceipt) -> std::io::Result<()> {
    if !signal_managed_process_group(receipt, Signal::KILL)? {
        return Ok(());
    }

    // A successful `kill` status only proves that the signal was accepted.
    // Do not let callers release the terminal's worktree lease until the
    // entire group is observably absent.
    let deadline = Instant::now() + PROCESS_GROUP_KILL_GRACE;
    loop {
        if managed_process_group_status(receipt)? != ManagedProcessGroupStatus::Owned {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                format!("process group {} survived KILL", receipt.process_group_id),
            ));
        }
        thread::sleep(PROCESS_GROUP_HUP_POLL);
    }
}

fn process_is_already_gone(error: &std::io::Error) -> bool {
    if error.kind() == ErrorKind::NotFound {
        return true;
    }
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn finalize_verified_teardown<Master, Killer>(
    result: Result<(), TerminalBackendError>,
    input: &dyn TerminalInput,
    master: &Mutex<Option<Master>>,
    killer: &Mutex<Option<Killer>>,
    terminated: &mut bool,
) -> Result<(), TerminalBackendError> {
    if result.is_ok() {
        input.close();
        lock(master).take();
        lock(killer).take();
        *terminated = true;
    }
    result
}

mod orphan;
mod workspace_sleep_process;
pub use orphan::settle_stopped_orphan_process_group;

impl TerminalProcessControl for SystemTerminalControl {
    fn suspend_idle(&self, agent_idle: bool) -> Result<bool, TerminalBackendError> {
        workspace_sleep_process::suspend(self, agent_idle)
    }

    fn resume(&self) -> Result<(), TerminalBackendError> {
        workspace_sleep_process::resume(self)
    }

    fn resize(&self, size: TerminalSize) -> Result<(), TerminalBackendError> {
        let master = lock(&self.master);
        let master = master
            .as_ref()
            .ok_or_else(|| TerminalBackendError::new("terminal process is closed"))?;
        master
            .resize(PtySize {
                rows: size.rows,
                cols: size.columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalBackendError::new(error.to_string()))
    }

    fn terminate_tree(&self) -> Result<(), TerminalBackendError> {
        // Serialize teardown so a waiter and an explicit close cannot race.
        // Mark completion only after verified cleanup; a failed attempt keeps
        // every handle needed by a later retry.
        let mut terminated = lock(&self.terminated);
        if *terminated {
            return Ok(());
        }
        self.resume()?;
        #[cfg(unix)]
        let killed = match signal_managed_process_group(&self.process_group_receipt, Signal::HUP) {
            Ok(true) => {
                // A managed terminal supervisor may need to persist its
                // suspended state after HUP. Give that handler one short,
                // bounded window to exit before KILL becomes the backstop.
                wait_for_hup_exit(&self.exited);
                force_kill_process_group(&self.process_group_receipt)
            }
            Ok(false) => Ok(()),
            Err(error) => Err(error),
        };
        #[cfg(not(unix))]
        let killed = if self.exited.load(Ordering::Acquire) {
            Ok(())
        } else {
            lock(&self.killer)
                .as_mut()
                .map_or(Ok(()), |killer| killer.kill())
        };
        let result = killed
            .or_else(|error| {
                if process_is_already_gone(&error) {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .map_err(|error| TerminalBackendError::new(error.to_string()));
        // Kill before closing input: close() blocks on the writer mutex, and a
        // PTY write parked behind a non-reading child only unparks once the
        // process group is dead. More importantly, a failed verification must
        // retain the master PTY: it is the macOS process-identity anchor used
        // by a later teardown retry.
        finalize_verified_teardown(
            result,
            self.input.as_ref(),
            &self.master,
            &self.killer,
            &mut terminated,
        )
    }

    fn process_id(&self) -> Option<u32> {
        // The spawn-time pid is already retained here, verified against `ps`,
        // as the receipt that pins this PTY's process-group identity.
        #[cfg(unix)]
        {
            Some(self.process_group_receipt.process_id)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn process_identity(&self) -> Option<TerminalProcessIdentity> {
        #[cfg(unix)]
        {
            Some(terminal_process_identity(&self.process_group_receipt))
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn foreground_process_identity(
        &self,
    ) -> Result<Option<TerminalProcessIdentity>, TerminalBackendError> {
        #[cfg(unix)]
        {
            self.foreground_process_receipt()
                .map(|receipt| receipt.map(|receipt| terminal_process_identity(&receipt)))
                .map_err(|error| TerminalBackendError::new(error.to_string()))
        }
        #[cfg(not(unix))]
        {
            Ok(None)
        }
    }

    fn foreground_profile(&self) -> Result<Option<TerminalLaunchProfileId>, TerminalBackendError> {
        #[cfg(unix)]
        {
            let group = lock(&self.master)
                .as_ref()
                .and_then(|master| master.process_group_leader());
            let Some(group) = group.and_then(|group| u32::try_from(group).ok()) else {
                return Ok(None);
            };
            // The common idle-shell path makes no subprocess calls.
            if group == self.process_group_receipt.process_group_id {
                lock(&self.foreground_profile_cache).clear();
                return Ok(None);
            }
            // A job that stays in front is not re-read every poll.
            lock(&self.foreground_profile_cache).profile_or_read(group, Instant::now(), || {
                let profile = crate::foreground_profile::read(group)?;
                let current = lock(&self.master)
                    .as_ref()
                    .and_then(|master| master.process_group_leader());
                if current.and_then(|group| u32::try_from(group).ok()) != Some(group) {
                    return Err(TerminalBackendError::new(
                        "the foreground job changed during inspection",
                    ));
                }
                Ok(profile)
            })
        }
        #[cfg(not(unix))]
        {
            Ok(None)
        }
    }

    fn process_identity_is_live(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<bool, TerminalBackendError> {
        #[cfg(unix)]
        {
            process_snapshot(process.process_id)
                .map(|observed| process_generation_is_present(process, observed.as_ref()))
                .map_err(|error| TerminalBackendError::new(error.to_string()))
        }
        #[cfg(not(unix))]
        {
            let _ = process;
            Ok(false)
        }
    }

    fn process_group_identity_is_live(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<bool, TerminalBackendError> {
        #[cfg(unix)]
        {
            self.captured_process_group_receipt(process)?;
            captured_process_group_is_live(process)
        }
        #[cfg(not(unix))]
        {
            self.process_identity_is_live(process)
        }
    }

    fn process_cwd(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<Option<PathBuf>, TerminalBackendError> {
        if !self.process_identity_is_live(process)? {
            return Ok(None);
        }
        Ok(crate::proc_cwd::live_process_cwd(process.process_id))
    }
}

#[cfg(unix)]
fn process_generation_is_present(
    expected: &TerminalProcessIdentity,
    observed: Option<&ProcessGroupReceipt>,
) -> bool {
    observed.is_some_and(|observed| {
        observed.process_id == expected.process_id
            && observed.process_start == expected.process_start
    })
}

#[cfg(unix)]
fn process_group_generation_is_present(
    expected: &TerminalProcessIdentity,
    group_exists: bool,
    current_leader: Option<&ProcessGroupReceipt>,
) -> bool {
    group_exists
        && current_leader
            .is_none_or(|current| process_generation_is_present(expected, Some(current)))
}

/// Whether any process remains in a captured process-group generation.
///
/// A missing leader PID is not sufficient evidence: descendants can retain
/// the old numeric PGID after its leader exits. Conversely, if that PID has
/// been reused with a different start time, the old group generation must
/// already have disappeared before the numeric identifier could be reused.
///
/// # Errors
///
/// Returns an error when the supplied identity is malformed or the operating
/// system cannot prove the process-group state. Callers must treat an error as
/// potentially live.
pub fn captured_process_group_is_live(
    process: &TerminalProcessIdentity,
) -> Result<bool, TerminalBackendError> {
    #[cfg(unix)]
    {
        if process.process_id == 0
            || process.process_group_id != process.process_id
            || process.process_start.is_empty()
            || process.process_start.len() > 128
            || !process.process_start.is_ascii()
            || process.process_start.chars().any(char::is_control)
        {
            return Err(TerminalBackendError::new(
                "captured process-group identity was invalid",
            ));
        }
        #[cfg(target_os = "macos")]
        if process.terminal_device.as_deref().is_none_or(|terminal| {
            terminal.is_empty()
                || terminal.len() > 128
                || !terminal.is_ascii()
                || terminal.chars().any(char::is_control)
                || matches!(terminal, "??" | "-")
        }) {
            return Err(TerminalBackendError::new(
                "captured process-group terminal was invalid",
            ));
        }

        let group_exists = captured_process_group_is_present(process.process_group_id)
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        if !group_exists {
            return Ok(false);
        }
        let current_leader = process_snapshot(process.process_id)
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        if current_leader.is_none()
            && !captured_process_group_is_present(process.process_group_id)
                .map_err(|error| TerminalBackendError::new(error.to_string()))?
        {
            return Ok(false);
        }
        Ok(process_group_generation_is_present(
            process,
            true,
            current_leader.as_ref(),
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = process;
        Err(TerminalBackendError::new(
            "captured process-group liveness is unsupported",
        ))
    }
}

impl SystemTerminalControl {
    #[cfg(unix)]
    fn captured_process_group_receipt(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<ProcessGroupReceipt, TerminalBackendError> {
        if process.process_id == 0
            || process.process_group_id != process.process_id
            || process.process_start.is_empty()
        {
            return Err(TerminalBackendError::new(
                "foreground process-group identity was invalid",
            ));
        }
        #[cfg(target_os = "macos")]
        let identity = {
            let terminal_device = process.terminal_device.clone().ok_or_else(|| {
                TerminalBackendError::new("foreground process-group terminal was unavailable")
            })?;
            let identity = ProcessIdentity::RetainedPty { terminal_device };
            if identity != self.process_group_receipt.identity {
                return Err(TerminalBackendError::new(
                    "foreground process group did not belong to the retained terminal",
                ));
            }
            identity
        };
        #[cfg(not(target_os = "macos"))]
        let identity = ProcessIdentity::ProcessStart(process.process_start.clone());
        Ok(ProcessGroupReceipt {
            process_id: process.process_id,
            process_group_id: process.process_group_id,
            process_start: process.process_start.clone(),
            identity,
        })
    }

    #[cfg(unix)]
    fn foreground_process_receipt(&self) -> std::io::Result<Option<ProcessGroupReceipt>> {
        let process_id = {
            let master = lock(&self.master);
            let Some(master) = master.as_ref() else {
                return Ok(None);
            };
            master
                .process_group_leader()
                .and_then(|process_id| u32::try_from(process_id).ok())
        };
        let Some(process_id) = process_id else {
            return Ok(None);
        };
        let Some(receipt) = process_snapshot(process_id)? else {
            return Ok(None);
        };
        if receipt.process_group_id != process_id {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "the PTY foreground process was not its process-group leader",
            ));
        }
        #[cfg(target_os = "macos")]
        if receipt.identity != self.process_group_receipt.identity {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "the PTY foreground process did not belong to the retained terminal",
            ));
        }
        Ok(Some(receipt))
    }
}

#[cfg(unix)]
fn terminal_process_identity(receipt: &ProcessGroupReceipt) -> TerminalProcessIdentity {
    TerminalProcessIdentity {
        process_id: receipt.process_id,
        process_group_id: receipt.process_group_id,
        #[cfg(target_os = "macos")]
        terminal_device: match &receipt.identity {
            ProcessIdentity::RetainedPty { terminal_device } => Some(terminal_device.clone()),
        },
        #[cfg(not(target_os = "macos"))]
        terminal_device: None,
        process_start: receipt.process_start.clone(),
    }
}

/// Snapshot the calling process as an exact foreground-command candidate.
///
/// The manual Claude wrapper calls this after an interactive shell has placed
/// it in its own process group and before `exec`. The PID, process group,
/// start time, and controlling terminal therefore remain the same when the
/// wrapper becomes Claude.
///
/// # Errors
///
/// Returns an error unless the caller is a live process-group leader with a
/// usable controlling-terminal identity on platforms that expose one.
pub fn current_terminal_process_identity() -> Result<TerminalProcessIdentity, TerminalBackendError>
{
    #[cfg(unix)]
    {
        let process_id = std::process::id();
        let receipt = process_snapshot(process_id)
            .map_err(|error| TerminalBackendError::new(error.to_string()))?
            .ok_or_else(|| TerminalBackendError::new("the wrapper process disappeared"))?;
        let foreground_process_group = foreground_process_group_id(process_id)
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        validate_current_terminal_process(&receipt, foreground_process_group)
    }
    #[cfg(not(unix))]
    {
        Err(TerminalBackendError::new(
            "exact wrapper process identity is unsupported",
        ))
    }
}

#[cfg(unix)]
fn validate_current_terminal_process(
    receipt: &ProcessGroupReceipt,
    foreground_process_group: Option<u32>,
) -> Result<TerminalProcessIdentity, TerminalBackendError> {
    if receipt.process_group_id != receipt.process_id {
        return Err(TerminalBackendError::new(
            "the wrapper process did not own its process group",
        ));
    }
    if foreground_process_group != Some(receipt.process_group_id) {
        return Err(TerminalBackendError::new(
            "the wrapper process did not own its controlling terminal foreground",
        ));
    }
    #[cfg(target_os = "macos")]
    if matches!(
        &receipt.identity,
        ProcessIdentity::RetainedPty { terminal_device }
            if terminal_device == "??" || terminal_device == "-"
    ) {
        return Err(TerminalBackendError::new(
            "the wrapper process had no controlling terminal",
        ));
    }
    Ok(terminal_process_identity(receipt))
}

struct SystemTerminalWaiter {
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
}

impl TerminalProcessWaiter for SystemTerminalWaiter {
    fn wait(&mut self) -> Result<TerminalExit, TerminalBackendError> {
        let status = self
            .child
            .wait()
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        self.exited.store(true, Ordering::Release);
        let code = status
            .signal()
            .is_none()
            .then(|| i32::try_from(status.exit_code()).unwrap_or(i32::MAX));
        Ok(TerminalExit { code, signal: None })
    }
}

/// Native `portable-pty` backend running the user's login shell.
#[derive(Clone, Debug, Default)]
pub struct SystemTerminalPtyBackend {
    shell: Option<PathBuf>,
}

impl SystemTerminalPtyBackend {
    /// Pin an explicit login-shell executable. The path is canonicalized now,
    /// before any terminal operation is accepted.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing, non-file, or non-absolute executable.
    pub fn with_shell(shell: &Path) -> Result<Self, TerminalBackendError> {
        Ok(Self {
            shell: Some(validate_shell(shell)?),
        })
    }

    fn shell(&self) -> Result<PathBuf, TerminalBackendError> {
        if let Some(shell) = &self.shell {
            return Ok(shell.clone());
        }
        if let Some(shell) = std::env::var_os("SHELL") {
            if let Ok(shell) = validate_shell(Path::new(&shell)) {
                return Ok(shell);
            }
        }
        for fallback in [Path::new("/bin/zsh"), Path::new("/bin/sh")] {
            if let Ok(shell) = validate_shell(fallback) {
                return Ok(shell);
            }
        }
        Err(TerminalBackendError::new(
            "no safe absolute login shell is available",
        ))
    }
}

fn validate_shell(shell: &Path) -> Result<PathBuf, TerminalBackendError> {
    if !shell.is_absolute() {
        return Err(TerminalBackendError::new(
            "login shell path must be absolute",
        ));
    }
    let canonical =
        fs::canonicalize(shell).map_err(|error| TerminalBackendError::new(error.to_string()))?;
    let metadata =
        fs::metadata(&canonical).map_err(|error| TerminalBackendError::new(error.to_string()))?;
    if !metadata.is_file() {
        return Err(TerminalBackendError::new("login shell path is not a file"));
    }
    Ok(canonical)
}

impl TerminalPtyBackend for SystemTerminalPtyBackend {
    fn spawn(&self, spec: &TerminalSpawnSpec) -> Result<SpawnedTerminal, TerminalBackendError> {
        let size = PtySize {
            rows: spec.size.rows,
            cols: spec.size.columns,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;

        let (mut command, overrides) = match &spec.launch {
            ResolvedTerminalLaunch::LoginShell => {
                let mut command = CommandBuilder::new(self.shell()?);
                command.arg(OsString::from("-l"));
                (command, None)
            }
            ResolvedTerminalLaunch::LoginShellWithEnvironment { environment } => {
                let mut command = CommandBuilder::new(self.shell()?);
                command.arg(OsString::from("-l"));
                (command, Some(environment))
            }
            ResolvedTerminalLaunch::Program {
                executable,
                arguments,
                environment,
            } => {
                let mut command = CommandBuilder::new(validate_shell(executable)?);
                command.args(arguments);
                (command, Some(environment))
            }
        };
        command.env("PI_FORCE_HYPERLINKS", "1");
        command.env("FORCE_HYPERLINK", "1");
        if let Some(environment) = overrides {
            for (name, value) in environment {
                command.env(name, value);
            }
        }
        command.cwd(&spec.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "Interface");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        drop(pair.slave);
        #[cfg(unix)]
        let process_id = match require_process_id(child.process_id()) {
            Ok(process_id) => process_id,
            Err(error) => {
                let mut killer = child.clone_killer();
                let _ = killer.kill();
                return Err(TerminalBackendError::new(error.to_string()));
            }
        };
        #[cfg(unix)]
        let process_group_receipt =
            match capture_process_group_receipt(process_id, pair.master.process_group_leader()) {
                Ok(receipt) => receipt,
                Err(error) => {
                    // The child has not been reaped, so its PID cannot yet be
                    // recycled. Best-effort cleanup is safe before refusing a
                    // spawn whose process-group identity could not be pinned.
                    let _ = raw_signal_process_group(process_id, Signal::KILL);
                    let mut killer = child.clone_killer();
                    let _ = killer.kill();
                    return Err(TerminalBackendError::new(format!(
                        "could not capture terminal process-group identity: {error}"
                    )));
                }
            };

        let input = Arc::new(SystemTerminalInput {
            writer: Mutex::new(Some(writer)),
        });
        let exited = Arc::new(AtomicBool::new(false));
        let control = Arc::new(SystemTerminalControl {
            master: Mutex::new(Some(pair.master)),
            input: Arc::clone(&input),
            killer: Mutex::new(Some(child.clone_killer())),
            #[cfg(unix)]
            process_group_receipt,
            #[cfg(unix)]
            sleeping_groups: Mutex::new(Vec::new()),
            #[cfg(unix)]
            foreground_profile_cache: Mutex::default(),
            terminated: Mutex::new(false),
            exited: Arc::clone(&exited),
        });
        Ok(SpawnedTerminal {
            reader,
            input,
            control,
            waiter: Box::new(SystemTerminalWaiter { child, exited }),
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        Errno, ManagedProcessGroupStatus, ProcessGroupProbe, ProcessGroupReceipt, ProcessIdentity,
        Signal, classify_process_group, classify_process_group_probe, finalize_verified_teardown,
        process_generation_is_present, process_group_generation_is_present, require_process_id,
        terminal_process_identity, validate_current_terminal_process,
    };
    use crate::{TerminalBackendError, TerminalInput};

    struct RecordingInput {
        closed: AtomicBool,
    }

    impl TerminalInput for RecordingInput {
        fn write(&self, _bytes: &[u8]) -> Result<(), TerminalBackendError> {
            Ok(())
        }

        fn close(&self) {
            self.closed.store(true, Ordering::Release);
        }
    }

    fn receipt(identity: &str) -> ProcessGroupReceipt {
        ProcessGroupReceipt {
            process_id: 42,
            process_group_id: 42,
            process_start: "Mon Jan 1 00:00:00 2024".to_owned(),
            #[cfg(target_os = "macos")]
            identity: ProcessIdentity::RetainedPty {
                terminal_device: identity.to_owned(),
            },
            #[cfg(not(target_os = "macos"))]
            identity: ProcessIdentity::ProcessStart(identity.to_owned()),
        }
    }

    #[test]
    fn unsignalable_group_remains_present_for_authoritative_capture_liveness() {
        let probe = classify_process_group_probe(Err(Errno::PERM)).expect("recognized EPERM probe");

        assert_eq!(probe, ProcessGroupProbe::Unsignalable);
        assert!(probe.is_authoritatively_present());
        assert!(
            !probe.exists_for_teardown(),
            "teardown retains its intentional zombie-group policy"
        );
        assert_eq!(
            classify_process_group_probe(Err(Errno::SRCH)),
            Ok(ProcessGroupProbe::Absent)
        );
        assert_eq!(
            classify_process_group_probe(Ok(())),
            Ok(ProcessGroupProbe::Signalable)
        );
        assert_eq!(
            classify_process_group_probe(Err(Errno::INVAL)),
            Err(Errno::INVAL),
            "any other errno proves nothing about the group"
        );
    }

    #[test]
    fn recycled_process_group_leader_is_never_classified_as_owned() {
        let spawned = receipt("16/1");
        let recycled = receipt("16/2");

        assert_eq!(
            classify_process_group(&spawned, true, Some(&recycled)),
            ManagedProcessGroupStatus::Recycled
        );
        let expected = terminal_process_identity(&spawned);
        let mut recycled_generation = recycled;
        recycled_generation.process_start = "Mon Jan 1 00:00:01 2024".to_owned();
        assert!(!process_group_generation_is_present(
            &expected,
            true,
            Some(&recycled_generation),
        ));
    }

    #[test]
    fn leaderless_existing_group_still_belongs_to_spawned_terminal() {
        let spawned = receipt("16/1");

        assert_eq!(
            classify_process_group(&spawned, true, None),
            ManagedProcessGroupStatus::Owned
        );
        assert_eq!(
            classify_process_group(&spawned, false, None),
            ManagedProcessGroupStatus::Absent
        );
        let expected = terminal_process_identity(&spawned);
        assert!(process_group_generation_is_present(&expected, true, None));
        assert!(!process_group_generation_is_present(&expected, false, None));
    }

    #[test]
    fn background_process_group_cannot_claim_manual_capture() {
        let wrapper = receipt("16/1");

        assert!(validate_current_terminal_process(&wrapper, Some(41)).is_err());
        assert!(validate_current_terminal_process(&wrapper, None).is_err());
        assert_eq!(
            validate_current_terminal_process(&wrapper, Some(42))
                .expect("foreground wrapper identity")
                .process_id,
            42
        );
    }

    #[test]
    fn same_start_process_remains_live_when_terminal_identity_drifts() {
        let captured = receipt("16/1");
        let expected = terminal_process_identity(&captured);
        let mut observed = receipt("??");
        observed.process_group_id = 77;

        assert!(process_generation_is_present(&expected, Some(&observed)));

        observed.process_start = "Mon Jan 1 00:00:01 2024".to_owned();
        assert!(!process_generation_is_present(&expected, Some(&observed)));
        assert!(!process_generation_is_present(&expected, None));
    }

    #[test]
    fn missing_spawn_process_id_is_rejected() {
        let error = require_process_id(None).expect_err("missing PID must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn same_second_pid_recycle_on_another_pty_is_not_owned() {
        // `ps lstart` would report the same value for these two process
        // instances. Their retained kernel PTY identities cannot collide.
        let spawned = receipt("16/1");
        let same_second_recycled = receipt("16/2");

        assert_eq!(
            classify_process_group(&spawned, true, Some(&same_second_recycled)),
            ManagedProcessGroupStatus::Recycled
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zombie_only_process_group_is_absent_not_an_error() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;
        use std::time::{Duration, Instant};

        // A group leader that has exited but is not yet reaped: the state a
        // HUP'd shell is in until the waiter thread reaps it.
        let mut child = Command::new("/usr/bin/true")
            .process_group(0)
            .spawn()
            .expect("spawn group leader");
        let process_id = child.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let stat = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &process_id.to_string()])
                .output()
                .expect("ps");
            if String::from_utf8_lossy(&stat.stdout)
                .trim_start()
                .starts_with('Z')
            {
                break;
            }
            assert!(Instant::now() < deadline, "child never became a zombie");
            std::thread::sleep(super::PROCESS_GROUP_HUP_POLL);
        }

        assert!(
            !super::process_group_exists(process_id).expect("zombie-only group is not an error"),
            "zombie-only group must read as absent"
        );
        // The kernel answers EPERM here, which stays authoritative presence for
        // capture liveness while teardown finds nothing left to signal.
        assert_eq!(
            super::probe_process_group(process_id).expect("zombie-only probe"),
            ProcessGroupProbe::Unsignalable
        );
        assert!(super::captured_process_group_is_present(process_id).expect("zombie presence"));
        let mut zombie = receipt("??");
        zombie.process_id = process_id;
        zombie.process_group_id = process_id;
        assert!(
            !super::signal_managed_process_group(&zombie, Signal::KILL)
                .expect("zombie-only group is not a teardown error"),
            "no signal is sent to a zombie-only group"
        );
        child.wait().expect("reap");
    }

    fn spawn_group_leader(script: &str) -> (std::process::Child, u32) {
        use std::io::BufRead;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let mut child = Command::new("/bin/sh")
            .args(["-c", script])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn group leader");
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .expect("member pid");
        (child, line.trim().parse().expect("member pid"))
    }

    /// KILLs a test's process group only if the test panics, so a stopped or
    /// sleeping member never outlives a failure.
    struct KillGroupOnPanic(u32);

    impl Drop for KillGroupOnPanic {
        fn drop(&mut self) {
            if std::thread::panicking() {
                super::raw_signal_process_group(self.0, Signal::KILL);
            }
        }
    }

    fn eventually(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready() {
            assert!(std::time::Instant::now() < deadline, "{what}");
            std::thread::sleep(super::PROCESS_GROUP_HUP_POLL);
        }
    }

    fn stopped(process_id: u32) -> bool {
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &process_id.to_string()])
            .output()
            .expect("ps");
        String::from_utf8_lossy(&output.stdout)
            .trim_start()
            .starts_with('T')
    }

    #[test]
    fn live_and_reaped_process_groups_classify_as_signalable_then_absent() {
        let (mut leader, member) = spawn_group_leader("/bin/sleep 30 & echo $!; wait");
        let group = leader.id();
        let _cleanup = KillGroupOnPanic(group);
        assert_eq!(
            super::probe_process_group(group).expect("live probe"),
            ProcessGroupProbe::Signalable
        );
        assert!(super::process_group_exists(group).expect("live group"));
        assert!(super::captured_process_group_is_present(group).expect("live group"));
        assert!(super::process_exists(member).expect("live member"));

        leader.kill().expect("kill leader");
        leader.wait().expect("reap leader");
        // The leader is reaped but its forked member keeps the group alive.
        assert!(super::process_group_exists(group).expect("leaderless group"));
        let mut leaderless = receipt("??");
        leaderless.process_id = group;
        leaderless.process_group_id = group;
        assert!(
            super::signal_managed_process_group(&leaderless, Signal::KILL).expect("kill group")
        );
        eventually("member survived its group KILL", || {
            !super::process_exists(member).expect("member probe")
        });
        // ESRCH: no process carries either id any more.
        assert_eq!(
            super::probe_process_group(group).expect("absent group is not an error"),
            ProcessGroupProbe::Absent
        );
        assert!(!super::process_group_exists(group).expect("absent group"));
        assert!(!super::captured_process_group_is_present(group).expect("absent group"));
        assert!(!super::process_exists(group).expect("absent process"));
    }

    #[test]
    fn managed_group_signals_reach_every_member_of_the_group() {
        use std::os::unix::process::ExitStatusExt;

        let (mut leader, member) = spawn_group_leader("/bin/sleep 30 & echo $!; wait");
        let _cleanup = KillGroupOnPanic(leader.id());
        let receipt = super::process_snapshot(leader.id())
            .expect("leader snapshot")
            .expect("live leader");
        assert!(super::signal_managed_process_group(&receipt, Signal::STOP).expect("STOP"));
        eventually("group did not stop", || {
            stopped(leader.id()) && stopped(member)
        });
        assert!(super::signal_managed_process_group(&receipt, Signal::CONT).expect("CONT"));
        eventually("group did not continue", || {
            !stopped(leader.id()) && !stopped(member)
        });
        assert!(super::signal_managed_process_group(&receipt, Signal::HUP).expect("HUP"));
        assert_eq!(
            leader.wait().expect("reap leader").signal(),
            Some(libc::SIGHUP)
        );
        eventually("member survived its group HUP", || {
            !super::process_exists(member).expect("member probe")
        });
    }

    #[test]
    fn spawn_identity_accepts_a_job_that_already_took_the_foreground() {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "set -m; /bin/sleep 30; :"]);
        let mut child = pair.slave.spawn_command(command).expect("spawn shell");
        drop(pair.slave);
        let process_id = child.process_id().expect("shell pid");
        let _cleanup = KillGroupOnPanic(process_id);
        // Hold the identity check until the job owns the foreground: the
        // order a loaded machine can produce between spawn and the check.
        let job_group = || {
            pair.master
                .process_group_leader()
                .filter(|group| u32::try_from(*group).ok() != Some(process_id))
        };
        eventually("the job never took the foreground", || {
            job_group().is_some()
        });
        let job = job_group().expect("job group");
        let _job_cleanup = KillGroupOnPanic(u32::try_from(job).expect("job group id"));

        let receipt = super::capture_process_group_receipt(process_id, Some(job))
            .expect("a job in the shell's own session keeps the spawn");
        assert_eq!(receipt.process_id, process_id);
        assert_eq!(receipt.process_group_id, process_id);

        // A foreground group from another session still refuses the spawn.
        let outsider = rustix::process::getpgrp().as_raw_pid();
        let error = super::capture_process_group_receipt(process_id, Some(outsider))
            .expect_err("a foreign foreground group proves nothing");
        assert!(
            error
                .to_string()
                .contains("did not own its controlling PTY"),
            "{error}"
        );

        super::raw_signal_process_group(process_id, Signal::KILL);
        super::raw_signal_process_group(u32::try_from(job).expect("job group id"), Signal::KILL);
        // Nobody reads the PTY: an exiting session leader with output still
        // queued (the shell's "Killed" notice) waits for it to drain until
        // the master closes.
        drop(pair.master);
        child.wait().expect("reap shell");
    }

    #[test]
    fn an_unsignalable_process_is_an_error_not_absence() {
        if interface_common::unix::effective_uid().expect("uid") == 0 {
            return;
        }
        // PID 1 belongs to root, so `kill(1, 0)` answers EPERM.
        assert!(super::process_exists(1).is_err());
    }

    #[test]
    fn ids_no_process_can_carry_are_errors_and_never_signalled() {
        // `/bin/kill` read `-0` as the caller's own group and `-2147483648` as
        // `i32::MIN`; neither ever named a terminal's group.
        for id in [0, 1 << 31, u32::MAX] {
            assert!(super::probe_process_group(id).is_err());
            assert!(super::process_exists(id).is_err());
            assert!(!super::raw_signal_process_group(id, Signal::CONT));
        }
    }

    #[test]
    fn failed_teardown_retains_identity_anchor_for_retry() {
        let input = RecordingInput {
            closed: AtomicBool::new(false),
        };
        let master = Mutex::new(Some("retained master PTY"));
        let killer = Mutex::new(Some("retained child handle"));
        let mut terminated = false;

        let error = finalize_verified_teardown(
            Err(TerminalBackendError::new("transient verification failure")),
            &input,
            &master,
            &killer,
            &mut terminated,
        )
        .expect_err("failed verification must remain retryable");
        assert_eq!(error.to_string(), "transient verification failure");
        assert!(!input.closed.load(Ordering::Acquire));
        assert!(super::lock(&master).is_some());
        assert!(super::lock(&killer).is_some());
        assert!(!terminated);

        finalize_verified_teardown(Ok(()), &input, &master, &killer, &mut terminated)
            .expect("verified retry releases anchors");
        assert!(input.closed.load(Ordering::Acquire));
        assert!(super::lock(&master).is_none());
        assert!(super::lock(&killer).is_none());
        assert!(terminated);
    }
}
