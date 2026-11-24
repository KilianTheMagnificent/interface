//! `iface:diagnostics` (protocol 39): the host side of Help → Copy
//! Diagnostics and Reveal Logs, Settings → Reveal data folder, and the
//! one-time notice for a store the host set aside because it could not read
//! it. The report's content and its credential filter are
//! `interface_review::diagnostics`; this adapter gathers the facts (probes,
//! the log tail, the macOS version), keeps the notices for this run and asks
//! Finder to show things.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use interface_contracts::diagnostics::{
    DiagnosticsCommand, DiagnosticsResponse, RevealTarget, StoreNotice,
};
use interface_contracts::ipc::{CHANNELS, EngineProbe};
use interface_review::diagnostics::{
    DIAGNOSTIC_WARNING_LINES, DiagnosticsFacts, render_diagnostics,
};
use interface_runtime::SessionRuntime;
use serde_json::Value;
use tokio::task::JoinSet;

use crate::dispatcher::{DispatchError, HostDispatcher};
use crate::host_log::{HOST_LOG_FILE_NAME, recent_host_log_lines};
use crate::payload;

pub const DIAGNOSTICS_OPERATIONS: [&str; 1] = [CHANNELS.diagnostics];

const INVALID_CODE: &str = "diagnostics-invalid";
const UNKNOWN_NOTICE_CODE: &str = "diagnostics-unknown-notice";
const REVEAL_FAILED_CODE: &str = "diagnostics-reveal-failed";
const REPORT_FAILED_CODE: &str = "diagnostics-report-failed";
/// A report waits this long for one provider's probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const SYSTEM_VERSION_PLIST: &str = "/System/Library/CoreServices/SystemVersion.plist";

/// A store the host set aside at startup: what it held, in words, and
/// where the unreadable bytes went.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetAsideStore {
    /// `threads.v4.json`.
    Threads,
    /// `session-access.v1.json`.
    PermissionModes,
    /// `registry.json`.
    Sessions,
    /// A segment of `thread-journal/` (protocol 56).
    ThreadJournal,
    /// `thread-team.v1.json` (protocol 57).
    ThreadTeam,
    /// `draft-stash.v1.json` (protocol 63).
    DraftStash,
    /// `thread-schedules.v2.json` (protocol 62; version 2 since protocol 66).
    ThreadSchedules,
}

impl SetAsideStore {
    const fn what(self) -> &'static str {
        match self {
            Self::Threads => "your Threads list",
            Self::PermissionModes => "the permission modes your threads resume in",
            Self::Sessions => "your saved session names and archive marks",
            Self::ThreadJournal => "part of a thread's saved history",
            Self::ThreadTeam => "which threads your threads started",
            Self::DraftStash => "your stashed drafts",
            Self::ThreadSchedules => "your scheduled prompts and pull request watches",
        }
    }

    const fn slug(self) -> &'static str {
        match self {
            Self::Threads => "threads",
            Self::PermissionModes => "permissions",
            Self::Sessions => "sessions",
            Self::ThreadJournal => "thread-journal",
            Self::ThreadTeam => "thread-team",
            Self::DraftStash => "draft-stash",
            Self::ThreadSchedules => "thread-schedules",
        }
    }
}

#[derive(Clone, Debug)]
struct NoticeEntry {
    notice: StoreNotice,
    artifact: PathBuf,
}

/// This run's set-aside notices. Each is shown until the owner dismisses
/// it; none survives a restart (the file stays set aside, and the next
/// start opens cleanly, so nothing would be new to say).
#[derive(Debug, Default)]
pub struct StoreNotices {
    entries: Mutex<Vec<NoticeEntry>>,
    next: Mutex<u32>,
}

impl StoreNotices {
    /// Records that `store` could not be read and its bytes now live at
    /// `artifact`, and logs it.
    pub fn record(&self, store: SetAsideStore, artifact: &Path) {
        tracing::warn!(
            store = store.slug(),
            artifact = %artifact.display(),
            "a store could not be read; it was set aside and starts empty"
        );
        let id = {
            let mut next = interface_common::sync::lock(&self.next);
            *next = next.saturating_add(1);
            format!("{}-{}", store.slug(), *next)
        };
        interface_common::sync::lock(&self.entries).push(NoticeEntry {
            notice: StoreNotice {
                id,
                what: store.what().to_owned(),
            },
            artifact: artifact.to_path_buf(),
        });
    }

    #[must_use]
    pub fn current(&self) -> Vec<StoreNotice> {
        interface_common::sync::lock(&self.entries)
            .iter()
            .map(|entry| entry.notice.clone())
            .collect()
    }

    fn dismiss(&self, id: &str) -> bool {
        let mut entries = interface_common::sync::lock(&self.entries);
        let before = entries.len();
        entries.retain(|entry| entry.notice.id != id);
        entries.len() != before
    }

    fn artifact(&self, id: &str) -> Option<PathBuf> {
        interface_common::sync::lock(&self.entries)
            .iter()
            .find(|entry| entry.notice.id == id)
            .map(|entry| entry.artifact.clone())
    }
}

/// Each registered provider's probe, the way `probeEngines` reports it.
#[async_trait]
pub trait ProviderProbes: Send + Sync + 'static {
    async fn probes(&self) -> Vec<EngineProbe>;
}

#[async_trait]
impl ProviderProbes for SessionRuntime {
    /// A kept probe answers at once; a provider never probed (or whose
    /// probe failed) is asked now, for at most [`PROBE_TIMEOUT`].
    async fn probes(&self) -> Vec<EngineProbe> {
        let mut work = JoinSet::new();
        for engine in self.registered_engines() {
            let runtime = self.clone();
            work.spawn(async move {
                match tokio::time::timeout(PROBE_TIMEOUT, runtime.probe_provider(&engine)).await {
                    Ok(readiness) => crate::dispatcher::engine_probes(vec![readiness]),
                    Err(_) => vec![EngineProbe {
                        engine,
                        ok: false,
                        version: None,
                        info: None,
                        error: Some("did not answer within 20 seconds".into()),
                    }],
                }
            });
        }
        let mut probes = Vec::new();
        while let Some(joined) = work.join_next().await {
            if let Ok(found) = joined {
                probes.extend(found);
            }
        }
        probes.sort_by(|left, right| left.engine.as_str().cmp(right.engine.as_str()));
        probes
    }
}

/// The app's own version, as main told the updater at startup.
#[async_trait]
pub trait AppVersion: Send + Sync + 'static {
    async fn app_version(&self) -> Option<String>;
}

#[async_trait]
impl AppVersion for interface_updater::UpdaterService {
    async fn app_version(&self) -> Option<String> {
        let version = self.state().await.current_version;
        interface_common::version::is_product_version(&version).then_some(version)
    }
}

/// Shows a path in Finder.
pub trait Revealer: Send + Sync + 'static {
    /// `select`: open the enclosing folder with `path` selected; otherwise
    /// open the folder `path` itself.
    ///
    /// # Errors
    /// Finder could not be asked.
    fn reveal(&self, path: &Path, select: bool) -> Result<(), String>;
}

/// `/usr/bin/open`, the way an agent home is revealed.
pub struct FinderRevealer;

impl Revealer for FinderRevealer {
    fn reveal(&self, path: &Path, select: bool) -> Result<(), String> {
        let mut command = std::process::Command::new("/usr/bin/open");
        if select {
            command.arg("-R");
        }
        let status = command
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|error| format!("Finder could not be asked: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("Finder could not show it.".into())
        }
    }
}

pub struct DiagnosticsHostConfig {
    pub data_root: PathBuf,
    pub home: PathBuf,
    /// The host binary's version (the release stamp, else the crate's).
    pub product_version: String,
    /// The app's version, which a Dev or test build's host does not share.
    pub app_version: Option<Arc<dyn AppVersion>>,
    pub probes: Arc<dyn ProviderProbes>,
    pub notices: Arc<StoreNotices>,
    pub revealer: Arc<dyn Revealer>,
}

/// The owner of `iface:diagnostics`.
pub struct DiagnosticsHost {
    config: DiagnosticsHostConfig,
}

impl DiagnosticsHost {
    #[must_use]
    pub const fn new(config: DiagnosticsHostConfig) -> Self {
        Self { config }
    }

    async fn report(&self) -> Result<String, DispatchError> {
        let providers = self.config.probes.probes().await;
        let app_version = match &self.config.app_version {
            Some(source) => source.app_version().await,
            None => None,
        };
        let version = match app_version {
            Some(app) if app != self.config.product_version => {
                format!("{app} (host {})", self.config.product_version)
            }
            Some(app) => app,
            None => self.config.product_version.clone(),
        };
        let data_root = self.config.data_root.clone();
        let (warnings, os) = tokio::task::spawn_blocking(move || {
            (
                recent_host_log_lines(&data_root, DIAGNOSTIC_WARNING_LINES),
                macos_version(Path::new(SYSTEM_VERSION_PLIST)),
            )
        })
        .await
        .map_err(|_| {
            DispatchError::new(
                REPORT_FAILED_CODE,
                "Interface could not gather its diagnostics.",
                true,
                true,
            )
        })?;
        Ok(render_diagnostics(&DiagnosticsFacts {
            version: &version,
            os: os.as_deref(),
            arch: std::env::consts::ARCH,
            data_root: &self.config.data_root,
            home: Some(&self.config.home),
            providers: &providers,
            warnings: &warnings,
        }))
    }

    async fn reveal(&self, target: RevealTarget, id: Option<&str>) -> Result<(), DispatchError> {
        let (path, select) = match target {
            RevealTarget::Logs => {
                let log = self.config.data_root.join(HOST_LOG_FILE_NAME);
                // Before the first warning there is no file yet: show the folder.
                if log.is_file() {
                    (log, true)
                } else {
                    (self.config.data_root.clone(), false)
                }
            }
            RevealTarget::DataFolder => (self.config.data_root.clone(), false),
            RevealTarget::Notice => {
                let artifact = id
                    .and_then(|id| self.config.notices.artifact(id))
                    .ok_or_else(unknown_notice)?;
                (artifact, true)
            }
        };
        let revealer = self.config.revealer.clone();
        tokio::task::spawn_blocking(move || revealer.reveal(&path, select))
            .await
            .map_err(|_| reveal_failed("Finder could not be asked."))?
            .map_err(|message| reveal_failed(&message))
    }
}

#[async_trait]
impl HostDispatcher for DiagnosticsHost {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation != CHANNELS.diagnostics {
            return Err(DispatchError::new(
                crate::dispatcher::OPERATION_NOT_MIGRATED_CODE,
                "This Interface operation has not migrated to the Rust host.",
                false,
                true,
            ));
        }
        let command: DiagnosticsCommand = payload::decode_object(payload, |_| invalid())?;
        command.validate().map_err(|_| invalid())?;
        let response = match command {
            DiagnosticsCommand::Report {} => DiagnosticsResponse {
                report: Some(self.report().await?),
                notices: None,
            },
            DiagnosticsCommand::Notices {} => DiagnosticsResponse {
                report: None,
                notices: Some(self.config.notices.current()),
            },
            DiagnosticsCommand::DismissNotice { id } => {
                // Dismissing one already gone is a no-op: two windows, or a
                // retried click, settle on the same list.
                let _ = self.config.notices.dismiss(&id);
                DiagnosticsResponse {
                    report: None,
                    notices: Some(self.config.notices.current()),
                }
            }
            DiagnosticsCommand::Reveal { target, id } => {
                self.reveal(target, id.as_deref()).await?;
                DiagnosticsResponse::default()
            }
        };
        serde_json::to_value(response).map_err(|_| {
            DispatchError::new(
                REPORT_FAILED_CODE,
                "Interface could not encode its diagnostics.",
                false,
                true,
            )
        })
    }
}

fn invalid() -> DispatchError {
    DispatchError::new(
        INVALID_CODE,
        "That diagnostics request is not one Interface understands.",
        false,
        true,
    )
}

fn unknown_notice() -> DispatchError {
    DispatchError::new(
        UNKNOWN_NOTICE_CODE,
        "That notice is no longer open.",
        false,
        true,
    )
}

fn reveal_failed(message: &str) -> DispatchError {
    // Finder may have opened before failing; never repeat it unasked.
    DispatchError::new(REVEAL_FAILED_CODE, message, true, false)
}

/// `macOS 26.1 (25B78)` from the system's own version file.
fn macos_version(plist: &Path) -> Option<String> {
    let text = std::fs::read_to_string(plist).ok()?;
    if text.len() > 64 * 1024 {
        return None;
    }
    let value = |key: &str| -> Option<String> {
        let after = text.split(&format!("<key>{key}</key>")).nth(1)?;
        let start = after.find("<string>")? + "<string>".len();
        let end = after[start..].find("</string>")? + start;
        let value = after[start..end].trim();
        (!value.is_empty() && value.len() <= 32).then(|| value.to_owned())
    };
    let version = value("ProductVersion")?;
    Some(match value("ProductBuildVersion") {
        Some(build) => format!("macOS {version} ({build})"),
        None => format!("macOS {version}"),
    })
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
