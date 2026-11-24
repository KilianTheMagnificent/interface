use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use interface_contracts::canonical_timestamp;
use interface_contracts::updater::{UpdatePhase, UpdateSource, UpdateState};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::task::JoinHandle;

use crate::{
    CHANNEL_STATE_NAME, LocalUpdateFeed, MirrorError, MirrorEvent, MirrorOutcome, MirrorPlan,
    NormalizedFeed, ReleaseChannel, RemoteFetcher, RetainedVerifiedArchive, UpdateMachine,
    UreqFetcher, VerificationError, prepare_mirror, read_mac_app_requirement, refresh_mirror,
    verify_and_retain_mac_update_artifact, verify_mac_update_artifact,
};

pub const DEFAULT_UPDATE_STARTUP_DELAY_MS: u64 = 15_000;
pub const DEFAULT_UPDATE_INTERVAL_MS: u64 = 15 * 60 * 1_000;
const MAX_NATIVE_PATH_BYTES: usize = 16 * 1024;
/// How long after startup the public feed is first asked, and how often
/// after that. A release reaches every running app within the interval.
pub const CHANNEL_FIRST_CHECK: Duration = Duration::from_secs(45);
pub const CHANNEL_CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
/// After a check that could not reach the feed or was cut off mid-download.
const CHANNEL_RETRY_FIRST: Duration = Duration::from_secs(10 * 60);
const FEED_DIRECTORY_NAME: &str = "update-feed";

/// What one check of the public channel came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChannelCheck {
    /// This app does not follow a public channel.
    NotFollowing,
    /// Another check is already running.
    Busy,
    UpToDate,
    /// A newer release is in the local feed; the installer takes it from there.
    Staged(String),
    Failed(MirrorError),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeUpdaterConfigurationInput {
    pub is_packaged: bool,
    pub current_version: String,
    pub app_bundle_path: Option<String>,
}

impl NativeUpdaterConfigurationInput {
    #[must_use]
    pub fn app_bundle(&self) -> Option<PathBuf> {
        self.app_bundle_path
            .as_deref()
            .filter(|path| !path.is_empty() && path.len() <= MAX_NATIVE_PATH_BYTES)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeUpdaterConfiguration {
    pub enabled: bool,
    pub feed_url: Option<String>,
    pub startup_delay_ms: u64,
    pub interval_ms: u64,
}

impl NativeUpdaterConfiguration {
    fn disabled() -> Self {
        Self {
            enabled: false,
            feed_url: None,
            startup_delay_ms: DEFAULT_UPDATE_STARTUP_DELAY_MS,
            interval_ms: DEFAULT_UPDATE_INTERVAL_MS,
        }
    }

    fn enabled(feed_url: String) -> Self {
        Self {
            enabled: true,
            feed_url: Some(feed_url),
            startup_delay_ms: DEFAULT_UPDATE_STARTUP_DELAY_MS,
            interval_ms: DEFAULT_UPDATE_INTERVAL_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum NativeCheckOutcome {
    Available,
    NotAvailable,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NativeUpdaterEvent {
    Checking {},
    Available {
        version: Option<String>,
    },
    NotAvailable {},
    Progress {
        percent: Option<f64>,
    },
    Downloaded {
        version: Option<String>,
        downloaded_file: Option<String>,
    },
    Error {},
    CheckResult {
        outcome: NativeCheckOutcome,
        version: Option<String>,
    },
}

pub trait UpdaterClock: Send + Sync {
    fn now_iso(&self) -> String;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemUpdaterClock;

impl UpdaterClock for SystemUpdaterClock {
    fn now_iso(&self) -> String {
        canonical_timestamp(Utc::now())
    }
}

#[async_trait]
pub trait UpdaterVerifier: Send + Sync {
    /// Returns the running app's enforceable signing requirement when this
    /// platform requires one, or `None` where artifact verification does not
    /// apply. A failure disables updates rather than weakening the gate.
    async fn running_requirement(
        &self,
        packaged: bool,
        app_bundle: Option<&Path>,
    ) -> Result<Option<String>, VerificationError>;

    async fn verify_artifact(
        &self,
        artifact: &Path,
        requirement: &str,
    ) -> Result<(), VerificationError>;

    async fn verify_and_retain_artifact(
        &self,
        artifact: &Path,
        requirement: &str,
        _current_version: &str,
        _expected_version: Option<&str>,
        _staging_root: &Path,
    ) -> Result<Option<RetainedVerifiedArchive>, VerificationError> {
        self.verify_artifact(artifact, requirement)
            .await
            .map(|()| None)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemUpdaterVerifier;

#[async_trait]
impl UpdaterVerifier for SystemUpdaterVerifier {
    async fn running_requirement(
        &self,
        packaged: bool,
        app_bundle: Option<&Path>,
    ) -> Result<Option<String>, VerificationError> {
        if !packaged {
            let _ = app_bundle;
            return Ok(None);
        }
        #[cfg(target_os = "macos")]
        {
            let app_bundle = app_bundle.ok_or(VerificationError::InvalidArchivePath)?;
            read_mac_app_requirement(app_bundle).await.map(Some)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = app_bundle;
            Err(VerificationError::MissingRequirement)
        }
    }

    async fn verify_artifact(
        &self,
        artifact: &Path,
        requirement: &str,
    ) -> Result<(), VerificationError> {
        verify_mac_update_artifact(artifact, requirement).await
    }

    async fn verify_and_retain_artifact(
        &self,
        artifact: &Path,
        requirement: &str,
        current_version: &str,
        expected_version: Option<&str>,
        staging_root: &Path,
    ) -> Result<Option<RetainedVerifiedArchive>, VerificationError> {
        verify_and_retain_mac_update_artifact(
            artifact,
            requirement,
            current_version,
            expected_version,
            Some(staging_root),
        )
        .await
        .map(Some)
    }
}

struct UpdaterServiceState {
    machine: UpdateMachine,
    requirement: Option<String>,
    local_feed: Option<LocalUpdateFeed>,
    retained_archive: Option<RetainedVerifiedArchive>,
    /// The public channel is what this app updates from. The installer still
    /// reads only the loopback feed; the channel fills it.
    follows_channel: bool,
    /// A check of the public channel is running and owns the visible state.
    channel_busy: bool,
}

/// Rust owner for the legacy updater policy and renderer-visible state.
///
/// Electron remains a narrow native-effects port. It receives the private feed
/// URL and reports native events, while this service owns enablement, sticky
/// ready semantics, signature verification and every public `UpdateState`.
pub struct UpdaterService {
    data_root: PathBuf,
    remote_feed_url: Option<String>,
    verifier: Arc<dyn UpdaterVerifier>,
    clock: Arc<dyn UpdaterClock>,
    state: Mutex<UpdaterServiceState>,
    subscribers: StdMutex<Vec<mpsc::UnboundedSender<UpdateState>>>,
    channel: Option<ReleaseChannel>,
    fetcher: Arc<dyn RemoteFetcher>,
    /// One channel check at a time.
    channel_gate: Mutex<()>,
    /// Set by "Check for updates"; the schedule runs that check at once.
    asked: AtomicBool,
    wake: Notify,
    /// Signalled once `configure` has turned the channel on.
    following: Notify,
    scheduled: AtomicBool,
    stop: Arc<AtomicBool>,
}

impl UpdaterService {
    #[must_use]
    pub fn new(
        data_root: impl Into<PathBuf>,
        remote_feed_url: Option<String>,
        verifier: Arc<dyn UpdaterVerifier>,
        clock: Arc<dyn UpdaterClock>,
    ) -> Self {
        let machine = UpdateMachine::new("unknown", false);
        Self {
            data_root: data_root.into(),
            remote_feed_url: remote_feed_url.filter(|url| !url.trim().is_empty()),
            verifier,
            clock,
            state: Mutex::new(UpdaterServiceState {
                machine,
                requirement: None,
                local_feed: None,
                retained_archive: None,
                follows_channel: false,
                channel_busy: false,
            }),
            subscribers: StdMutex::new(Vec::new()),
            channel: None,
            fetcher: Arc::new(UreqFetcher::new()),
            channel_gate: Mutex::new(()),
            asked: AtomicBool::new(false),
            wake: Notify::new(),
            following: Notify::new(),
            scheduled: AtomicBool::new(false),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Follows a public release channel: its signed manifest decides what
    /// reaches the local feed. `None` leaves the app on the local feed alone.
    #[must_use]
    pub fn with_release_channel(
        mut self,
        channel: Option<ReleaseChannel>,
        fetcher: Arc<dyn RemoteFetcher>,
    ) -> Self {
        self.channel = channel;
        self.fetcher = fetcher;
        self
    }

    #[must_use]
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<UpdateState> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(sender);
        receiver
    }

    pub async fn verified_archive_path(&self) -> Option<PathBuf> {
        let state = self.state.lock().await;
        state
            .retained_archive
            .as_ref()
            .map(|retained| retained.archive_path().to_path_buf())
    }

    pub async fn configure(
        &self,
        input: NativeUpdaterConfigurationInput,
    ) -> NativeUpdaterConfiguration {
        let previous_feed = {
            let mut state = self.state.lock().await;
            state.requirement = None;
            state.retained_archive = None;
            state.follows_channel = false;
            state.channel_busy = false;
            state.local_feed.take()
        };
        if let Some(feed) = previous_feed {
            let _ = feed.close().await;
        }

        let mut machine = UpdateMachine::new(&input.current_version, input.is_packaged);
        if !input.is_packaged {
            return self.disable(machine, None).await;
        }

        let Ok(requirement) = self
            .verifier
            .running_requirement(true, input.app_bundle().as_deref())
            .await
        else {
            return self.disable(machine, None).await;
        };

        if let Some(remote) = self.remote_feed_url.as_deref() {
            let Ok(feed) = NormalizedFeed::new(remote.trim(), UpdateSource::Remote) else {
                return self.disable(machine, requirement).await;
            };
            let feed_url = feed.as_url().to_string();
            machine.set_feed(Some(feed));
            self.replace(machine, requirement, None).await;
            return NativeUpdaterConfiguration::enabled(feed_url);
        }

        let feed_dir = self.data_root.join(FEED_DIRECTORY_NAME);
        let Ok(local_feed) = LocalUpdateFeed::start(&feed_dir).await else {
            return self.disable(machine, requirement).await;
        };
        let feed_url = local_feed.url();
        let Ok(feed) = NormalizedFeed::new(&feed_url, UpdateSource::Local) else {
            let _ = local_feed.close().await;
            return self.disable(machine, requirement).await;
        };
        let Some(channel) = self.channel.as_ref() else {
            machine.set_feed(Some(feed));
            self.replace(machine, requirement, Some(local_feed)).await;
            return NativeUpdaterConfiguration::enabled(feed_url);
        };
        let state_path = self.data_root.join(CHANNEL_STATE_NAME);
        let current_version = input.current_version.clone();
        let _ = tokio::task::spawn_blocking(move || {
            prepare_mirror(&feed_dir, &state_path, &current_version);
        })
        .await;
        machine.set_feed(Some(NormalizedFeed::channel(channel.feeds()[0].clone())));
        self.replace(machine, requirement, Some(local_feed)).await;
        self.state.lock().await.follows_channel = true;
        self.following.notify_one();
        NativeUpdaterConfiguration::enabled(feed_url)
    }

    /// Installs `machine` with no feed (and no local feed server) and answers
    /// the disabled configuration: every startup branch that cannot update.
    async fn disable(
        &self,
        mut machine: UpdateMachine,
        requirement: Option<String>,
    ) -> NativeUpdaterConfiguration {
        machine.set_feed(None);
        self.replace(machine, requirement, None).await;
        NativeUpdaterConfiguration::disabled()
    }

    pub async fn state(&self) -> UpdateState {
        let mut state = self.state.lock().await;
        self.retire_stopped_local_feed(&mut state);
        state.machine.state()
    }

    pub async fn begin_check(&self) -> UpdateState {
        let follows_channel = {
            let mut state = self.state.lock().await;
            self.retire_stopped_local_feed(&mut state);
            if !state.machine.state().can_check {
                return state.machine.state();
            }
            state.machine.checking();
            self.publish(&state.machine);
            state.follows_channel
        };
        if follows_channel {
            if self.scheduled.load(Ordering::SeqCst) {
                // The schedule runs it, so this call returns with "checking"
                // and the download never holds the caller.
                self.asked.store(true, Ordering::SeqCst);
                self.wake.notify_one();
            } else {
                self.check_channel(true).await;
            }
        }
        self.state().await
    }

    /// Asks the public feed for a newer release and, when it has one, brings
    /// it into the local feed, where the installer finds it.
    ///
    /// `asked` is a check someone is watching: it shows "checking" and says
    /// when the feed could not be reached. A check on the schedule stays
    /// quiet unless it finds a release or something that fails its checks.
    pub async fn check_channel(&self, asked: bool) -> ChannelCheck {
        let Ok(_gate) = self.channel_gate.try_lock() else {
            return ChannelCheck::Busy;
        };
        let Some(plan) = self.channel_plan().await else {
            return ChannelCheck::NotFollowing;
        };
        {
            let mut state = self.state.lock().await;
            state.channel_busy = true;
            if asked {
                state.machine.checking();
                self.publish(&state.machine);
            }
        }
        let (events, mut heard) = mpsc::unbounded_channel();
        let fetcher = Arc::clone(&self.fetcher);
        let stop = Arc::clone(&self.stop);
        let mut work = tokio::task::spawn_blocking(move || {
            refresh_mirror(
                &plan,
                fetcher.as_ref(),
                &mut |event| {
                    let _ = events.send(event);
                },
                &stop,
            )
        });
        let mut found = false;
        let outcome = loop {
            tokio::select! {
                biased;
                Some(event) = heard.recv() => {
                    found = true;
                    self.channel_event(event).await;
                }
                finished = &mut work => break finished.unwrap_or(Err(MirrorError::Storage)),
            }
        };
        while let Ok(event) = heard.try_recv() {
            found = true;
            self.channel_event(event).await;
        }
        self.channel_finished(outcome, asked, found).await
    }

    /// Runs the channel's checks for as long as the app does: once shortly
    /// after startup, then on the interval, sooner after a failed attempt,
    /// and at once when "Check for updates" asks.
    #[must_use]
    pub fn spawn_channel_schedule(self: &Arc<Self>) -> JoinHandle<()> {
        let service = Arc::clone(self);
        service.scheduled.store(true, Ordering::SeqCst);
        tokio::spawn(async move {
            // An app that follows no channel (a development build, a build
            // with none compiled in) never wakes this task at all.
            service.following.notified().await;
            let mut wait = CHANNEL_FIRST_CHECK;
            let mut retry = CHANNEL_RETRY_FIRST;
            loop {
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = service.wake.notified() => {}
                }
                let asked = service.asked.swap(false, Ordering::SeqCst);
                wait = match service.check_channel(asked).await {
                    ChannelCheck::Failed(MirrorError::Network | MirrorError::Busy) => {
                        let soon = retry;
                        retry = (retry * 2).min(CHANNEL_CHECK_INTERVAL);
                        soon
                    }
                    ChannelCheck::Failed(MirrorError::Stopped) | ChannelCheck::NotFollowing => {
                        break;
                    }
                    _ => {
                        retry = CHANNEL_RETRY_FIRST;
                        CHANNEL_CHECK_INTERVAL
                    }
                };
            }
        })
    }

    async fn channel_plan(&self) -> Option<MirrorPlan> {
        let state = self.state.lock().await;
        if !state.follows_channel {
            return None;
        }
        Some(MirrorPlan {
            channel: self.channel.clone()?,
            feed_dir: self.data_root.join(FEED_DIRECTORY_NAME),
            state_path: self.data_root.join(CHANNEL_STATE_NAME),
            current_version: state.machine.state().current_version,
        })
    }

    async fn channel_event(&self, event: MirrorEvent) {
        let now = self.clock.now_iso();
        let mut state = self.state.lock().await;
        match event {
            MirrorEvent::Available(version) => state.machine.available(Some(&version), &now),
            MirrorEvent::Progress(percent) => state.machine.progress(Some(percent)),
        }
        self.publish(&state.machine);
    }

    async fn channel_finished(
        &self,
        outcome: Result<MirrorOutcome, MirrorError>,
        asked: bool,
        found: bool,
    ) -> ChannelCheck {
        let now = self.clock.now_iso();
        let mut state = self.state.lock().await;
        state.channel_busy = false;
        // A release that is ready to install stays ready whatever a later
        // check runs into: the feed going quiet or bad takes nothing back.
        let ready = state.machine.state().phase == UpdatePhase::Ready;
        let check = match outcome {
            Ok(MirrorOutcome::UpToDate | MirrorOutcome::NothingPublished) => {
                state.machine.not_available(&now);
                ChannelCheck::UpToDate
            }
            Ok(MirrorOutcome::Staged(version)) => {
                // Stays "downloading" until the installer has checked the
                // archive's signature and reports it ready.
                state.machine.progress(Some(100.0));
                ChannelCheck::Staged(version)
            }
            Ok(MirrorOutcome::AlreadyStaged(version)) => {
                if state.machine.state().phase == UpdatePhase::Checking {
                    state.machine.available(Some(&version), &now);
                }
                ChannelCheck::Staged(version)
            }
            Err(error @ MirrorError::Integrity) => {
                if !ready {
                    state.machine.refuse_unverified(&now);
                }
                ChannelCheck::Failed(error)
            }
            Err(error) => {
                // Offline on the schedule is not news. A check someone asked
                // for, or a download that broke off, is.
                if !ready && (asked || found) {
                    state.machine.error(&now);
                }
                ChannelCheck::Failed(error)
            }
        };
        self.publish(&state.machine);
        check
    }

    pub async fn request_install(&self) -> bool {
        let mut state = self.state.lock().await;
        if state.machine.state().phase != UpdatePhase::Ready || !state.machine.state().can_restart {
            return false;
        }
        if let Some(retained) = state.retained_archive.as_ref()
            && retained.reverify().is_err()
        {
            state.retained_archive = None;
            let now = self.clock.now_iso();
            state.machine.refuse_install(&now);
            self.publish(&state.machine);
            return false;
        }
        let accepted = state.machine.request_install();
        if accepted {
            self.publish(&state.machine);
        }
        accepted
    }

    pub async fn native_event(&self, event: NativeUpdaterEvent) -> UpdateState {
        let now = self.clock.now_iso();
        match event {
            NativeUpdaterEvent::Downloaded {
                version,
                downloaded_file,
            } => {
                self.downloaded(version.as_deref(), downloaded_file.as_deref(), &now)
                    .await;
            }
            event => {
                let mut state = self.state.lock().await;
                if state.follows_channel && !Self::installer_event_counts(&state, &event) {
                    return state.machine.state();
                }
                match event {
                    NativeUpdaterEvent::Checking {} => state.machine.checking(),
                    NativeUpdaterEvent::Available { version } => {
                        state.machine.available(version.as_deref(), &now);
                    }
                    NativeUpdaterEvent::NotAvailable {} => state.machine.not_available(&now),
                    NativeUpdaterEvent::Progress { percent } => state.machine.progress(percent),
                    NativeUpdaterEvent::Error {} => state.machine.error(&now),
                    NativeUpdaterEvent::CheckResult { outcome, version } => match outcome {
                        NativeCheckOutcome::Available
                            if state.machine.state().phase == UpdatePhase::Checking =>
                        {
                            state.machine.available(version.as_deref(), &now);
                        }
                        NativeCheckOutcome::NotAvailable
                            if state.machine.state().phase == UpdatePhase::Checking =>
                        {
                            state.machine.not_available(&now);
                        }
                        NativeCheckOutcome::Error
                            if state.machine.state().phase != UpdatePhase::Ready =>
                        {
                            state.machine.error(&now);
                        }
                        _ => {}
                    },
                    NativeUpdaterEvent::Downloaded { .. } => unreachable!(),
                }
                self.publish(&state.machine);
            }
        }
        self.state().await
    }

    /// While the public channel is followed, the installer only ever sees
    /// the loopback feed, so most of what it reports is about that folder and
    /// not about whether an update exists. Its "checking", "nothing new" and
    /// download progress are dropped; the channel's own check says those. It
    /// is heard when it finds a staged release, and when it fails on one it
    /// is fetching or installing.
    fn installer_event_counts(state: &UpdaterServiceState, event: &NativeUpdaterEvent) -> bool {
        if state.channel_busy {
            return false;
        }
        let phase = state.machine.state().phase;
        let handling_a_release = matches!(
            phase,
            UpdatePhase::Available | UpdatePhase::Downloading | UpdatePhase::Installing
        );
        match event {
            NativeUpdaterEvent::Available { .. } => phase != UpdatePhase::Downloading,
            NativeUpdaterEvent::Downloaded { .. }
            | NativeUpdaterEvent::CheckResult {
                outcome: NativeCheckOutcome::Available,
                ..
            } => true,
            NativeUpdaterEvent::Error {}
            | NativeUpdaterEvent::CheckResult {
                outcome: NativeCheckOutcome::Error,
                ..
            } => handling_a_release,
            NativeUpdaterEvent::Checking {}
            | NativeUpdaterEvent::NotAvailable {}
            | NativeUpdaterEvent::Progress { .. }
            | NativeUpdaterEvent::CheckResult {
                outcome: NativeCheckOutcome::NotAvailable,
                ..
            } => false,
        }
    }

    pub async fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let feed = self.state.lock().await.local_feed.take();
        if let Some(feed) = feed {
            let _ = feed.close().await;
        }
    }

    async fn downloaded(&self, version: Option<&str>, file: Option<&str>, now: &str) {
        let (ticket, requirement, artifact, current_version, expected_version) = {
            let mut state = self.state.lock().await;
            let requirement = state.requirement.clone();
            if requirement.is_none() {
                state.retained_archive = None;
                state.machine.downloaded_without_verification(version, now);
                self.publish(&state.machine);
                return;
            }
            let artifact = file
                .filter(|path| !path.is_empty() && path.len() <= MAX_NATIVE_PATH_BYTES)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute());
            let ticket =
                state
                    .machine
                    .downloaded_requiring_verification(version, artifact.is_some(), now);
            let current_version = state.machine.state().current_version;
            let expected_version = state.machine.state().available_version;
            self.publish(&state.machine);
            (
                ticket,
                requirement,
                artifact,
                current_version,
                expected_version,
            )
        };
        let (Some(ticket), Some(requirement), Some(artifact)) = (ticket, requirement, artifact)
        else {
            return;
        };
        let staging_root = self.data_root.join("verified-updates");
        let verified = self
            .verifier
            .verify_and_retain_artifact(
                &artifact,
                &requirement,
                &current_version,
                expected_version.as_deref(),
                &staging_root,
            )
            .await;
        let mut state = self.state.lock().await;
        if let Ok(retained) = verified {
            if state.machine.state().phase == UpdatePhase::Downloading {
                state.retained_archive = retained;
            }
            state.machine.complete_verification(ticket, true, now);
        } else {
            state.retained_archive = None;
            state.machine.complete_verification(ticket, false, now);
        }
        self.publish(&state.machine);
    }

    /// Drops a local feed whose loopback listener tripped off, so the public
    /// state reports no update source instead of an enabled one that can no
    /// longer serve. A finished download keeps its already-verified artifact:
    /// the restart action does not need the feed, and the next transition out
    /// of Ready retires it.
    fn retire_stopped_local_feed(&self, state: &mut UpdaterServiceState) {
        if state
            .local_feed
            .as_ref()
            .is_none_or(LocalUpdateFeed::is_serving)
            || matches!(
                state.machine.state().phase,
                UpdatePhase::Ready | UpdatePhase::Installing
            )
        {
            return;
        }
        state.local_feed = None;
        state.retained_archive = None;
        state.machine.set_feed(None);
        self.publish(&state.machine);
    }

    async fn replace(
        &self,
        machine: UpdateMachine,
        requirement: Option<String>,
        local_feed: Option<LocalUpdateFeed>,
    ) {
        let mut state = self.state.lock().await;
        state.machine = machine;
        state.requirement = requirement;
        state.local_feed = local_feed;
        state.retained_archive = None;
        self.publish(&state.machine);
    }

    fn publish(&self, machine: &UpdateMachine) {
        let state = machine.state();
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|subscriber| subscriber.send(state.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    struct FixedClock;

    impl UpdaterClock for FixedClock {
        fn now_iso(&self) -> String {
            "2026-08-27T12:00:00.000Z".to_owned()
        }
    }

    struct FakeVerifier {
        requirement: Result<Option<String>, VerificationError>,
        accepted: bool,
        artifacts: StdMutex<Vec<PathBuf>>,
        verifies: AtomicUsize,
    }

    #[async_trait]
    impl UpdaterVerifier for FakeVerifier {
        async fn running_requirement(
            &self,
            _packaged: bool,
            _app_bundle: Option<&Path>,
        ) -> Result<Option<String>, VerificationError> {
            self.requirement.clone()
        }

        async fn verify_artifact(
            &self,
            artifact: &Path,
            _requirement: &str,
        ) -> Result<(), VerificationError> {
            self.verifies.fetch_add(1, Ordering::SeqCst);
            self.artifacts
                .lock()
                .expect("artifacts")
                .push(artifact.to_owned());
            self.accepted
                .then_some(())
                .ok_or(VerificationError::CommandRejected)
        }
    }

    fn verifier(requirement: Option<&str>, accepted: bool) -> Arc<FakeVerifier> {
        Arc::new(FakeVerifier {
            requirement: Ok(requirement.map(str::to_owned)),
            accepted,
            artifacts: StdMutex::new(Vec::new()),
            verifies: AtomicUsize::new(0),
        })
    }

    fn service(root: &Path, remote: Option<&str>, verifier: Arc<FakeVerifier>) -> UpdaterService {
        UpdaterService::new(
            root,
            remote.map(str::to_owned),
            verifier,
            Arc::new(FixedClock),
        )
    }

    fn input(packaged: bool) -> NativeUpdaterConfigurationInput {
        NativeUpdaterConfigurationInput {
            is_packaged: packaged,
            current_version: "0.2.0".to_owned(),
            app_bundle_path: Some("/Applications/Interface.app".to_owned()),
        }
    }

    #[tokio::test]
    async fn development_is_inert_and_does_not_start_a_feed() {
        let root = tempfile::tempdir().expect("root");
        let service = service(root.path(), None, verifier(None, true));
        assert_eq!(
            service.configure(input(false)).await,
            NativeUpdaterConfiguration::disabled()
        );
        let state = service.begin_check().await;
        assert_eq!(state.phase, UpdatePhase::Disabled);
        assert_eq!(state.source, UpdateSource::None);
    }

    #[tokio::test]
    async fn remote_feed_drives_exact_public_lifecycle_and_sticky_ready() {
        let root = tempfile::tempdir().expect("root");
        let service = service(
            root.path(),
            Some("https://updates.example.test/interface/"),
            verifier(None, true),
        );
        let configured = service.configure(input(true)).await;
        assert!(configured.enabled);
        assert_eq!(configured.interval_ms, 15 * 60 * 1_000);
        assert_eq!(
            configured.feed_url.as_deref(),
            Some("https://updates.example.test/interface/")
        );
        assert_eq!(service.begin_check().await.phase, UpdatePhase::Checking);
        assert_eq!(
            service
                .native_event(NativeUpdaterEvent::Available {
                    version: Some("0.3.0".to_owned()),
                })
                .await
                .phase,
            UpdatePhase::Available
        );
        service
            .native_event(NativeUpdaterEvent::Progress {
                percent: Some(42.5),
            })
            .await;
        let ready = service
            .native_event(NativeUpdaterEvent::Downloaded {
                version: Some("0.3.0".to_owned()),
                downloaded_file: None,
            })
            .await;
        assert_eq!(ready.phase, UpdatePhase::Ready);
        assert!(ready.can_check);
        assert!(ready.can_restart);
        service.begin_check().await;
        let sticky = service
            .native_event(NativeUpdaterEvent::CheckResult {
                outcome: NativeCheckOutcome::NotAvailable,
                version: None,
            })
            .await;
        assert_eq!(sticky.phase, UpdatePhase::Ready);
        assert!(service.request_install().await);
        assert_eq!(service.state().await.phase, UpdatePhase::Installing);
    }

    #[tokio::test]
    async fn signature_gate_accepts_only_the_exact_downloaded_artifact() {
        let root = tempfile::tempdir().expect("root");
        let verifier = verifier(Some("anchor apple generic"), true);
        let service = service(
            root.path(),
            Some("https://updates.example.test/interface/"),
            verifier.clone(),
        );
        service.configure(input(true)).await;
        let state = service
            .native_event(NativeUpdaterEvent::Downloaded {
                version: Some("0.3.0".to_owned()),
                downloaded_file: Some("/private/tmp/Interface.zip".to_owned()),
            })
            .await;
        assert_eq!(state.phase, UpdatePhase::Ready);
        assert_eq!(verifier.verifies.load(Ordering::SeqCst), 1);
        assert_eq!(
            verifier.artifacts.lock().expect("artifacts").as_slice(),
            [PathBuf::from("/private/tmp/Interface.zip")]
        );
    }

    #[tokio::test]
    async fn signature_failure_and_missing_artifact_fail_closed() {
        for artifact in [Some("/private/tmp/attacker.zip"), None] {
            let root = tempfile::tempdir().expect("root");
            let service = service(
                root.path(),
                Some("https://updates.example.test/interface/"),
                verifier(Some("anchor apple generic"), false),
            );
            service.configure(input(true)).await;
            let state = service
                .native_event(NativeUpdaterEvent::Downloaded {
                    version: Some("0.3.0".to_owned()),
                    downloaded_file: artifact.map(str::to_owned),
                })
                .await;
            assert_eq!(state.phase, UpdatePhase::Error);
            assert!(!state.can_restart);
        }
    }

    #[tokio::test]
    async fn local_feed_is_private_and_closes_cleanly() {
        let root = tempfile::tempdir().expect("root");
        let service = service(root.path(), None, verifier(None, true));
        let configured = service.configure(input(true)).await;
        assert!(configured.enabled);
        assert!(
            configured
                .feed_url
                .as_deref()
                .is_some_and(|url| url.starts_with("http://127.0.0.1:"))
        );
        assert_eq!(service.state().await.source, UpdateSource::Local);
        service.close().await;
    }

    #[tokio::test]
    async fn a_stopped_local_feed_reports_unavailable_instead_of_enabled() {
        let root = tempfile::tempdir().expect("root");
        let service = service(root.path(), None, verifier(None, true));
        assert!(service.configure(input(true)).await.enabled);
        let mut states = service.subscribe();
        service
            .state
            .lock()
            .await
            .local_feed
            .as_ref()
            .expect("local feed")
            .stop_serving_for_test();

        let state = service.state().await;
        assert_eq!(state.phase, UpdatePhase::Disabled);
        assert_eq!(state.source, UpdateSource::None);
        assert!(!state.can_check);
        assert_eq!(
            states.recv().await.expect("published").phase,
            UpdatePhase::Disabled
        );
        assert_eq!(service.begin_check().await.phase, UpdatePhase::Disabled);
        service.close().await;
    }

    #[tokio::test]
    async fn invalid_remote_and_unavailable_requirement_disable_without_leaking_details() {
        let root = tempfile::tempdir().expect("root");
        let invalid = service(
            root.path(),
            Some("https://user:secret@example.test/?token=never"),
            verifier(None, true),
        );
        assert!(!invalid.configure(input(true)).await.enabled);

        let failed = Arc::new(FakeVerifier {
            requirement: Err(VerificationError::MissingRequirement),
            accepted: false,
            artifacts: StdMutex::new(Vec::new()),
            verifies: AtomicUsize::new(0),
        });
        let unavailable = service(root.path(), None, failed);
        assert!(!unavailable.configure(input(true)).await.enabled);
        assert_eq!(unavailable.state().await.phase, UpdatePhase::Disabled);
    }

    mod channel {
        use super::*;
        use crate::remote::tests::{FEED, FakeFeed, Publisher};
        use crate::{CHANNEL_STATE_NAME, REFUSED_UPDATE_MESSAGE};

        const ARCHIVE: &[u8] = b"the signed archive of the next release";

        struct Published {
            root: tempfile::TempDir,
            feed: Arc<FakeFeed>,
            publisher: Publisher,
            service: Arc<UpdaterService>,
        }

        /// A packaged 0.2.0 that follows a channel nothing is published on yet.
        async fn following() -> Published {
            let root = tempfile::tempdir().expect("root");
            let feed = Arc::new(FakeFeed::default());
            let publisher = Publisher::new();
            let service = Arc::new(
                UpdaterService::new(
                    root.path(),
                    None,
                    verifier(Some("anchor apple generic"), true),
                    Arc::new(FixedClock),
                )
                .with_release_channel(Some(publisher.channel(&[FEED])), feed.clone()),
            );
            assert!(service.configure(input(true)).await.enabled);
            Published {
                root,
                feed,
                publisher,
                service,
            }
        }

        impl Published {
            fn staged(&self, name: &str) -> Option<Vec<u8>> {
                std::fs::read(self.root.path().join("update-feed").join(name)).ok()
            }
        }

        #[tokio::test]
        async fn a_published_release_is_downloaded_checked_and_handed_to_the_installer() {
            let app = following().await;
            assert_eq!(app.service.state().await.source, UpdateSource::Remote);
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);

            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::Staged("0.3.0".to_owned())
            );
            let staged = app.service.state().await;
            assert_eq!(staged.phase, UpdatePhase::Downloading);
            assert_eq!(staged.available_version.as_deref(), Some("0.3.0"));
            assert_eq!(staged.progress, Some(100.0));
            assert!(!staged.can_restart);
            assert_eq!(
                app.staged("Interface-0.3.0-arm64-mac.zip").as_deref(),
                Some(ARCHIVE)
            );

            // The installer now finds it on the loopback feed. What it says
            // about its own copy does not walk the state backwards.
            for event in [
                NativeUpdaterEvent::Checking {},
                NativeUpdaterEvent::Available {
                    version: Some("0.3.0".to_owned()),
                },
                NativeUpdaterEvent::Progress {
                    percent: Some(12.0),
                },
            ] {
                let state = app.service.native_event(event).await;
                assert_eq!(state.phase, UpdatePhase::Downloading);
                assert_eq!(state.progress, Some(100.0));
            }
            let ready = app
                .service
                .native_event(NativeUpdaterEvent::Downloaded {
                    version: Some("0.3.0".to_owned()),
                    downloaded_file: Some("/private/tmp/Interface.zip".to_owned()),
                })
                .await;
            assert_eq!(ready.phase, UpdatePhase::Ready);
            assert!(ready.can_restart);
        }

        #[tokio::test]
        async fn nothing_newer_reads_up_to_date_whatever_the_installer_says_about_its_folder() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.2.0", ARCHIVE);
            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::UpToDate
            );
            let state = app.service.state().await;
            assert_eq!(state.phase, UpdatePhase::UpToDate);
            assert!(state.last_checked_at.is_some());

            // An empty loopback feed answers 404; that is not an update error.
            for event in [
                NativeUpdaterEvent::Checking {},
                NativeUpdaterEvent::Error {},
                NativeUpdaterEvent::CheckResult {
                    outcome: NativeCheckOutcome::Error,
                    version: None,
                },
                NativeUpdaterEvent::NotAvailable {},
            ] {
                let state = app.service.native_event(event).await;
                assert_eq!(state.phase, UpdatePhase::UpToDate);
                assert_eq!(state.error, None);
            }
        }

        #[tokio::test]
        async fn a_feed_with_nothing_published_yet_reads_up_to_date_even_when_asked() {
            let app = following().await;
            app.feed
                .not_found
                .lock()
                .expect("not found")
                .push(FEED.to_owned());
            let asked = app.service.begin_check().await;
            assert_eq!(asked.phase, UpdatePhase::UpToDate);
            assert_eq!(asked.error, None);
            assert!(asked.last_checked_at.is_some());
        }

        #[tokio::test]
        async fn a_release_nobody_trusted_signed_is_refused_in_words_and_never_staged() {
            let app = following().await;
            Publisher::new().publish(&app.feed, FEED, "9.9.9", b"forged");
            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::Failed(MirrorError::Integrity)
            );
            let state = app.service.state().await;
            assert_eq!(state.phase, UpdatePhase::Error);
            assert_eq!(state.error.as_deref(), Some(REFUSED_UPDATE_MESSAGE));
            assert!(!state.can_restart);
            assert_eq!(app.staged("latest-mac.yml"), None);
        }

        #[tokio::test]
        async fn an_unreachable_feed_is_quiet_on_the_schedule_and_said_when_asked() {
            let app = following().await;
            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::Failed(MirrorError::Network)
            );
            assert_eq!(app.service.state().await.phase, UpdatePhase::Idle);

            let asked = app.service.begin_check().await;
            assert_eq!(asked.phase, UpdatePhase::Error);
            assert!(asked.can_check);
        }

        #[tokio::test]
        async fn a_download_that_breaks_off_says_so_and_the_next_check_finishes_it() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);
            let archive = "http://127.0.0.1:9/releases/Interface-0.3.0-arm64-mac.zip";
            app.feed.put(archive, &ARCHIVE[..ARCHIVE.len() / 2]);
            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::Failed(MirrorError::Network)
            );
            let broken = app.service.state().await;
            assert_eq!(broken.phase, UpdatePhase::Error);
            assert_eq!(broken.available_version.as_deref(), Some("0.3.0"));

            app.feed.put(archive, ARCHIVE);
            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::Staged("0.3.0".to_owned())
            );
            assert_eq!(app.service.state().await.phase, UpdatePhase::Downloading);
        }

        #[tokio::test]
        async fn check_for_updates_asks_the_public_feed() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);
            let state = app.service.begin_check().await;
            assert_eq!(state.phase, UpdatePhase::Downloading);
            assert_eq!(app.feed.requested(FEED), 1);
        }

        #[tokio::test]
        async fn check_for_updates_returns_at_once_and_the_schedule_does_the_work() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);
            let schedule = app.service.spawn_channel_schedule();
            assert_eq!(app.service.begin_check().await.phase, UpdatePhase::Checking);
            let mut states = app.service.subscribe();
            let staged = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let state = states.recv().await.expect("state");
                    if state.phase == UpdatePhase::Downloading && state.progress == Some(100.0) {
                        break state;
                    }
                }
            })
            .await
            .expect("the schedule ran the check");
            assert_eq!(staged.available_version.as_deref(), Some("0.3.0"));
            schedule.abort();
        }

        #[tokio::test]
        async fn a_release_ready_to_install_stays_ready_while_a_later_one_arrives() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);
            app.service.check_channel(false).await;
            let downloaded = |version: &str| NativeUpdaterEvent::Downloaded {
                version: Some(version.to_owned()),
                downloaded_file: Some("/private/tmp/Interface.zip".to_owned()),
            };
            assert_eq!(
                app.service.native_event(downloaded("0.3.0")).await.phase,
                UpdatePhase::Ready
            );

            app.publisher
                .publish(&app.feed, FEED, "0.3.1", b"a later archive");
            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::Staged("0.3.1".to_owned())
            );
            let still = app.service.state().await;
            assert_eq!(still.phase, UpdatePhase::Ready);
            assert_eq!(still.available_version.as_deref(), Some("0.3.0"));
            let later = app.service.native_event(downloaded("0.3.1")).await;
            assert_eq!(later.phase, UpdatePhase::Ready);
            assert_eq!(later.available_version.as_deref(), Some("0.3.1"));
        }

        #[tokio::test]
        async fn a_release_ready_to_install_survives_a_feed_that_goes_quiet_or_bad() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);
            app.service.check_channel(false).await;
            app.service
                .native_event(NativeUpdaterEvent::Downloaded {
                    version: Some("0.3.0".to_owned()),
                    downloaded_file: Some("/private/tmp/Interface.zip".to_owned()),
                })
                .await;

            app.feed.files.lock().expect("files").clear();
            assert_eq!(
                app.service.check_channel(true).await,
                ChannelCheck::Failed(MirrorError::Network)
            );
            Publisher::new().publish(&app.feed, FEED, "9.9.9", b"forged");
            assert_eq!(
                app.service.check_channel(true).await,
                ChannelCheck::Failed(MirrorError::Integrity)
            );
            let state = app.service.state().await;
            assert_eq!(state.phase, UpdatePhase::Ready);
            assert!(state.can_restart);
            assert_eq!(state.error, None);
        }

        #[tokio::test]
        async fn an_install_macos_could_not_finish_is_said() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);
            app.service.check_channel(false).await;
            app.service
                .native_event(NativeUpdaterEvent::Downloaded {
                    version: Some("0.3.0".to_owned()),
                    downloaded_file: Some("/private/tmp/Interface.zip".to_owned()),
                })
                .await;
            assert!(app.service.request_install().await);
            let failed = app.service.native_event(NativeUpdaterEvent::Error {}).await;
            assert_eq!(failed.phase, UpdatePhase::Error);
            assert!(failed.can_check);
        }

        #[tokio::test]
        async fn a_release_staged_on_this_mac_still_installs_while_the_channel_is_followed() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.2.0", ARCHIVE);
            app.service.check_channel(false).await;
            // `cargo xtask stage-update` fills the folder; the installer sees it.
            let available = app
                .service
                .native_event(NativeUpdaterEvent::Available {
                    version: Some("0.2.1".to_owned()),
                })
                .await;
            assert_eq!(available.phase, UpdatePhase::Available);
            let ready = app
                .service
                .native_event(NativeUpdaterEvent::Downloaded {
                    version: Some("0.2.1".to_owned()),
                    downloaded_file: Some("/private/tmp/Interface.zip".to_owned()),
                })
                .await;
            assert_eq!(ready.phase, UpdatePhase::Ready);
        }

        #[tokio::test]
        async fn an_app_that_is_not_packaged_never_asks_the_feed() {
            let root = tempfile::tempdir().expect("root");
            let feed = Arc::new(FakeFeed::default());
            let publisher = Publisher::new();
            publisher.publish(&feed, FEED, "0.3.0", ARCHIVE);
            let service = UpdaterService::new(
                root.path(),
                None,
                verifier(None, true),
                Arc::new(FixedClock),
            )
            .with_release_channel(Some(publisher.channel(&[FEED])), feed.clone());
            service.configure(input(false)).await;
            assert_eq!(
                service.check_channel(true).await,
                ChannelCheck::NotFollowing
            );
            assert_eq!(service.begin_check().await.phase, UpdatePhase::Disabled);
            assert!(feed.requests.lock().expect("requests").is_empty());
            assert!(!root.path().join(CHANNEL_STATE_NAME).exists());
        }

        #[tokio::test]
        async fn closing_stops_a_check_that_is_under_way() {
            let app = following().await;
            app.publisher.publish(&app.feed, FEED, "0.3.0", ARCHIVE);
            app.service.close().await;
            assert_eq!(
                app.service.check_channel(false).await,
                ChannelCheck::Failed(MirrorError::Stopped)
            );
            assert_eq!(app.staged("latest-mac.yml"), None);
        }
    }

    #[test]
    fn native_event_wire_is_exact_and_rejects_unknown_fields() {
        let downloaded: NativeUpdaterEvent = serde_json::from_value(serde_json::json!({
            "kind":"downloaded",
            "version":"0.3.0",
            "downloadedFile":"/private/tmp/Interface.zip"
        }))
        .expect("downloaded");
        assert!(matches!(downloaded, NativeUpdaterEvent::Downloaded { .. }));
        assert!(
            serde_json::from_value::<NativeUpdaterEvent>(serde_json::json!({
                "kind":"error", "message":"private native details"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn retained_archive_reverify_refuses_tampered_install() {
        struct RetainingVerifier;

        #[async_trait]
        impl UpdaterVerifier for RetainingVerifier {
            async fn running_requirement(
                &self,
                _packaged: bool,
                _app_bundle: Option<&Path>,
            ) -> Result<Option<String>, VerificationError> {
                Ok(Some("anchor apple generic".to_owned()))
            }

            async fn verify_artifact(
                &self,
                _artifact: &Path,
                _requirement: &str,
            ) -> Result<(), VerificationError> {
                Ok(())
            }

            async fn verify_and_retain_artifact(
                &self,
                artifact: &Path,
                _requirement: &str,
                current_version: &str,
                expected_version: Option<&str>,
                staging_root: &Path,
            ) -> Result<Option<RetainedVerifiedArchive>, VerificationError> {
                if !crate::is_newer_version(expected_version.unwrap_or_default(), current_version) {
                    return Err(VerificationError::RollbackOrVersionMismatch);
                }
                crate::retain_verified_archive_file(artifact, Some(staging_root))
                    .await
                    .map(Some)
            }
        }

        let root = tempfile::tempdir().expect("root");
        let downloaded = root.path().join("Interface-0.3.0.zip");
        std::fs::write(&downloaded, b"signed-archive-bytes").expect("downloaded zip");

        let service = UpdaterService::new(
            root.path(),
            Some("https://updates.example.test/interface/".to_owned()),
            Arc::new(RetainingVerifier),
            Arc::new(FixedClock),
        );
        service.configure(input(true)).await;
        let ready = service
            .native_event(NativeUpdaterEvent::Downloaded {
                version: Some("0.3.0".to_owned()),
                downloaded_file: Some(downloaded.to_string_lossy().into_owned()),
            })
            .await;
        assert_eq!(ready.phase, UpdatePhase::Ready);
        let retained_path = service.verified_archive_path().await.expect("retained zip");
        assert!(retained_path.exists());

        // Mutating the downloaded zip after verification must fail `request_install`.
        std::fs::write(&downloaded, b"tampered-archive-bytes").expect("tamper");
        assert!(!service.request_install().await);
        let refused = service.state().await;
        assert_eq!(refused.phase, UpdatePhase::Error);
        assert!(!refused.can_restart);
    }
}
