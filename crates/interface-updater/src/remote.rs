//! Brings the newest signed release from the public feed into the local feed.
//!
//! The installer that already ships (electron-updater, Squirrel.Mac and the
//! codesign gate in `macos.rs`) only ever reads the loopback feed served from
//! `update-feed` in the data folder. This module is the one place that talks
//! to the internet: it fetches the signed manifest, checks it, downloads the
//! archive the manifest names, checks the archive's size and SHA-512, and only
//! then moves both into that folder. Bytes nobody signed never reach the
//! installer.
//!
//! The folder has a second possible writer on a developer's Mac: `cargo xtask
//! stage-update`. The two never fight over it. Whatever is in the folder, a
//! public release is staged only when it is newer than both the running app
//! and the release already there, and never while a local stage holds its
//! lock; `stage-update` in turn refuses to stage anything older.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha512};
use url::Url;

use crate::channel::ReleaseChannel;
use crate::signed_manifest::{
    MAX_SIGNED_MANIFEST_BYTES, ReleaseManifest, feed_summary, is_artifact_name,
    verify_signed_manifest,
};
use crate::state::{is_newer_version, is_plain_version};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;

/// Where the mirror remembers what it staged and the newest release it has
/// ever seen signed. Lives beside, not inside, the feed folder.
pub const CHANNEL_STATE_NAME: &str = "update-channel.v1.json";
const FEED_MANIFEST_NAME: &str = "latest-mac.yml";
const PARTIAL_PREFIX: &str = ".mirror-";
/// `cargo xtask stage-update` holds this while it rewrites the folder.
const STAGING_LOCK_NAME: &str = ".package-mac.lock";
const CHANNEL_STATE_SCHEMA: u32 = 1;
const MAX_CHANNEL_STATE_BYTES: u64 = 64 * 1024;
const MAX_FEED_MANIFEST_BYTES: u64 = 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const USER_AGENT: &str = "Interface-Updater";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchError {
    /// No answer, a refused or failed request, or a body that ended early.
    Unreachable,
    /// The host answered, and what it said is that the file is not there.
    Missing,
    /// More bytes than the caller allowed.
    TooLarge,
    /// The caller's sink refused the bytes.
    Stopped,
}

/// The network, as little of it as an update needs. Blocking: callers run it
/// off the async runtime.
pub trait RemoteFetcher: Send + Sync {
    /// Reads a small document whole, refusing one over `max_bytes`.
    ///
    /// # Errors
    ///
    /// See [`FetchError`].
    fn fetch(&self, url: &Url, max_bytes: u64) -> Result<Vec<u8>, FetchError>;

    /// Streams a body of exactly `expected_bytes` into `sink`.
    ///
    /// # Errors
    ///
    /// See [`FetchError`]. A body of any other length is an error.
    fn download(
        &self,
        url: &Url,
        expected_bytes: u64,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), FetchError>;
}

/// HTTPS through rustls with the bundled web roots. Redirects are followed,
/// to HTTPS only; no cookies, no credentials.
pub struct UreqFetcher {
    documents: ureq::Agent,
    archives: ureq::Agent,
}

impl UreqFetcher {
    #[must_use]
    pub fn new() -> Self {
        Self::build(true)
    }

    /// For a feed served from this Mac in tests and rehearsals.
    #[must_use]
    pub fn allowing_loopback_http() -> Self {
        Self::build(false)
    }

    fn build(https_only: bool) -> Self {
        let agent = |whole_exchange: Duration| {
            ureq::Agent::config_builder()
                .https_only(https_only)
                .max_redirects(5)
                .user_agent(USER_AGENT)
                .timeout_resolve(Some(Duration::from_secs(15)))
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_per_read(Some(Duration::from_secs(60)))
                .timeout_global(Some(whole_exchange))
                .build()
                .new_agent()
        };
        Self {
            documents: agent(Duration::from_secs(45)),
            archives: agent(Duration::from_secs(2 * 60 * 60)),
        }
    }
}

impl Default for UreqFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteFetcher for UreqFetcher {
    fn fetch(&self, url: &Url, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        let mut response = self
            .documents
            .get(url.as_str())
            .call()
            .map_err(|error| request_failure(&error))?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| FetchError::Unreachable)?;
        if bytes.len() as u64 > max_bytes {
            return Err(FetchError::TooLarge);
        }
        Ok(bytes)
    }

    fn download(
        &self,
        url: &Url,
        expected_bytes: u64,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), FetchError> {
        let mut response = self
            .archives
            .get(url.as_str())
            .call()
            .map_err(|error| request_failure(&error))?;
        if response
            .body()
            .content_length()
            .is_some_and(|length| length != expected_bytes)
        {
            return Err(FetchError::TooLarge);
        }
        let mut reader = response.body_mut().as_reader();
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
        let mut received = 0_u64;
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|_| FetchError::Unreachable)?;
            if count == 0 {
                break;
            }
            received = received.saturating_add(count as u64);
            if received > expected_bytes {
                return Err(FetchError::TooLarge);
            }
            sink(&buffer[..count]).map_err(|_| FetchError::Stopped)?;
        }
        if received == expected_bytes {
            Ok(())
        } else {
            Err(FetchError::Unreachable)
        }
    }
}

fn request_failure(error: &ureq::Error) -> FetchError {
    match error {
        ureq::Error::StatusCode(404 | 410) => FetchError::Missing,
        _ => FetchError::Unreachable,
    }
}

/// One check of the public feed.
#[derive(Clone, Debug)]
pub struct MirrorPlan {
    pub channel: ReleaseChannel,
    /// `update-feed` in the data folder: what the loopback feed serves.
    pub feed_dir: PathBuf,
    /// [`CHANNEL_STATE_NAME`] in the data folder.
    pub state_path: PathBuf,
    pub current_version: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MirrorEvent {
    /// A newer release is signed and its download is starting.
    Available(String),
    /// Whole percent of the archive received.
    Progress(f64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirrorOutcome {
    /// The feed's newest release is not newer than the running app.
    UpToDate,
    /// Every feed answered that it holds no manifest: nothing has been
    /// published yet, which is not a fault.
    NothingPublished,
    /// The local feed already holds this release, intact, or a newer one
    /// staged on this Mac. The installer takes it from there.
    AlreadyStaged(String),
    /// The newest release was downloaded, checked and put in the local feed.
    Staged(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorError {
    /// No feed could be reached, or a download broke off.
    Network,
    /// A manifest nobody trusted signed, an older release offered again, or
    /// an archive that is not the bytes the manifest names.
    Integrity,
    /// The data folder could not be read or written.
    Storage,
    /// A local stage is rewriting the folder right now.
    Busy,
    /// The app is shutting down.
    Stopped,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct ChannelState {
    schema: u32,
    /// Newest release a trusted key has ever been seen to sign. A feed that
    /// later offers anything older is replaying it.
    highest_verified_version: Option<String>,
    /// The release this module last put in the local feed. Only ever used
    /// to free its archive once installed: losing it loses nothing else.
    mirrored: Option<MirroredRelease>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MirroredRelease {
    version: String,
    artifact: String,
    /// Hex SHA-512 of the `latest-mac.yml` written for it.
    feed_sha512: String,
}

/// What `latest-mac.yml` in the local feed currently offers.
struct LocalFeed {
    bytes: Vec<u8>,
    version: String,
    archive: String,
}

/// Tidies, once at startup, what an earlier run left: unfinished downloads,
/// and the archive of a release this module staged that is now installed.
/// A feed staged on this Mac by `cargo xtask stage-update` is left alone.
pub fn prepare(feed_dir: &Path, state_path: &Path, current_version: &str) {
    remove_partials(feed_dir);
    let mut state = read_state(state_path);
    let Some(release) = state.mirrored.take() else {
        return;
    };
    let ours = local_feed(feed_dir)
        .is_some_and(|feed| hex(&Sha512::digest(&feed.bytes)) == release.feed_sha512);
    if ours && is_newer_version(&release.version, current_version) {
        return;
    }
    if ours {
        // Installed. The archive has done its work.
        remove_file(&feed_dir.join(FEED_MANIFEST_NAME));
        remove_file(&feed_dir.join(&release.artifact));
    }
    let _ = write_state(state_path, &state);
}

/// Checks every feed of the channel in order and stages a newer release.
///
/// `events` hears about a release found and its download; `stop` ends a
/// download early.
///
/// # Errors
///
/// See [`MirrorError`]. A feed that cannot be reached or that offers
/// something untrusted is passed over for the next; the error returned is
/// the most serious one met.
pub fn refresh(
    plan: &MirrorPlan,
    fetcher: &dyn RemoteFetcher,
    events: &mut dyn FnMut(MirrorEvent),
    stop: &AtomicBool,
) -> Result<MirrorOutcome, MirrorError> {
    let mut worst = MirrorError::Network;
    let mut unpublished = 0;
    for feed in plan.channel.feeds() {
        match refresh_from(plan, feed, fetcher, events, stop) {
            Ok(outcome) => return Ok(outcome),
            Err(FeedFailure::Unpublished) => unpublished += 1,
            Err(FeedFailure::Failed(MirrorError::Network)) => {}
            Err(FeedFailure::Failed(MirrorError::Integrity)) => worst = MirrorError::Integrity,
            Err(FeedFailure::Failed(error)) => return Err(error),
        }
    }
    if unpublished == plan.channel.feeds().len() {
        return Ok(MirrorOutcome::NothingPublished);
    }
    Err(worst)
}

/// Why one feed gave no release.
enum FeedFailure {
    /// It answered that it has no manifest.
    Unpublished,
    Failed(MirrorError),
}

impl From<MirrorError> for FeedFailure {
    fn from(error: MirrorError) -> Self {
        Self::Failed(error)
    }
}

fn refresh_from(
    plan: &MirrorPlan,
    feed: &Url,
    fetcher: &dyn RemoteFetcher,
    events: &mut dyn FnMut(MirrorEvent),
    stop: &AtomicBool,
) -> Result<MirrorOutcome, FeedFailure> {
    let signed = fetcher
        .fetch(feed, MAX_SIGNED_MANIFEST_BYTES as u64)
        .map_err(|error| match error {
            FetchError::Missing => FeedFailure::Unpublished,
            FetchError::TooLarge => FeedFailure::Failed(MirrorError::Integrity),
            FetchError::Unreachable | FetchError::Stopped => {
                FeedFailure::Failed(MirrorError::Network)
            }
        })?;
    let manifest = verify_signed_manifest(
        &signed,
        plan.channel.keys(),
        plan.channel.product(),
        plan.channel.channel(),
    )
    .map_err(|_| MirrorError::Integrity)?;
    let digest = manifest.artifact_sha512().ok_or(MirrorError::Integrity)?;

    let mut state = read_state(&plan.state_path);
    if state
        .highest_verified_version
        .as_deref()
        .is_some_and(|highest| is_newer_version(highest, &manifest.version))
    {
        return Err(MirrorError::Integrity.into());
    }
    if state.highest_verified_version.as_deref() != Some(manifest.version.as_str()) {
        state.highest_verified_version = Some(manifest.version.clone());
        write_state(&plan.state_path, &state)?;
    }
    if !is_newer_version(&manifest.version, &plan.current_version) {
        return Ok(MirrorOutcome::UpToDate);
    }

    let local = local_feed(&plan.feed_dir);
    if let Some(local) = &local {
        if is_newer_version(&local.version, &manifest.version) {
            return Ok(MirrorOutcome::AlreadyStaged(local.version.clone()));
        }
        if local.version == manifest.version
            && local.archive == manifest.artifact.name
            && file_matches(
                &plan.feed_dir.join(&local.archive),
                manifest.artifact.size,
                &digest,
            )
        {
            return Ok(MirrorOutcome::AlreadyStaged(manifest.version));
        }
    }
    if plan.feed_dir.join(STAGING_LOCK_NAME).exists() {
        return Err(MirrorError::Busy.into());
    }

    events(MirrorEvent::Available(manifest.version.clone()));
    let archive_url = feed
        .join(&manifest.artifact.name)
        .map_err(|_| MirrorError::Integrity)?;
    let partial = Partial::create(&plan.feed_dir)?;
    download_archive(
        &partial,
        &archive_url,
        &manifest,
        &digest,
        fetcher,
        events,
        stop,
    )?;
    Ok(stage(plan, &mut state, &manifest, partial)?)
}

fn download_archive(
    partial: &Partial,
    url: &Url,
    manifest: &ReleaseManifest,
    digest: &[u8; 64],
    fetcher: &dyn RemoteFetcher,
    events: &mut dyn FnMut(MirrorEvent),
    stop: &AtomicBool,
) -> Result<(), MirrorError> {
    let expected = manifest.artifact.size;
    let mut file = partial.open()?;
    let mut hasher = Sha512::new();
    let mut received = 0_u64;
    let mut reported = None;
    let mut storage_failed = false;
    let mut sink = |chunk: &[u8]| -> io::Result<()> {
        if stop.load(Ordering::SeqCst) {
            return Err(io::Error::other("stopped"));
        }
        if let Err(error) = file.write_all(chunk) {
            storage_failed = true;
            return Err(error);
        }
        hasher.update(chunk);
        received = received.saturating_add(chunk.len() as u64);
        let percent = received.saturating_mul(100) / expected.max(1);
        if reported != Some(percent) {
            reported = Some(percent);
            #[allow(clippy::cast_precision_loss)]
            events(MirrorEvent::Progress(percent as f64));
        }
        Ok(())
    };
    match fetcher.download(url, expected, &mut sink) {
        Ok(()) => {}
        Err(FetchError::TooLarge) => return Err(MirrorError::Integrity),
        // A manifest that names an archive the host does not have is a
        // release still being uploaded, or one being replaced: try again.
        Err(FetchError::Unreachable | FetchError::Missing) => return Err(MirrorError::Network),
        Err(FetchError::Stopped) if storage_failed => return Err(MirrorError::Storage),
        Err(FetchError::Stopped) => return Err(MirrorError::Stopped),
    }
    file.sync_all().map_err(|_| MirrorError::Storage)?;
    if received != expected || hasher.finalize().as_slice() != digest {
        return Err(MirrorError::Integrity);
    }
    Ok(())
}

/// Moves a checked archive and its installer feed into the local feed: the
/// archive first and `latest-mac.yml` last, the order the installer's watch
/// expects. A crash in between leaves the old feed working and an archive
/// the next check replaces.
fn stage(
    plan: &MirrorPlan,
    state: &mut ChannelState,
    manifest: &ReleaseManifest,
    partial: Partial,
) -> Result<MirrorOutcome, MirrorError> {
    // The download took minutes; look again before touching the folder.
    if plan.feed_dir.join(STAGING_LOCK_NAME).exists() {
        return Err(MirrorError::Busy);
    }
    let replaced = local_feed(&plan.feed_dir);
    if replaced
        .as_ref()
        .is_some_and(|local| is_newer_version(&local.version, &manifest.version))
    {
        let version = replaced.map(|local| local.version).unwrap_or_default();
        return Ok(MirrorOutcome::AlreadyStaged(version));
    }
    let feed_bytes = manifest.latest_mac_yml.as_bytes();
    let release = MirroredRelease {
        version: manifest.version.clone(),
        artifact: manifest.artifact.name.clone(),
        feed_sha512: hex(&Sha512::digest(feed_bytes)),
    };

    let archive = plan.feed_dir.join(&release.artifact);
    remove_file(&archive);
    partial.rename_to(&archive)?;
    let feed_partial = Partial::create(&plan.feed_dir)?;
    let mut file = feed_partial.open()?;
    file.write_all(feed_bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| MirrorError::Storage)?;
    drop(file);
    feed_partial.rename_to(&plan.feed_dir.join(FEED_MANIFEST_NAME))?;

    if let Some(old) = replaced.filter(|old| old.archive != release.artifact) {
        remove_file(&plan.feed_dir.join(old.archive));
    }
    state.mirrored = Some(release.clone());
    // The release is staged whether or not this note of it is kept.
    let _ = write_state(&plan.state_path, state);
    Ok(MirrorOutcome::Staged(release.version))
}

/// A private file in the feed folder that is removed unless it is renamed
/// into place. The local feed never serves a name that starts with a dot.
struct Partial {
    path: PathBuf,
    kept: bool,
}

impl Partial {
    fn create(feed_dir: &Path) -> Result<Self, MirrorError> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path = feed_dir.join(format!(
            "{PARTIAL_PREFIX}{}-{unique}.partial",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        options.open(&path).map_err(|_| MirrorError::Storage)?;
        Ok(Self { path, kept: false })
    }

    fn open(&self) -> Result<File, MirrorError> {
        let mut options = OpenOptions::new();
        options.write(true).truncate(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        options.open(&self.path).map_err(|_| MirrorError::Storage)
    }

    fn rename_to(mut self, destination: &Path) -> Result<(), MirrorError> {
        fs::rename(&self.path, destination).map_err(|_| MirrorError::Storage)?;
        self.kept = true;
        Ok(())
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.kept {
            remove_file(&self.path);
        }
    }
}

/// Reads the local feed's `latest-mac.yml`. `None` when there is none, or
/// none that names one flat archive of a readable version.
fn local_feed(feed_dir: &Path) -> Option<LocalFeed> {
    let bytes = read_regular_file(&feed_dir.join(FEED_MANIFEST_NAME), MAX_FEED_MANIFEST_BYTES)
        .ok()
        .flatten()?;
    let summary = feed_summary(std::str::from_utf8(&bytes).ok()?);
    let version = summary
        .version
        .filter(|version| is_plain_version(version))?;
    let archive = summary.path.filter(|path| is_artifact_name(path))?;
    Some(LocalFeed {
        version: version.to_owned(),
        archive: archive.to_owned(),
        bytes,
    })
}

fn file_matches(path: &Path, size: u64, sha512: &[u8; 64]) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() || metadata.len() != size {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut hasher = Sha512::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => hasher.update(&buffer[..count]),
            Err(_) => return false,
        }
    }
    hasher.finalize().as_slice() == sha512
}

fn remove_partials(feed_dir: &Path) {
    let Ok(entries) = fs::read_dir(feed_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(PARTIAL_PREFIX))
        {
            remove_file(&entry.path());
        }
    }
}

fn remove_file(path: &Path) {
    let _ = fs::remove_file(path);
}

/// `Ok(None)` for a missing file; `Err` for anything but a plain file within
/// the size limit.
fn read_regular_file(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>, ()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return Err(());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| ())?
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > max_bytes {
        return Err(());
    }
    Ok(Some(bytes))
}

/// An unreadable or damaged state file is the same as none: the worst that
/// follows is one archive downloaded again.
fn read_state(path: &Path) -> ChannelState {
    read_regular_file(path, MAX_CHANNEL_STATE_BYTES)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<ChannelState>(&bytes).ok())
        .filter(|state| state.schema == CHANNEL_STATE_SCHEMA)
        .unwrap_or_default()
}

fn write_state(path: &Path, state: &ChannelState) -> Result<(), MirrorError> {
    let directory = path.parent().ok_or(MirrorError::Storage)?;
    let mut state = state.clone();
    state.schema = CHANNEL_STATE_SCHEMA;
    let mut bytes = serde_json::to_vec_pretty(&state).map_err(|_| MirrorError::Storage)?;
    bytes.push(b'\n');
    let partial = Partial::create(directory)?;
    let mut file = partial.open()?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| MirrorError::Storage)?;
    drop(file);
    partial.rename_to(path)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::signed_manifest::tests::{CHANNEL, PRODUCT, manifest_for};
    use crate::signed_manifest::{PublicKey, generate_signing_key, sign_manifest};

    pub(crate) const FEED: &str = "http://127.0.0.1:9/releases/interface-update.json";

    /// A feed held in memory: what each address answers, and what was asked.
    #[derive(Default)]
    pub(crate) struct FakeFeed {
        pub(crate) files: Mutex<HashMap<String, Vec<u8>>>,
        pub(crate) requests: Mutex<Vec<String>>,
        /// Addresses the host answers "not found" for; any other address
        /// with no file does not answer at all.
        pub(crate) not_found: Mutex<Vec<String>>,
    }

    impl FakeFeed {
        pub(crate) fn put(&self, url: &str, bytes: &[u8]) {
            self.files
                .lock()
                .expect("files")
                .insert(url.to_owned(), bytes.to_vec());
        }

        pub(crate) fn requested(&self, url: &str) -> usize {
            self.requests
                .lock()
                .expect("requests")
                .iter()
                .filter(|asked| asked.as_str() == url)
                .count()
        }

        fn answer(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
            self.requests
                .lock()
                .expect("requests")
                .push(url.to_string());
            if let Some(bytes) = self.files.lock().expect("files").get(url.as_str()) {
                return Ok(bytes.clone());
            }
            if self
                .not_found
                .lock()
                .expect("not found")
                .iter()
                .any(|gone| gone == url.as_str())
            {
                return Err(FetchError::Missing);
            }
            Err(FetchError::Unreachable)
        }
    }

    impl RemoteFetcher for FakeFeed {
        fn fetch(&self, url: &Url, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
            let bytes = self.answer(url)?;
            if bytes.len() as u64 > max_bytes {
                return Err(FetchError::TooLarge);
            }
            Ok(bytes)
        }

        fn download(
            &self,
            url: &Url,
            expected_bytes: u64,
            sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
        ) -> Result<(), FetchError> {
            let bytes = self.answer(url)?;
            if bytes.len() as u64 > expected_bytes {
                return Err(FetchError::TooLarge);
            }
            for chunk in bytes.chunks(7) {
                sink(chunk).map_err(|_| FetchError::Stopped)?;
            }
            if bytes.len() as u64 == expected_bytes {
                Ok(())
            } else {
                Err(FetchError::Unreachable)
            }
        }
    }

    pub(crate) struct Publisher {
        pkcs8: Vec<u8>,
        pub(crate) key: PublicKey,
    }

    impl Publisher {
        pub(crate) fn new() -> Self {
            let (pkcs8, key) = generate_signing_key().expect("key");
            Self { pkcs8, key }
        }

        /// Publishes `version` with `archive` at `feed`, as the owner would.
        pub(crate) fn publish(&self, feed: &FakeFeed, at: &str, version: &str, archive: &[u8]) {
            let manifest = manifest_for(version, archive);
            let base = Url::parse(at).expect("feed url");
            feed.put(
                base.join(&manifest.artifact.name)
                    .expect("archive url")
                    .as_str(),
                archive,
            );
            feed.put(at, &sign_manifest(&manifest, &self.pkcs8).expect("signed"));
        }

        pub(crate) fn channel(&self, feeds: &[&str]) -> ReleaseChannel {
            ReleaseChannel::loopback(PRODUCT, CHANNEL, feeds, &[self.key]).expect("channel")
        }
    }

    struct Home {
        root: tempfile::TempDir,
    }

    impl Home {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("root");
            fs::create_dir(root.path().join("update-feed")).expect("feed dir");
            Self { root }
        }

        fn feed_dir(&self) -> PathBuf {
            self.root.path().join("update-feed")
        }

        fn state_path(&self) -> PathBuf {
            self.root.path().join(CHANNEL_STATE_NAME)
        }

        fn plan(&self, publisher: &Publisher, current: &str) -> MirrorPlan {
            MirrorPlan {
                channel: publisher.channel(&[FEED]),
                feed_dir: self.feed_dir(),
                state_path: self.state_path(),
                current_version: current.to_owned(),
            }
        }

        fn staged(&self, name: &str) -> Option<Vec<u8>> {
            fs::read(self.feed_dir().join(name)).ok()
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.feed_dir())
                .expect("feed dir")
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            names.sort();
            names
        }
    }

    fn run(
        plan: &MirrorPlan,
        feed: &FakeFeed,
    ) -> (Result<MirrorOutcome, MirrorError>, Vec<MirrorEvent>) {
        let mut events = Vec::new();
        let outcome = refresh(
            plan,
            feed,
            &mut |event| events.push(event),
            &AtomicBool::new(false),
        );
        (outcome, events)
    }

    const ARCHIVE: &[u8] = b"the bytes of a signed, notarized Interface.app in a zip";

    #[test]
    fn a_newer_signed_release_lands_in_the_local_feed_byte_for_byte() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        let (outcome, events) = run(&home.plan(&publisher, "0.3.9"), &feed);
        assert_eq!(outcome, Ok(MirrorOutcome::Staged("0.4.0".to_owned())));
        assert_eq!(
            events.first(),
            Some(&MirrorEvent::Available("0.4.0".to_owned()))
        );
        assert_eq!(events.last(), Some(&MirrorEvent::Progress(100.0)));
        assert_eq!(
            home.staged("Interface-0.4.0-arm64-mac.zip").as_deref(),
            Some(ARCHIVE)
        );
        assert_eq!(
            home.staged("latest-mac.yml"),
            Some(manifest_for("0.4.0", ARCHIVE).latest_mac_yml.into_bytes())
        );
        assert_eq!(
            home.names(),
            ["Interface-0.4.0-arm64-mac.zip", "latest-mac.yml"]
        );
    }

    #[test]
    fn the_running_version_or_an_older_one_downloads_nothing() {
        for published in ["0.4.0", "0.3.0"] {
            let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
            publisher.publish(&feed, FEED, published, ARCHIVE);
            let (outcome, events) = run(&home.plan(&publisher, "0.4.0"), &feed);
            assert_eq!(outcome, Ok(MirrorOutcome::UpToDate), "{published}");
            assert!(events.is_empty());
            assert!(home.names().is_empty());
        }
    }

    #[test]
    fn a_feed_with_nothing_published_yet_is_not_a_fault() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        let second = "http://127.0.0.1:9/mirror/interface-update.json";
        let mut plan = home.plan(&publisher, "0.3.9");
        plan.channel = publisher.channel(&[FEED, second]);
        feed.not_found
            .lock()
            .expect("not found")
            .push(FEED.to_owned());
        // One feed empty and the other silent: still unknown, so still a failure.
        assert_eq!(run(&plan, &feed).0, Err(MirrorError::Network));
        feed.not_found
            .lock()
            .expect("not found")
            .push(second.to_owned());
        let (outcome, events) = run(&plan, &feed);
        assert_eq!(outcome, Ok(MirrorOutcome::NothingPublished));
        assert!(events.is_empty());
        assert!(home.names().is_empty());
    }

    #[test]
    fn a_manifest_signed_by_someone_else_downloads_nothing() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        Publisher::new().publish(&feed, FEED, "0.4.0", ARCHIVE);
        let (outcome, events) = run(&home.plan(&publisher, "0.3.9"), &feed);
        assert_eq!(outcome, Err(MirrorError::Integrity));
        assert!(events.is_empty());
        assert_eq!(feed.requests.lock().expect("requests").len(), 1);
        assert!(home.names().is_empty());
    }

    #[test]
    fn an_archive_that_is_not_the_signed_bytes_never_reaches_the_installer() {
        // Same length, one byte different: only the digest can tell.
        let mut one_byte_off = ARCHIVE.to_vec();
        one_byte_off[ARCHIVE.len() / 2] ^= 1;
        let longer = [ARCHIVE, b" plus more"].concat();
        for (name, swapped, expected) in [
            ("one byte off", &one_byte_off[..], MirrorError::Integrity),
            ("shorter", &b"short"[..], MirrorError::Network),
            ("longer", &longer[..], MirrorError::Integrity),
        ] {
            let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
            publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
            feed.put(
                "http://127.0.0.1:9/releases/Interface-0.4.0-arm64-mac.zip",
                swapped,
            );
            let (outcome, _) = run(&home.plan(&publisher, "0.3.9"), &feed);
            assert_eq!(outcome, Err(expected), "{name}");
            assert!(home.names().is_empty(), "{name}: {:?}", home.names());
        }
    }

    #[test]
    fn an_older_release_offered_again_is_refused_once_a_newer_one_was_seen() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        // 0.5.0 is seen signed, but its archive cannot be fetched.
        publisher.publish(&feed, FEED, "0.5.0", ARCHIVE);
        feed.files
            .lock()
            .expect("files")
            .remove("http://127.0.0.1:9/releases/Interface-0.5.0-arm64-mac.zip");
        let plan = home.plan(&publisher, "0.3.9");
        assert_eq!(run(&plan, &feed).0, Err(MirrorError::Network));
        // The feed then serves the genuine, older 0.4.0 again.
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        assert_eq!(run(&plan, &feed).0, Err(MirrorError::Integrity));
        assert!(home.names().is_empty());
    }

    #[test]
    fn a_second_feed_serves_the_release_when_the_first_is_down_or_lying() {
        let second = "http://127.0.0.1:9/mirror/interface-update.json";
        for first_is_forged in [false, true] {
            let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
            if first_is_forged {
                Publisher::new().publish(&feed, FEED, "9.9.9", b"forged");
            }
            publisher.publish(&feed, second, "0.4.0", ARCHIVE);
            let mut plan = home.plan(&publisher, "0.3.9");
            plan.channel = publisher.channel(&[FEED, second]);
            assert_eq!(
                run(&plan, &feed).0,
                Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
            );
            assert_eq!(
                home.staged("Interface-0.4.0-arm64-mac.zip").as_deref(),
                Some(ARCHIVE)
            );
        }
    }

    #[test]
    fn a_release_already_staged_is_not_downloaded_twice_unless_it_was_damaged() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        let plan = home.plan(&publisher, "0.3.9");
        let archive = "http://127.0.0.1:9/releases/Interface-0.4.0-arm64-mac.zip";
        assert_eq!(
            run(&plan, &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        assert_eq!(
            run(&plan, &feed).0,
            Ok(MirrorOutcome::AlreadyStaged("0.4.0".to_owned()))
        );
        assert_eq!(feed.requested(archive), 1);

        fs::write(
            home.feed_dir().join("Interface-0.4.0-arm64-mac.zip"),
            b"torn",
        )
        .expect("damage");
        assert_eq!(
            run(&plan, &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        assert_eq!(feed.requested(archive), 2);
        assert_eq!(
            home.staged("Interface-0.4.0-arm64-mac.zip").as_deref(),
            Some(ARCHIVE)
        );
    }

    #[test]
    fn the_next_release_replaces_the_last_ones_archive() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        let plan = home.plan(&publisher, "0.3.9");
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        assert_eq!(
            run(&plan, &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        publisher.publish(&feed, FEED, "0.4.1", b"the next archive");
        assert_eq!(
            run(&plan, &feed).0,
            Ok(MirrorOutcome::Staged("0.4.1".to_owned()))
        );
        assert_eq!(
            home.names(),
            ["Interface-0.4.1-arm64-mac.zip", "latest-mac.yml"]
        );
    }

    /// A feed as `cargo xtask stage-update` leaves it: no signed manifest.
    fn stage_locally(home: &Home, version: &str, archive: &[u8]) {
        let manifest = manifest_for(version, archive);
        fs::write(home.feed_dir().join(&manifest.artifact.name), archive).expect("archive");
        fs::write(
            home.feed_dir().join("latest-mac.yml"),
            &manifest.latest_mac_yml,
        )
        .expect("feed");
    }

    #[test]
    fn a_newer_release_staged_on_this_mac_is_left_in_place() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        stage_locally(&home, "0.4.1", b"the owner's own newer build");
        let (outcome, events) = run(&home.plan(&publisher, "0.3.9"), &feed);
        assert_eq!(
            outcome,
            Ok(MirrorOutcome::AlreadyStaged("0.4.1".to_owned()))
        );
        assert!(events.is_empty());
        assert_eq!(
            home.names(),
            ["Interface-0.4.1-arm64-mac.zip", "latest-mac.yml"]
        );
        assert_eq!(feed.requests.lock().expect("requests").len(), 1);
    }

    #[test]
    fn the_same_release_staged_on_this_mac_is_recognised_by_its_bytes() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        stage_locally(&home, "0.4.0", ARCHIVE);
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Ok(MirrorOutcome::AlreadyStaged("0.4.0".to_owned()))
        );
        assert_eq!(feed.requests.lock().expect("requests").len(), 1);
    }

    #[test]
    fn an_older_release_staged_on_this_mac_gives_way_to_the_public_one() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        stage_locally(&home, "0.3.9", b"the build this Mac already runs");
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        assert_eq!(
            home.names(),
            ["Interface-0.4.0-arm64-mac.zip", "latest-mac.yml"]
        );
    }

    #[test]
    fn a_local_stage_in_progress_makes_the_mirror_wait() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        fs::write(home.feed_dir().join(STAGING_LOCK_NAME), b"{\"pid\":1}").expect("lock");
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Err(MirrorError::Busy)
        );
        assert_eq!(home.names(), [STAGING_LOCK_NAME]);
    }

    #[test]
    fn stopping_ends_a_download_and_leaves_nothing_behind() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        let outcome = refresh(
            &home.plan(&publisher, "0.3.9"),
            &feed,
            &mut |_| {},
            &AtomicBool::new(true),
        );
        assert_eq!(outcome, Err(MirrorError::Stopped));
        assert!(home.names().is_empty());
    }

    #[test]
    fn startup_keeps_a_staged_release_until_it_is_installed_then_frees_its_archive() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        fs::write(home.feed_dir().join(".mirror-1-2.partial"), b"half").expect("partial");

        prepare(&home.feed_dir(), &home.state_path(), "0.3.9");
        assert_eq!(
            home.names(),
            ["Interface-0.4.0-arm64-mac.zip", "latest-mac.yml"]
        );
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Ok(MirrorOutcome::AlreadyStaged("0.4.0".to_owned()))
        );

        prepare(&home.feed_dir(), &home.state_path(), "0.4.0");
        assert!(home.names().is_empty());
        assert_eq!(
            run(&home.plan(&publisher, "0.4.0"), &feed).0,
            Ok(MirrorOutcome::UpToDate)
        );
    }

    #[test]
    fn startup_never_removes_a_feed_staged_on_this_mac() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        // The owner then stages their own build over it and installs that.
        fs::remove_file(home.feed_dir().join("Interface-0.4.0-arm64-mac.zip")).expect("old");
        stage_locally(&home, "0.4.1", b"the owner's own newer build");
        prepare(&home.feed_dir(), &home.state_path(), "0.4.1");
        assert_eq!(
            home.names(),
            ["Interface-0.4.1-arm64-mac.zip", "latest-mac.yml"]
        );
    }

    #[test]
    fn a_stage_cut_short_before_its_feed_is_written_is_finished_by_the_next_check() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        // As a crash would leave it: the archive renamed into place, no feed yet.
        fs::write(
            home.feed_dir().join("Interface-0.4.0-arm64-mac.zip"),
            b"torn",
        )
        .expect("orphan");
        prepare(&home.feed_dir(), &home.state_path(), "0.3.9");
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        assert_eq!(
            home.staged("Interface-0.4.0-arm64-mac.zip").as_deref(),
            Some(ARCHIVE)
        );
    }

    /// A plain HTTP/1.1 server on loopback: each path answers with a canned
    /// response, written exactly as given.
    struct Loopback {
        port: u16,
        asked: Arc<Mutex<Vec<String>>>,
    }

    impl Loopback {
        fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().expect("address").port();
            let asked = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&asked);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let mut request = Vec::new();
                    let mut byte = [0_u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        match stream.read(&mut byte) {
                            Ok(1) => request.push(byte[0]),
                            _ => break,
                        }
                    }
                    let request = String::from_utf8_lossy(&request).into_owned();
                    let path = request.split(' ').nth(1).unwrap_or_default().to_owned();
                    seen.lock().expect("asked").push(path.clone());
                    let response = routes
                        .iter()
                        .find(|(route, _)| *route == path)
                        .map_or_else(
                            || b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                            |(_, response)| response.clone(),
                        );
                    let _ = stream.write_all(&response);
                }
            });
            Self { port, asked }
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }
    }

    fn ok(body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn signed_feed(publisher: &Publisher, version: &str, archive: &[u8]) -> Vec<u8> {
        sign_manifest(&manifest_for(version, archive), &publisher.pkcs8).expect("signed")
    }

    #[test]
    fn the_shipped_client_follows_the_feeds_redirect_and_stages_the_release() {
        let (home, publisher) = (Home::new(), Publisher::new());
        // As a release host answers "latest": a redirect to the versioned file.
        let server = Loopback::serve(vec![
            (
                "/latest/interface-update.json",
                b"HTTP/1.1 302 Found\r\nLocation: /v0.4.0/interface-update.json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            ),
            (
                "/v0.4.0/interface-update.json",
                ok(&signed_feed(&publisher, "0.4.0", ARCHIVE)),
            ),
            ("/latest/Interface-0.4.0-arm64-mac.zip", ok(ARCHIVE)),
        ]);
        let mut plan = home.plan(&publisher, "0.3.9");
        plan.channel = publisher.channel(&[&server.url("/latest/interface-update.json")]);
        let outcome = refresh(
            &plan,
            &UreqFetcher::allowing_loopback_http(),
            &mut |_| {},
            &AtomicBool::new(false),
        );
        assert_eq!(outcome, Ok(MirrorOutcome::Staged("0.4.0".to_owned())));
        assert_eq!(
            home.staged("Interface-0.4.0-arm64-mac.zip").as_deref(),
            Some(ARCHIVE)
        );
    }

    #[test]
    fn the_shipped_client_refuses_an_archive_longer_or_shorter_than_the_signed_one() {
        let longer = [ARCHIVE, b" and a tail nobody signed"].concat();
        let mut unannounced = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        unannounced.extend_from_slice(&longer);
        let mut cut_off = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            ARCHIVE.len()
        )
        .into_bytes();
        cut_off.extend_from_slice(&ARCHIVE[..10]);
        for (name, response, expected) in [
            ("announced longer", ok(&longer), MirrorError::Integrity),
            (
                "longer without a length",
                unannounced,
                MirrorError::Integrity,
            ),
            ("cut off", cut_off, MirrorError::Network),
        ] {
            let (home, publisher) = (Home::new(), Publisher::new());
            let server = Loopback::serve(vec![
                (
                    "/interface-update.json",
                    ok(&signed_feed(&publisher, "0.4.0", ARCHIVE)),
                ),
                ("/Interface-0.4.0-arm64-mac.zip", response),
            ]);
            let mut plan = home.plan(&publisher, "0.3.9");
            plan.channel = publisher.channel(&[&server.url("/interface-update.json")]);
            let outcome = refresh(
                &plan,
                &UreqFetcher::allowing_loopback_http(),
                &mut |_| {},
                &AtomicBool::new(false),
            );
            assert_eq!(outcome, Err(expected), "{name}");
            assert!(home.names().is_empty(), "{name}: {:?}", home.names());
        }
    }

    #[test]
    fn the_shipped_client_refuses_a_manifest_larger_than_any_real_one() {
        let (home, publisher) = (Home::new(), Publisher::new());
        let server = Loopback::serve(vec![(
            "/interface-update.json",
            ok(&vec![b' '; MAX_SIGNED_MANIFEST_BYTES + 1]),
        )]);
        let mut plan = home.plan(&publisher, "0.3.9");
        plan.channel = publisher.channel(&[&server.url("/interface-update.json")]);
        let outcome = refresh(
            &plan,
            &UreqFetcher::allowing_loopback_http(),
            &mut |_| {},
            &AtomicBool::new(false),
        );
        assert_eq!(outcome, Err(MirrorError::Integrity));
    }

    #[test]
    fn the_shipped_client_reads_a_missing_manifest_as_nothing_published() {
        let (home, publisher) = (Home::new(), Publisher::new());
        let server = Loopback::serve(Vec::new());
        let mut plan = home.plan(&publisher, "0.3.9");
        plan.channel = publisher.channel(&[&server.url("/latest/interface-update.json")]);
        let outcome = refresh(
            &plan,
            &UreqFetcher::allowing_loopback_http(),
            &mut |_| {},
            &AtomicBool::new(false),
        );
        assert_eq!(outcome, Ok(MirrorOutcome::NothingPublished));
    }

    #[test]
    fn the_client_a_published_app_runs_never_speaks_plain_http() {
        let server = Loopback::serve(vec![("/interface-update.json", ok(b"{}"))]);
        let url = Url::parse(&server.url("/interface-update.json")).expect("url");
        assert_eq!(
            UreqFetcher::new().fetch(&url, 1024),
            Err(FetchError::Unreachable)
        );
        assert!(server.asked.lock().expect("asked").is_empty());
        // The same address answers a client that is allowed loopback.
        assert_eq!(
            UreqFetcher::allowing_loopback_http().fetch(&url, 1024),
            Ok(b"{}".to_vec())
        );
    }

    #[test]
    fn a_damaged_or_missing_state_file_never_stops_the_channel() {
        let (home, feed, publisher) = (Home::new(), FakeFeed::default(), Publisher::new());
        publisher.publish(&feed, FEED, "0.4.0", ARCHIVE);
        assert_eq!(
            run(&home.plan(&publisher, "0.3.9"), &feed).0,
            Ok(MirrorOutcome::Staged("0.4.0".to_owned()))
        );
        fs::write(home.state_path(), b"{ not json").expect("damage");
        prepare(&home.feed_dir(), &home.state_path(), "0.4.0");
        publisher.publish(&feed, FEED, "0.4.1", b"the next archive");
        assert_eq!(
            run(&home.plan(&publisher, "0.4.0"), &feed).0,
            Ok(MirrorOutcome::Staged("0.4.1".to_owned()))
        );
        assert_eq!(
            home.names(),
            ["Interface-0.4.1-arm64-mac.zip", "latest-mac.yml"]
        );
    }
}
